use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering},
    },
};

use futures::{StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use serde_json::{Value as JsonValue, json};
use tm_host::{
    ApprovalDecision, ApprovalPolicy, HostError, HostEventSink, HostRegistry, InvocationCtx,
};

use crate::{
    BinaryOp, Callable, CapabilityCatalog, Cell, Diagnostic, EffectMachine, EffectSignature,
    Environment, Expr, ExprKind, Form, FormKind, MatchArm, Pattern, PatternKind, Span, TypeDecl,
    TypeTerm, UnaryOp, Value, ValueType, check_with_bindings_bounded, parser::parse_bounded,
    value::json_semantic_eq_bounded,
};

#[derive(Debug, Clone, thiserror::Error)]
pub enum RuntimeError {
    #[error("{0}")]
    Diagnostic(#[from] Diagnostic),
    #[error("{name}: {message}")]
    Effect {
        name: String,
        message: String,
        payload: Option<JsonValue>,
    },
    #[error("TypeError: {0}")]
    Type(String),
    #[error("ResourceLimitError: {0}")]
    Limit(String),
    #[error("RuntimePersistenceError: {0}")]
    Persistence(String),
    #[error("CancellationError: cell cancelled")]
    Cancelled,
}

pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;

const MAX_SAFE_RUNTIME_DEPTH: usize = 128;

#[derive(Debug, Clone)]
pub struct RuntimeLimits {
    pub steps: u64,
    pub print_bytes: usize,
    pub preview_bytes: usize,
    pub source_bytes: usize,
    pub syntax_nodes: usize,
    pub parse_depth: usize,
    pub runtime_depth: usize,
    pub value_bytes: usize,
    pub environment_bytes: usize,
    pub parallelism: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            steps: 100_000,
            print_bytes: 8 * 1024,
            preview_bytes: 1024,
            source_bytes: 256 * 1024,
            syntax_nodes: 100_000,
            parse_depth: 256,
            runtime_depth: MAX_SAFE_RUNTIME_DEPTH,
            value_bytes: 4 * 1024 * 1024,
            environment_bytes: 16 * 1024 * 1024,
            parallelism: 8,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeOutput {
    pub stdout: String,
    pub value: Value,
    pub committed: BTreeSet<String>,
}

#[derive(Clone)]
pub struct Interpreter {
    env: Environment,
    catalog: CapabilityCatalog,
    registry: Arc<HostRegistry>,
    invocation: InvocationCtx,
    limits: RuntimeLimits,
    cell_counter: u64,
    active: Arc<Mutex<Option<ActiveExecution>>>,
    terminal_selected: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug, Clone)]
struct ActiveEffect {
    scope_id: Option<String>,
    machine: Arc<Mutex<EffectMachine>>,
    terminal: Option<JsonValue>,
}

#[derive(Debug, Clone)]
struct ActiveExecution {
    cell_id: String,
    effects: BTreeMap<String, ActiveEffect>,
    scopes: BTreeMap<String, ActiveScope>,
    cell_terminal: Option<JsonValue>,
    cell_started: bool,
}

#[derive(Debug, Clone)]
struct ActiveScope {
    total: Option<usize>,
    completed: usize,
    parent_node_id: Option<String>,
    terminal: Option<JsonValue>,
}

fn active_scope_depth(node: &str, scopes: &BTreeMap<String, ActiveScope>) -> usize {
    let mut depth = 0usize;
    let mut current = Some(node);
    let mut visited = BTreeSet::new();
    while let Some(node) = current {
        if !visited.insert(node.to_string()) {
            break;
        }
        let Some(scope) = scopes.get(node) else {
            break;
        };
        depth = depth.saturating_add(1);
        current = scope.parent_node_id.as_deref();
    }
    depth
}

impl Interpreter {
    pub fn new(
        catalog: CapabilityCatalog,
        registry: Arc<HostRegistry>,
        invocation: InvocationCtx,
        mut limits: RuntimeLimits,
    ) -> Self {
        limits.runtime_depth = limits.runtime_depth.min(MAX_SAFE_RUNTIME_DEPTH);
        Self {
            env: prelude(),
            catalog,
            registry,
            invocation,
            limits,
            cell_counter: 0,
            active: Arc::new(Mutex::new(None)),
            terminal_selected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn environment(&self) -> &Environment {
        &self.env
    }
    pub fn reset(&mut self) {
        self.env = prelude();
        self.cell_counter = 0;
        *self.active.lock().expect("active execution lock poisoned") = None;
    }

    pub(crate) fn fork_for_parallel(&self, offset: u64) -> Self {
        let mut fork = self.clone();
        fork.cell_counter += offset;
        fork.active = Arc::new(Mutex::new(None));
        fork.terminal_selected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        fork
    }

    pub(crate) fn terminal_selected_handle(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.terminal_selected)
    }

    pub(crate) fn abandon_active_eval(&mut self) {
        *self.active.lock().expect("active execution lock poisoned") = None;
        self.terminal_selected.store(true, AtomicOrdering::Release);
    }

    pub(crate) async fn cancel_active_eval(
        &mut self,
        status: &str,
        _reason: &str,
    ) -> RuntimeResult<()> {
        let active = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .take();
        let Some(active) = active else {
            return Ok(());
        };
        self.terminal_selected.store(true, AtomicOrdering::Release);
        if !active.cell_started {
            self.invocation
                .emit_event(
                    "cell_start",
                    json!({"cellId": active.cell_id, "sourcePreview": "[redacted]"}),
                )
                .await
                .map_err(runtime_event_error)?;
        }
        for (node_id, effect) in &active.effects {
            let terminal = if let Some(terminal) = &effect.terminal {
                terminal.clone()
            } else {
                let _ = effect
                    .machine
                    .lock()
                    .expect("effect machine lock poisoned")
                    .cancel();
                json!({"cellId": active.cell_id, "nodeId": node_id, "parentNodeId": effect.scope_id, "status": status, "error": "[redacted]"})
            };
            self.invocation
                .emit_event("effect_result", terminal)
                .await
                .map_err(runtime_event_error)?;
        }
        let mut scopes = active.scopes.iter().collect::<Vec<_>>();
        scopes.sort_by(|(left_node, _), (right_node, _)| {
            active_scope_depth(right_node, &active.scopes)
                .cmp(&active_scope_depth(left_node, &active.scopes))
                .then_with(|| left_node.cmp(right_node))
        });
        for (node_id, scope) in scopes {
            let terminal = scope.terminal.clone().unwrap_or_else(|| {
                json!({"cellId": active.cell_id, "nodeId": node_id, "parentNodeId": scope.parent_node_id, "status": status, "error": "[redacted]"})
            });
            self.invocation
                .emit_event("scope_result", terminal)
                .await
                .map_err(runtime_event_error)?;
        }
        let terminal = active.cell_terminal.unwrap_or_else(
            || json!({"cellId": active.cell_id, "status": status, "error": "[redacted]"}),
        );
        self.invocation
            .emit_event("cell_result", terminal)
            .await
            .map_err(runtime_event_error)
    }

    pub(crate) fn finish_parallel_batch(&mut self, count: u64) {
        self.cell_counter += count;
    }

    pub(crate) fn merge_committed_from(&mut self, fork: &Self, committed: &BTreeSet<String>) {
        for name in committed {
            if let Some(value) = fork.env.get(name) {
                self.env.insert(name.clone(), value.clone());
            }
        }
    }

    pub(crate) async fn emit_immediate_terminal(
        &mut self,
        _source: &str,
        status: &str,
        _reason: &str,
    ) -> RuntimeResult<()> {
        self.cell_counter += 1;
        let cell_id = format!("cell-{}", self.cell_counter);
        self.terminal_selected.store(true, AtomicOrdering::Release);
        self.invocation
            .emit_event(
                "cell_start",
                json!({"cellId": cell_id, "sourcePreview": "[redacted]"}),
            )
            .await
            .map_err(runtime_event_error)?;
        self.invocation
            .emit_event(
                "cell_result",
                json!({"cellId": cell_id, "status": status, "error": "[redacted]"}),
            )
            .await
            .map_err(runtime_event_error)
    }

    pub(crate) async fn emit_dependency_failure(
        &mut self,
        _source: &str,
        _error: &str,
    ) -> RuntimeResult<()> {
        self.cell_counter += 1;
        let cell_id = format!("cell-{}", self.cell_counter);
        self.invocation
            .emit_event(
                "cell_start",
                json!({"cellId": cell_id, "sourcePreview": "[redacted]"}),
            )
            .await
            .map_err(runtime_event_error)?;
        self.invocation
            .emit_event(
                "cell_result",
                json!({"cellId": cell_id, "status": "failed", "error": "[redacted dependency error]"}),
            )
            .await
            .map_err(runtime_event_error)
    }

    async fn emit_validation_failure(&mut self, cell_id: &str, phase: &str) -> RuntimeResult<()> {
        let result = async {
            self.invocation
                .emit_event(
                    "cell_start",
                    json!({"cellId": cell_id, "sourcePreview": "[redacted]"}),
                )
                .await
                .map_err(runtime_event_error)?;
            if let Some(active) = self
                .active
                .lock()
                .expect("active execution lock poisoned")
                .as_mut()
            {
                active.cell_started = true;
            }
            let terminal = json!({
                "cellId": cell_id,
                "status": "failed",
                "error": format!("[redacted {phase} error]")
            });
            if let Some(active) = self
                .active
                .lock()
                .expect("active execution lock poisoned")
                .as_mut()
            {
                active.cell_terminal = Some(terminal.clone());
            }
            self.terminal_selected.store(true, AtomicOrdering::Release);
            self.invocation
                .emit_event("cell_result", terminal)
                .await
                .map_err(runtime_event_error)
        }
        .await;
        *self.active.lock().expect("active execution lock poisoned") = None;
        result
    }

    pub async fn eval(
        &mut self,
        source: &str,
        output_bytes: usize,
    ) -> RuntimeResult<RuntimeOutput> {
        if self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .is_some()
        {
            self.cancel_active_eval("failed", "previous terminal persistence failed")
                .await?;
        }
        self.cell_counter += 1;
        let cell_id = format!("cell-{}", self.cell_counter);
        *self.active.lock().expect("active execution lock poisoned") = Some(ActiveExecution {
            cell_id: cell_id.clone(),
            effects: BTreeMap::new(),
            scopes: BTreeMap::new(),
            cell_terminal: None,
            cell_started: false,
        });
        self.terminal_selected.store(false, AtomicOrdering::Release);
        let cell = match parse_bounded(
            source,
            self.limits.source_bytes,
            self.limits.syntax_nodes,
            self.limits.parse_depth,
        ) {
            Ok(cell) => cell,
            Err(error) => {
                self.emit_validation_failure(&cell_id, "parse").await?;
                return Err(error.into());
            }
        };
        let _checked = match check_with_bindings_bounded(
            source,
            &cell,
            &self.catalog,
            self.env.keys().cloned(),
            self.limits.source_bytes,
            self.limits.syntax_nodes,
            self.limits.parse_depth,
        ) {
            Ok(checked) => checked,
            Err(error) => {
                self.emit_validation_failure(&cell_id, "check").await?;
                return Err(error.into());
            }
        };
        // Durable telemetry is content-blind for every cell. Authority can flow through a
        // persistent binding into a later pure cell, and even a pure literal can be sensitive;
        // effect-row inspection alone therefore cannot safely decide what may be persisted.
        let sensitive_source = true;
        let mut evaluator = Evaluator {
            env: self.env.clone(),
            catalog: self.catalog.clone(),
            registry: Arc::clone(&self.registry),
            invocation: self.invocation.clone(),
            limits: self.limits.clone(),
            steps: Arc::new(AtomicU64::new(0)),
            stdout: Arc::new(Mutex::new(String::new())),
            output_bytes,
            output_used: Arc::new(AtomicUsize::new(0)),
            cell_id: cell_id.clone(),
            node_counter: Arc::new(AtomicU64::new(0)),
            constructors: BTreeMap::new(),
            committed: BTreeSet::new(),
            active: Arc::clone(&self.active),
            terminal_selected: Arc::clone(&self.terminal_selected),
            scope_id: None,
            sensitive_cell: sensitive_source,
            current_error: None,
            depth: 0,
        };
        let source_preview = if sensitive_source {
            "[redacted]".to_string()
        } else {
            bounded(source, self.limits.preview_bytes)
        };
        let start_payload = if evaluator.reserve_preview(&source_preview).is_ok() {
            json!({"cellId": cell_id, "sourcePreview": source_preview})
        } else {
            json!({"cellId": cell_id, "sourcePreviewTruncated": true})
        };
        if let Err(error) = evaluator.emit("cell_start", start_payload).await {
            *self.active.lock().expect("active execution lock poisoned") = None;
            return Err(error);
        }
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
        {
            active.cell_started = true;
        }
        let result = evaluator.cell(&cell).await;
        match result {
            Ok(value) => {
                if merged_environment_size_bounded(
                    &self.env,
                    &evaluator.env,
                    &evaluator.committed,
                    self.limits.environment_bytes,
                    self.limits.runtime_depth,
                ) > self.limits.environment_bytes
                {
                    let error =
                        RuntimeError::Limit("persistent environment budget exceeded".into());
                    return Err(evaluator
                        .terminal_error(
                            json!({"cellId": cell_id, "status": "failed", "error": "[redacted]"}),
                            error,
                        )
                        .await);
                }
                let remaining = evaluator.remaining_output();
                let Some((rendered_len, rendered)) =
                    value_json_bounded(&value, remaining, true, self.limits.runtime_depth)
                else {
                    let error = RuntimeError::Limit("result/output budget exceeded".into());
                    return Err(evaluator
                        .terminal_error(
                            json!({"cellId": cell_id, "status": "failed", "error": "[redacted]"}),
                            error,
                        )
                        .await);
                };
                if let Err(error) = evaluator.reserve_output(rendered_len) {
                    return Err(evaluator
                        .terminal_error(
                            json!({"cellId": cell_id, "status": "failed", "error": "[redacted]"}),
                            error,
                        )
                        .await);
                }
                // From this point onward cancellation must await the commit protocol. Install the
                // preflighted values before the first cancellable persistence await. A dropped
                // eval future can then never leave a durable `binding_committed` event without
                // the matching in-memory state. An explicit sink failure still rolls the install
                // back synchronously because the session remains exclusively borrowed here.
                self.terminal_selected.store(true, AtomicOrdering::Release);
                let mut previous_bindings = Vec::with_capacity(evaluator.committed.len());
                for name in &evaluator.committed {
                    if let Some(value) = evaluator.env.get(name) {
                        previous_bindings
                            .push((name.clone(), self.env.insert(name.clone(), value.clone())));
                    }
                }
                let binding_payload = json!({
                    "cellId": cell_id,
                    "bindingCount": evaluator.committed.len(),
                    "namesRedacted": true
                });
                if let Err(error) = evaluator.emit("binding_committed", binding_payload).await {
                    for (name, previous) in previous_bindings {
                        if let Some(previous) = previous {
                            self.env.insert(name, previous);
                        } else {
                            self.env.remove(&name);
                        }
                    }
                    return Err(evaluator
                        .terminal_error(
                            json!({"cellId": cell_id, "status": "failed", "error": "[redacted]"}),
                            error,
                        )
                        .await);
                }
                let result_preview = if evaluator.sensitive_cell {
                    "[redacted]".to_string()
                } else {
                    bounded(&rendered, self.limits.preview_bytes)
                };
                let result_payload = if evaluator.reserve_preview(&result_preview).is_ok() {
                    json!({"cellId": cell_id, "status": "completed", "resultPreview": result_preview})
                } else {
                    json!({"cellId": cell_id, "status": "completed", "previewTruncated": true})
                };
                evaluator.emit_cell_terminal(result_payload).await?;
                let stdout = evaluator
                    .stdout
                    .lock()
                    .expect("stdout lock poisoned")
                    .clone();
                Ok(RuntimeOutput {
                    stdout,
                    value,
                    committed: evaluator.committed,
                })
            }
            Err(error) => {
                let error_preview = if evaluator.sensitive_cell {
                    "[redacted]".to_string()
                } else {
                    bounded_display(&error, self.limits.preview_bytes)
                };
                let terminal = if evaluator.reserve_preview(&error_preview).is_ok() {
                    json!({"cellId": cell_id, "status": "failed", "error": error_preview})
                } else {
                    json!({"cellId": cell_id, "status": "failed", "errorTruncated": true})
                };
                Err(evaluator.terminal_error(terminal, error).await)
            }
        }
    }
}

mod builtins;
mod effects;
mod evaluator;
mod host;
mod limits;
mod operators;
mod parallel;
mod render;
mod sizing;

use evaluator::Evaluator;
use host::*;
use operators::*;
use render::*;
pub use sizing::catalog_from_registry;
use sizing::*;

#[cfg(test)]
mod tests;

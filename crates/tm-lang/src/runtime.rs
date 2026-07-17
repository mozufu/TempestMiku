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

#[derive(Clone)]
struct Evaluator {
    env: Environment,
    catalog: CapabilityCatalog,
    registry: Arc<HostRegistry>,
    invocation: InvocationCtx,
    limits: RuntimeLimits,
    steps: Arc<AtomicU64>,
    stdout: Arc<Mutex<String>>,
    output_bytes: usize,
    output_used: Arc<AtomicUsize>,
    cell_id: String,
    node_counter: Arc<AtomicU64>,
    constructors: BTreeMap<String, bool>,
    committed: BTreeSet<String>,
    active: Arc<Mutex<Option<ActiveExecution>>>,
    terminal_selected: Arc<std::sync::atomic::AtomicBool>,
    scope_id: Option<String>,
    sensitive_cell: bool,
    current_error: Option<RuntimeError>,
    depth: usize,
}

impl Evaluator {
    async fn cell(&mut self, cell: &Cell) -> RuntimeResult<Value> {
        let mut value = Value::Null;
        for form in &cell.forms {
            value = self.form(form, true).await?;
        }
        Ok(value)
    }

    fn form<'a>(
        &'a mut self,
        form: &'a Form,
        persistent: bool,
    ) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            self.step(form.span)?;
            match &form.node {
                FormKind::Type(decl) => {
                    for variant in &decl.variants {
                        self.constructors
                            .insert(variant.name.clone(), variant.payload.is_some());
                        self.env.insert(
                            variant.name.clone(),
                            if variant.payload.is_some() {
                                Value::Callable(Rc::new(Callable::Constructor {
                                    name: variant.name.clone(),
                                }))
                            } else {
                                Value::Tagged {
                                    name: variant.name.clone(),
                                    payload: None,
                                }
                            },
                        );
                    }
                    Ok(Value::Null)
                }
                FormKind::Let { pattern, value } => {
                    let value = self.expr(value).await?;
                    let bindings = self
                        .match_pattern(pattern, &value)?
                        .ok_or_else(|| RuntimeError::Type("let pattern did not match".into()))?;
                    for (name, value) in bindings {
                        self.env.insert(name.clone(), value);
                        if persistent {
                            self.committed.insert(name);
                        }
                    }
                    Ok(value)
                }
                FormKind::Fun { name, params, body } => {
                    self.ensure_capture(name.len())?;
                    let function = Value::Callable(Rc::new(Callable::User {
                        params: params.clone(),
                        body: body.clone(),
                        captured: Rc::new(self.env.clone()),
                        args: Vec::new(),
                    }));
                    self.env.insert(name.clone(), function.clone());
                    if persistent {
                        self.committed.insert(name.clone());
                    }
                    Ok(function)
                }
                FormKind::Expr(expr) => self.expr(expr).await,
            }
        })
    }

    fn expr<'a>(&'a mut self, expr: &'a Expr) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            self.enter_runtime_depth()?;
            let evaluated = async {
                self.step(expr.span)?;
                let result: RuntimeResult<Value> = async {
                match &expr.node {
                ExprKind::String(value) => Ok(Value::String(self.interpolate(value).await?)),
                ExprKind::Int(value) => Ok(Value::Int(*value)),
                ExprKind::Decimal(value) => Ok(Value::Decimal(*value)),
                ExprKind::Bool(value) => Ok(Value::Bool(*value)),
                ExprKind::Null => Ok(Value::Null),
                ExprKind::Uri(value) => Ok(Value::Uri(value.clone())),
                ExprKind::Name(name) => self
                    .env
                    .get(name)
                    .cloned()
                    .ok_or_else(|| RuntimeError::Type(format!("unbound name {name}"))),
                ExprKind::Constructor(name) => {
                    Ok(self.env.get(name).cloned().unwrap_or_else(|| {
                        if name == "None" {
                            return Value::Tagged {
                                name: name.clone(),
                                payload: None,
                            };
                        }
                        Value::Callable(Rc::new(Callable::Constructor { name: name.clone() }))
                    }))
                }
                ExprKind::Capability(name) => Ok(Value::Callable(Rc::new(Callable::Capability {
                    name: name.clone(),
                }))),
                ExprKind::List(values) => {
                    let mut output = Vec::with_capacity(values.len());
                    let mut allocated = 0;
                    for value in values {
                        let value = self.expr(value).await?;
                        self.add_value_size(&mut allocated, &value, 0)?;
                        output.push(value);
                    }
                    Ok(Value::List(output))
                }
                ExprKind::Record(fields) => {
                    let mut output = BTreeMap::new();
                    let mut allocated = 0;
                    for (name, value) in fields {
                        let value = self.expr(value).await?;
                        self.add_value_size(&mut allocated, &value, name.len())?;
                        output.insert(name.clone(), value);
                    }
                    Ok(Value::Record(output))
                }
                ExprKind::Lambda { params, body } => {
                    self.ensure_capture(0)?;
                    Ok(Value::Callable(Rc::new(Callable::User {
                        params: params.clone(),
                        body: (**body).clone(),
                        captured: Rc::new(self.env.clone()),
                        args: Vec::new(),
                    })))
                }
                ExprKind::Apply { function, argument }
                    if matches!(&function.node, ExprKind::Name(name) if name == "par")
                        && matches!(&argument.node, ExprKind::Record(_)) =>
                {
                    self.parallel_expr(argument).await
                }
                ExprKind::Apply { function, argument } => {
                    let row_argument = matches!(
                        &function.node,
                        ExprKind::Name(name)
                            if matches!(name.as_str(), "where" | "select" | "sort_by" | "group_by" | "aggregate")
                    );
                    let function = self.expr(function).await?;
                    let argument = if row_argument {
                        self.ensure_capture(0)?;
                        Value::Callable(Rc::new(Callable::Row {
                            body: (**argument).clone(),
                            captured: Rc::new(self.env.clone()),
                        }))
                    } else {
                        self.expr(argument).await?
                    };
                    self.apply(function, argument).await
                }
                ExprKind::Field { target, field } => match self.expr(target).await? {
                    Value::Record(mut fields) => fields
                        .remove(field)
                        .ok_or_else(|| RuntimeError::Type(format!("missing field {field}"))),
                    other => Err(RuntimeError::Type(format!(
                        "field access on {}",
                        other.kind()
                    ))),
                },
                ExprKind::Unary { op, value } => unary(*op, self.expr(value).await?),
                ExprKind::Binary { op, left, right } => {
                    let left = self.expr(left).await?;
                    match (*op, &left) {
                        (BinaryOp::And, Value::Bool(false)) | (BinaryOp::Or, Value::Bool(true)) => {
                            return Ok(left);
                        }
                        (BinaryOp::And | BinaryOp::Or, Value::Bool(true | false)) => {}
                        (BinaryOp::And | BinaryOp::Or, other) => {
                            return Err(RuntimeError::Type(format!(
                                "{} requires boolean operands; found {}",
                                logical_name(*op),
                                other.kind()
                            )));
                        }
                        _ => {}
                    }
                    let right = self.expr(right).await?;
                    match op {
                        BinaryOp::Equal => Ok(Value::Bool(self.values_equal(&left, &right)?)),
                        BinaryOp::NotEqual => Ok(Value::Bool(!self.values_equal(&left, &right)?)),
                        _ => binary(*op, left, right),
                    }
                }
                ExprKind::Pipe { value, target } => {
                    let value = self.expr(value).await?;
                    let target = self.expr(target).await?;
                    self.apply_last(target, value).await
                }
                ExprKind::If {
                    condition,
                    then_value,
                    else_value,
                } => match self.expr(condition).await? {
                    Value::Bool(true) => self.expr(then_value).await,
                    Value::Bool(false) => self.expr(else_value).await,
                    other => Err(RuntimeError::Type(format!(
                        "if condition is {}",
                        other.kind()
                    ))),
                },
                ExprKind::Match { value, arms } => {
                    let value = self.expr(value).await?;
                    for arm in arms {
                        if let Some(bindings) = self.match_pattern(&arm.pattern, &value)? {
                            let old = self.env.clone();
                            self.env.extend(bindings);
                            let result = self.expr(&arm.value).await;
                            self.env = old;
                            return result;
                        }
                    }
                    Err(RuntimeError::Type(
                        "non-exhaustive match reached runtime".into(),
                    ))
                }
                ExprKind::Handle { value, arms } => match self.expr(value).await {
                    Ok(value) => Ok(value),
                    Err(error @ RuntimeError::Persistence(_)) => Err(error),
                    Err(error) => {
                        let tagged = Value::Tagged {
                            name: error_name(&error).into(),
                            payload: Some(Box::new(runtime_error_payload(
                                &error,
                                self.limits.value_bytes,
                                self.limits.runtime_depth,
                            ))),
                        };
                        for arm in arms {
                            if let Some(bindings) = self.match_pattern(&arm.pattern, &tagged)? {
                                let old = self.env.clone();
                                let old_error = self.current_error.replace(error.clone());
                                self.env.extend(bindings);
                                let result = self.expr(&arm.value).await;
                                self.env = old;
                                self.current_error = old_error;
                                return result;
                            }
                        }
                        Err(error)
                    }
                },
                ExprKind::Block(forms) => {
                    let old = self.env.clone();
                    let old_constructors = self.constructors.clone();
                    let result: RuntimeResult<Value> = async {
                        let mut value = Value::Null;
                        for form in forms {
                            value = self.form(form, false).await?;
                        }
                        Ok(value)
                    }
                    .await;
                    self.env = old;
                    self.constructors = old_constructors;
                    result
                }
                    }
                }
                .await;
                let value = result?;
                self.ensure_value(&value)?;
                Ok(value)
            }
            .await;
            self.leave_runtime_depth();
            evaluated
        })
    }

    fn apply<'a>(
        &'a mut self,
        function: Value,
        argument: Value,
    ) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            self.enter_runtime_depth()?;
            let applied = async {
                let Value::Callable(callable) = function else {
                    return Err(RuntimeError::Type(format!(
                        "{} is not callable",
                        function.kind()
                    )));
                };
                match callable.as_ref() {
                Callable::Capability { name } => self.perform(name, argument).await,
                Callable::Constructor { name } => Ok(Value::Tagged {
                    name: name.clone(),
                    payload: Some(Box::new(argument)),
                }),
                Callable::Builtin { name, args, arity } => {
                    if name == "par" && args.is_empty() {
                        if matches!(
                            &argument,
                            Value::Callable(callable)
                                if matches!(callable.as_ref(), Callable::Builtin { name, args, .. } if name == "map" && args.is_empty())
                        ) {
                            return Ok(Value::Callable(Rc::new(Callable::Builtin {
                                name: "par_map".into(),
                                args: Vec::new(),
                                arity: 2,
                            })));
                        }
                        return self.parallel(argument).await;
                    }
                    let mut args = args.clone();
                    args.push(argument);
                    if args.len() < *arity {
                        Ok(Value::Callable(Rc::new(Callable::Builtin {
                            name: name.clone(),
                            args,
                            arity: *arity,
                        })))
                    } else {
                        self.builtin(name, args).await
                    }
                }
                Callable::BuiltinDataLast {
                    name,
                    args,
                    data,
                    arity,
                } => {
                    let mut args = args.clone();
                    args.push(argument);
                    if args.len() + 1 < *arity {
                        Ok(Value::Callable(Rc::new(Callable::BuiltinDataLast {
                            name: name.clone(),
                            args,
                            data: data.clone(),
                            arity: *arity,
                        })))
                    } else {
                        args.push(data.clone());
                        self.builtin(name, args).await
                    }
                }
                Callable::User {
                    params,
                    body,
                    captured,
                    args,
                } => {
                    let mut args = args.clone();
                    args.push(argument);
                    if args.len() < params.len() {
                        return Ok(Value::Callable(Rc::new(Callable::User {
                            params: params.clone(),
                            body: body.clone(),
                            captured: captured.clone(),
                            args,
                        })));
                    }
                    let old = std::mem::replace(&mut self.env, captured.as_ref().clone());
                    for (name, value) in old
                        .iter()
                        .filter(|(_, value)| matches!(value, Value::Callable(_)))
                    {
                        self.env
                            .entry(name.clone())
                            .or_insert_with(|| value.clone());
                    }
                    let result: RuntimeResult<Value> = async {
                        for (pattern, value) in params.iter().zip(args) {
                            let Some(bindings) = self.match_pattern(pattern, &value)? else {
                                return Err(RuntimeError::Type(
                                    "function pattern did not match".into(),
                                ));
                            };
                            self.env.extend(bindings);
                        }
                        self.expr(body).await
                    }
                    .await;
                    self.env = old;
                    result
                }
                Callable::Row { body, captured } => {
                    let old = std::mem::replace(&mut self.env, captured.as_ref().clone());
                    if let Value::Record(fields) = &argument {
                        self.env.insert("row".into(), argument.clone());
                        for (name, value) in fields {
                            self.env
                                .entry(name.clone())
                                .or_insert_with(|| value.clone());
                        }
                        if let Some(Value::List(rows)) = fields.get("rows") {
                            self.env
                                .entry("count".into())
                                .or_insert(Value::Int(rows.len() as i64));
                            let mut columns = BTreeMap::<String, Vec<Value>>::new();
                            for row in rows {
                                if let Value::Record(fields) = row {
                                    for (name, value) in fields {
                                        columns
                                            .entry(name.clone())
                                            .or_default()
                                            .push(value.clone());
                                    }
                                }
                            }
                            for (name, values) in columns {
                                self.env.entry(name).or_insert(Value::List(values));
                            }
                        }
                    } else {
                        self.env.insert("row".into(), argument);
                    }
                    let result = self.expr(body).await;
                    self.env = old;
                    result
                    }
                }
            }
            .await;
            self.leave_runtime_depth();
            applied
        })
    }

    fn apply_last<'a>(
        &'a mut self,
        function: Value,
        argument: Value,
    ) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            let Value::Callable(callable) = &function else {
                return self.apply(function, argument).await;
            };
            match callable.as_ref() {
                Callable::Builtin { name, args, arity } if args.len() + 1 < *arity => {
                    Ok(Value::Callable(Rc::new(Callable::BuiltinDataLast {
                        name: name.clone(),
                        args: args.clone(),
                        data: argument,
                        arity: *arity,
                    })))
                }
                _ => self.apply(function, argument).await,
            }
        })
    }

    fn builtin<'a>(
        &'a mut self,
        name: &'a str,
        args: Vec<Value>,
    ) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            let result: RuntimeResult<Value> = async {
                match name {
                    "print" => {
                        let text = render_value_bounded(
                            &args[0],
                            self.limits.value_bytes.min(self.remaining_output()),
                            self.limits.runtime_depth,
                        )?;
                        self.push_stdout(&text)?;
                        Ok(Value::Null)
                    }
                    "display" => {
                        let payload = if self.sensitive_cell {
                            json!({"cellId": self.cell_id, "spec": "[redacted]", "value": "[redacted]"})
                        } else {
                            let remaining = self.remaining_output();
                            let spec_bytes = value_json_bounded(
                                &args[0],
                                remaining,
                                false,
                                self.limits.runtime_depth,
                            )
                                .map(|(bytes, _)| bytes)
                                .ok_or_else(|| {
                                    RuntimeError::Limit(
                                        "display/output budget exceeded".into(),
                                    )
                                })?;
                            let value_bytes = value_json_bounded(
                                &args[1],
                                remaining.saturating_sub(spec_bytes),
                                false,
                                self.limits.runtime_depth,
                            )
                            .map(|(bytes, _)| bytes)
                            .ok_or_else(|| {
                                RuntimeError::Limit("display/output budget exceeded".into())
                            })?;
                            let control_bytes = self.cell_id.len().saturating_add(32);
                            let total = spec_bytes
                                .saturating_add(value_bytes)
                                .saturating_add(control_bytes);
                            if total > remaining {
                                return Err(RuntimeError::Limit(
                                    "display/output budget exceeded".into(),
                                ));
                            }
                            json!({"cellId": self.cell_id, "spec": args[0].to_json(), "value": args[1].to_json()})
                        };
                        let payload_bytes = json_encoded_len_bounded(
                            &payload,
                            self.remaining_output(),
                        )
                        .ok_or_else(|| {
                            RuntimeError::Limit("display/output budget exceeded".into())
                        })?;
                        self.reserve_output(payload_bytes)
                            .map_err(|_| {
                                RuntimeError::Limit("display/output budget exceeded".into())
                            })?;
                        self.emit("display", payload).await?;
                        Ok(args[1].clone())
                    }
                    "length" => match &args[0] {
                        Value::List(v) => Ok(Value::Int(v.len() as i64)),
                        Value::String(v) => Ok(Value::Int(v.chars().count() as i64)),
                        Value::Record(v) => Ok(Value::Int(v.len() as i64)),
                        other => Err(RuntimeError::Type(format!("length on {}", other.kind()))),
                    },
                    "lines" => match &args[0] {
                        Value::String(v) => {
                            let mut result = Vec::new();
                            let mut allocated = 0;
                            let mut iterations = 0;
                            for line in v.lines() {
                                self.cooperate(&mut iterations).await?;
                                let value = Value::String(line.into());
                                self.add_value_size(&mut allocated, &value, 0)?;
                                result.push(value);
                            }
                            Ok(Value::List(result))
                        }
                        other => Err(RuntimeError::Type(format!("lines on {}", other.kind()))),
                    },
                    "split" => match (&args[0], &args[1]) {
                        (Value::String(delimiter), Value::String(value)) => {
                            let mut result = Vec::new();
                            let mut allocated = 0;
                            let mut iterations = 0;
                            for part in value.split(delimiter) {
                                self.cooperate(&mut iterations).await?;
                                let value = Value::String(part.into());
                                self.add_value_size(&mut allocated, &value, 0)?;
                                result.push(value);
                            }
                            Ok(Value::List(result))
                        }
                        _ => Err(RuntimeError::Type("split requires strings".into())),
                    },
                    "contains" => match (&args[0], &args[1]) {
                        (Value::String(needle), Value::String(value)) => {
                            Ok(Value::Bool(value.contains(needle)))
                        }
                        _ => Err(RuntimeError::Type("contains requires strings".into())),
                    },
                    "take" => match (&args[0], &args[1]) {
                        (Value::Int(count), Value::List(values)) => Ok(Value::List(
                            values
                                .iter()
                                .take((*count).max(0) as usize)
                                .cloned()
                                .collect(),
                        )),
                        _ => Err(RuntimeError::Type("take requires count and list".into())),
                    },
                    "merge" => match (&args[0], &args[1]) {
                        (Value::Record(update), Value::Record(base)) => {
                            let mut result = BTreeMap::new();
                            let mut allocated = 0;
                            for (name, value) in base.iter().chain(update) {
                                self.add_value_size(&mut allocated, value, name.len())?;
                                result.insert(name.clone(), value.clone());
                            }
                            Ok(Value::Record(result))
                        }
                        _ => Err(RuntimeError::Type("merge requires records".into())),
                    },
                    "map" => {
                        let (function, values) = list_call_args(args, "map")?;
                        let mut result = Vec::new();
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            let value = self.apply(function.clone(), value).await?;
                            self.add_value_size(&mut allocated, &value, 0)?;
                            result.push(value);
                        }
                        Ok(Value::List(result))
                    }
                    "flatmap" => {
                        let (function, values) = list_call_args(args, "flatmap")?;
                        let mut result = Vec::new();
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            match self.apply(function.clone(), value).await? {
                                Value::List(items) => {
                                    for item in items {
                                        self.cooperate(&mut iterations).await?;
                                        self.add_value_size(&mut allocated, &item, 0)?;
                                        result.push(item);
                                    }
                                }
                                other => {
                                    return Err(RuntimeError::Type(format!(
                                        "flatmap returned {}",
                                        other.kind()
                                    )));
                                }
                            }
                        }
                        Ok(Value::List(result))
                    }
                    "filter" | "where" => {
                        let (function, values) = list_call_args(args, name)?;
                        let mut result = Vec::new();
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            if matches!(
                                self.apply(function.clone(), value.clone()).await?,
                                Value::Bool(true)
                            ) {
                                self.add_value_size(&mut allocated, &value, 0)?;
                                result.push(value);
                            }
                        }
                        Ok(Value::List(result))
                    }
                    "select" | "aggregate" => {
                        let (function, values) = list_call_args(args, name)?;
                        let mut result = Vec::with_capacity(values.len());
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            let value = self.apply(function.clone(), value).await?;
                            self.add_value_size(&mut allocated, &value, 0)?;
                            result.push(value);
                        }
                        Ok(Value::List(result))
                    }
                    "sort_by" => {
                        let function = args[0].clone();
                        let descending =
                            matches!(&args[1], Value::String(value) if value == "desc");
                        let Value::List(values) = &args[2] else {
                            return Err(RuntimeError::Type("sort_by requires a list".into()));
                        };
                        let mut keyed = Vec::with_capacity(values.len());
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            let key = self.apply(function.clone(), value.clone()).await?;
                            self.add_value_size(&mut allocated, &key, 0)?;
                            self.add_value_size(&mut allocated, value, 0)?;
                            keyed.push((key, value.clone()));
                        }
                        let sort_work = (keyed.len().max(1).ilog2() as u64 + 1)
                            .saturating_mul(keyed.len() as u64);
                        self.charge_work(sort_work)?;
                        tokio::task::yield_now().await;
                        keyed.sort_by(|left, right| compare_sort_keys(&left.0, &right.0));
                        if descending {
                            keyed.reverse();
                        }
                        Ok(Value::List(
                            keyed.into_iter().map(|(_, value)| value).collect(),
                        ))
                    }
                    "group_by" => {
                        let (function, values) = list_call_args(args, "group_by")?;
                        let mut groups = BTreeMap::<String, (Value, Vec<Value>)>::new();
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            let key = self.apply(function.clone(), value.clone()).await?;
                            let group = group_key(
                                &key,
                                self.limits.value_bytes.saturating_sub(allocated),
                                self.limits.runtime_depth,
                            )?;
                            if !groups.contains_key(&group) {
                                self.add_value_size(&mut allocated, &key, group.len())?;
                            }
                            self.add_value_size(&mut allocated, &value, 0)?;
                            groups
                                .entry(group)
                                .or_insert_with(|| (key, Vec::new()))
                                .1
                                .push(value);
                        }
                        Ok(Value::List(
                            groups
                                .into_values()
                                .map(|(key, rows)| {
                                    Value::Record(BTreeMap::from([
                                        ("key".into(), key),
                                        ("rows".into(), Value::List(rows)),
                                    ]))
                                })
                                .collect(),
                        ))
                    }
                    "sum" => match &args[0] {
                        Value::List(values) => {
                            self.charge_work((values.len() as u64).saturating_mul(2))?;
                            tokio::task::yield_now().await;
                            if values.iter().all(|value| matches!(value, Value::Int(_))) {
                                let total = values.iter().try_fold(0_i64, |total, value| {
                                    let Value::Int(value) = value else {
                                        unreachable!("all values were checked as integers")
                                    };
                                    total.checked_add(*value).ok_or_else(|| {
                                        RuntimeError::Type("integer overflow in sum".into())
                                    })
                                })?;
                                return Ok(Value::Int(total));
                            }

                            let mut total = 0.0_f64;
                            for value in values {
                                match value {
                                    Value::Int(value) => total += *value as f64,
                                    Value::Decimal(value) => total += value,
                                    other => {
                                        return Err(RuntimeError::Type(format!(
                                            "sum on {}",
                                            other.kind()
                                        )));
                                    }
                                }
                            }
                            Ok(Value::Decimal(total))
                        }
                        other => Err(RuntimeError::Type(format!("sum on {}", other.kind()))),
                    },
                    "table" => Ok(args[0].clone()),
                    "par_map" => self.parallel_map(args).await,
                    "par" => self.parallel(args[0].clone()).await,
                    "help" => match &args[0] {
                        Value::Callable(callable) => match callable.as_ref() {
                            Callable::Capability { name } => {
                                let docs = self
                                    .registry
                                    .docs(name, &self.invocation)
                                    .map_err(host_error)?;
                                let docs = serde_json::to_value(docs).map_err(|error| {
                                    RuntimeError::Type(format!(
                                        "failed to encode capability docs: {error}"
                                    ))
                                })?;
                                if json_value_size_bounded(
                                    &docs,
                                    self.limits.value_bytes,
                                    self.limits.runtime_depth,
                                )
                                    > self.limits.value_bytes
                                {
                                    return Err(RuntimeError::Limit(
                                        "capability docs value budget exceeded".into(),
                                    ));
                                }
                                Ok(Value::from_json(docs))
                            }
                            _ => Ok(Value::String(render_value_bounded(
                                &args[0],
                                self.limits.value_bytes,
                                self.limits.runtime_depth,
                            )?)),
                        },
                        _ => Ok(Value::String(render_value_bounded(
                            &args[0],
                            self.limits.value_bytes,
                            self.limits.runtime_depth,
                        )?)),
                    },
                    "rethrow" => Err(self
                        .current_error
                        .clone()
                        .unwrap_or_else(|| {
                            rethrow_error(
                                &args[0],
                                self.limits.preview_bytes,
                                self.limits.runtime_depth,
                            )
                        })),
                    other => Err(RuntimeError::Type(format!(
                        "unsupported prelude function {other}"
                    ))),
                }
            }
            .await;
            let value = result?;
            self.ensure_value(&value)?;
            Ok(value)
        })
    }

    async fn parallel_expr(&mut self, expr: &Expr) -> RuntimeResult<Value> {
        let total = match &expr.node {
            ExprKind::Record(fields) => fields.len(),
            _ => 1,
        };
        let node = self.start_scope("par", Some(total)).await?;
        let result = if let ExprKind::Record(fields) = &expr.node {
            let work = fields
                .iter()
                .map(|(name, expr)| (name.clone(), expr.clone()))
                .collect::<Vec<_>>();
            self.run_parallel(work, &node, |mut child, expr| async move {
                let result = child.expr(&expr).await;
                (result, child)
            })
            .await
            .map(|values| Value::Record(values.into_iter().collect()))
            .inspect(|_| {
                debug_assert_eq!(total, fields.len());
            })
        } else {
            let mut child = self.fork_child(&node);
            child.expr(expr).await
        };
        let cancelled = if result.is_err() {
            self.cancel_scope_effects(&node, "parallel sibling failed")
                .await?
        } else {
            0
        };
        self.finish_scope(&node, result.as_ref().err(), cancelled)
            .await?;
        result
    }

    async fn parallel(&mut self, value: Value) -> RuntimeResult<Value> {
        let node = self.start_scope("par", None).await?;
        self.finish_scope(&node, None, 0).await?;
        Ok(value)
    }

    async fn parallel_map(&mut self, args: Vec<Value>) -> RuntimeResult<Value> {
        let (function, values) = list_call_args(args, "par map")?;
        let total = values.len();
        let node = self.start_scope("par_map", Some(total)).await?;
        let work = values.into_iter().enumerate().collect::<Vec<_>>();
        let result = self
            .run_parallel(work, &node, move |mut child, value| {
                let function = function.clone();
                async move {
                    let result = child.apply(function, value).await;
                    (result, child)
                }
            })
            .await
            .map(|values| Value::List(values.into_iter().map(|(_, value)| value).collect()));
        let cancelled = if result.is_err() {
            self.cancel_scope_effects(&node, "parallel sibling failed")
                .await?
        } else {
            0
        };
        self.finish_scope(&node, result.as_ref().err(), cancelled)
            .await?;
        result
    }

    async fn run_parallel<K, T, F, Fut>(
        &mut self,
        work: Vec<(K, T)>,
        scope: &str,
        mut evaluate: F,
    ) -> RuntimeResult<Vec<(K, Value)>>
    where
        K: Clone + Ord + 'static,
        F: FnMut(Evaluator, T) -> Fut,
        Fut: std::future::Future<Output = (RuntimeResult<Value>, Evaluator)> + 'static,
    {
        let total = work.len();
        if total == 0 {
            return Ok(Vec::new());
        }
        let parallelism = self.limits.parallelism.max(1).min(total);
        let mut remaining = work.into_iter();
        let mut pending: FuturesUnordered<
            LocalBoxFuture<'static, (K, RuntimeResult<Value>, Evaluator)>,
        > = FuturesUnordered::new();
        for _ in 0..parallelism {
            let Some((key, item)) = remaining.next() else {
                break;
            };
            let child = self.fork_child(scope);
            let future = evaluate(child, item);
            pending.push(Box::pin(async move {
                let (result, child) = future.await;
                (key, result, child)
            }));
        }
        let mut completed = 0usize;
        let mut output = Vec::with_capacity(total);
        let mut allocated = 0;
        while let Some((key, result, _child)) = pending.next().await {
            match result {
                Ok(value) => {
                    completed += 1;
                    self.add_value_size(&mut allocated, &value, 0)?;
                    output.push((key, value));
                    if let Some(active) = self
                        .active
                        .lock()
                        .expect("active execution lock poisoned")
                        .as_mut()
                        && let Some(active_scope) = active.scopes.get_mut(scope)
                    {
                        active_scope.completed = completed;
                    }
                    self.emit(
                        "scope_progress",
                        json!({"cellId": self.cell_id, "nodeId": scope, "parentNodeId": self.scope_id, "completed": completed, "total": total}),
                    )
                    .await?;
                    if let Some((next_key, next_item)) = remaining.next() {
                        let child = self.fork_child(scope);
                        let future = evaluate(child, next_item);
                        pending.push(Box::pin(async move {
                            let (result, child) = future.await;
                            (next_key, result, child)
                        }));
                    }
                }
                Err(error) => {
                    drop(pending);
                    return Err(error);
                }
            }
        }
        output.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(output)
    }

    fn fork_child(&self, scope: &str) -> Self {
        let mut child = self.clone();
        child.committed.clear();
        child.scope_id = Some(scope.to_string());
        child
    }

    async fn start_scope(&mut self, kind: &str, total: Option<usize>) -> RuntimeResult<String> {
        let index = self.node_counter.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        let node = format!("{}-scope-{index}", self.cell_id);
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
        {
            active.scopes.insert(
                node.clone(),
                ActiveScope {
                    total,
                    completed: 0,
                    parent_node_id: self.scope_id.clone(),
                    terminal: None,
                },
            );
        }
        if let Err(error) = self
            .emit(
                "scope_start",
                json!({"cellId": self.cell_id, "nodeId": node, "parentNodeId": self.scope_id, "kind": kind, "total": total}),
            )
            .await
        {
            if let Some(active) = self
                .active
                .lock()
                .expect("active execution lock poisoned")
                .as_mut()
            {
                active.scopes.remove(&node);
            }
            return Err(error);
        }
        Ok(node)
    }

    async fn finish_scope(
        &self,
        node: &str,
        error: Option<&RuntimeError>,
        cancelled_siblings: usize,
    ) -> RuntimeResult<()> {
        let payload = if let Some(error) = error {
            let error_preview = if self.sensitive_cell {
                "[redacted]".to_string()
            } else {
                bounded_display(error, self.limits.preview_bytes)
            };
            if self.reserve_preview(&error_preview).is_ok() {
                json!({"cellId": self.cell_id, "nodeId": node, "parentNodeId": self.scope_id, "status": "failed", "error": error_preview, "cancelledSiblings": cancelled_siblings})
            } else {
                json!({"cellId": self.cell_id, "nodeId": node, "parentNodeId": self.scope_id, "status": "failed", "errorTruncated": true, "cancelledSiblings": cancelled_siblings})
            }
        } else {
            json!({"cellId": self.cell_id, "nodeId": node, "parentNodeId": self.scope_id, "status": "completed"})
        };
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
            && let Some(scope) = active.scopes.get_mut(node)
        {
            scope.terminal = Some(payload.clone());
        }
        self.emit("scope_result", payload).await?;
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
        {
            active.scopes.remove(node);
        }
        Ok(())
    }

    async fn cancel_scope_effects(&self, scope: &str, _reason: &str) -> RuntimeResult<usize> {
        let (effects, mut descendant_scopes, cancelled_siblings) = {
            let mut active = self.active.lock().expect("active execution lock poisoned");
            let Some(active) = active.as_mut() else {
                return Ok(0);
            };
            let cancelled_siblings = active
                .scopes
                .get(scope)
                .and_then(|scope| {
                    scope
                        .total
                        .map(|total| total.saturating_sub(scope.completed).saturating_sub(1))
                })
                .unwrap_or(0);

            // A dropped outer branch can leave a nested `par` future suspended. Discover the
            // complete scope subtree before removing anything so every child effect and scope is
            // terminalized before the failed parent.
            let mut depths = BTreeMap::from([(scope.to_string(), 0usize)]);
            loop {
                let mut changed = false;
                for (node, child) in &active.scopes {
                    if depths.contains_key(node) {
                        continue;
                    }
                    let Some(parent) = child.parent_node_id.as_ref() else {
                        continue;
                    };
                    let Some(parent_depth) = depths.get(parent).copied() else {
                        continue;
                    };
                    depths.insert(node.clone(), parent_depth.saturating_add(1));
                    changed = true;
                }
                if !changed {
                    break;
                }
            }

            let effects = active
                .effects
                .iter()
                .filter(|(_, effect)| {
                    effect
                        .scope_id
                        .as_ref()
                        .is_some_and(|scope| depths.contains_key(scope))
                })
                .map(|(node, effect)| (node.clone(), effect.clone()))
                .collect::<Vec<_>>();
            for (node, _) in &effects {
                active.effects.remove(node);
            }

            let mut descendant_scopes = depths
                .into_iter()
                .filter(|(node, _)| node != scope)
                .filter_map(|(node, depth)| {
                    active
                        .scopes
                        .remove(&node)
                        .map(|active_scope| (node, active_scope, depth))
                })
                .collect::<Vec<_>>();
            descendant_scopes
                .sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));
            (effects, descendant_scopes, cancelled_siblings)
        };
        for (node_id, effect) in &effects {
            let payload = if let Some(terminal) = &effect.terminal {
                terminal.clone()
            } else {
                let _ = effect
                    .machine
                    .lock()
                    .expect("effect machine lock poisoned")
                    .cancel();
                json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": effect.scope_id, "status": "cancelled", "error": "[redacted]"})
            };
            self.emit("effect_result", payload).await?;
        }
        for (node_id, child_scope, _) in descendant_scopes.drain(..) {
            let ActiveScope {
                parent_node_id,
                terminal,
                ..
            } = child_scope;
            let payload = terminal.unwrap_or_else(|| {
                json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": parent_node_id, "status": "cancelled", "error": "[redacted]"})
            });
            self.emit("scope_result", payload).await?;
        }
        Ok(cancelled_siblings)
    }

    async fn perform(&mut self, name: &str, argument: Value) -> RuntimeResult<Value> {
        let node_index = self.node_counter.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        let node_id = format!("{}-node-{node_index}", self.cell_id);
        let args = normalize_effect_args(name, argument);
        if json_value_size_bounded(&args, self.limits.value_bytes, self.limits.runtime_depth)
            > self.limits.value_bytes
            || json_encoded_len_bounded(&args, self.limits.value_bytes).is_none()
        {
            return Err(RuntimeError::Limit(
                "effect argument value budget exceeded".into(),
            ));
        }
        let sensitive = self.catalog.effect(name).is_some();
        let preview = if sensitive {
            "[redacted]".to_string()
        } else {
            json_preview_bounded(&args, self.limits.preview_bytes)
        };
        self.reserve_preview(&preview)
            .map_err(|_| RuntimeError::Limit("effect/output budget exceeded".into()))?;
        let mut ctx = self.invocation.clone();
        let machine = Arc::new(Mutex::new(EffectMachine::new(
            self.cell_id.clone(),
            node_id.clone(),
            ctx.session_id.clone(),
        )));
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
        {
            active.effects.insert(
                node_id.clone(),
                ActiveEffect {
                    scope_id: self.scope_id.clone(),
                    machine: Arc::clone(&machine),
                    terminal: None,
                },
            );
        }
        if let Err(error) = self.emit("effect_start", json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "capability": name, "argsPreview": preview})).await {
            self.clear_active_effect(&node_id);
            return Err(error);
        }
        let approval_event_failure = Arc::new(Mutex::new(None));
        ctx.approvals = Arc::new(TracingApproval {
            inner: Arc::clone(&ctx.approvals),
            events: Arc::clone(&ctx.events),
            cell_id: self.cell_id.clone(),
            node_id: node_id.clone(),
            machine: Arc::clone(&machine),
            sensitive,
            preview_bytes: self.limits.preview_bytes,
            output_used: Arc::clone(&self.output_used),
            output_bytes: self.output_bytes,
            parent_node_id: self.scope_id.clone(),
            event_failure: Arc::clone(&approval_event_failure),
        });
        let invocation = self.registry.invoke(name, args, &ctx).await;
        let approval_event_error = approval_event_failure
            .lock()
            .expect("event failure lock poisoned")
            .take();
        if let Some(error) = approval_event_error {
            let _ = machine.lock().expect("effect machine lock poisoned").fail();
            return Err(RuntimeError::Persistence(error));
        }
        match invocation {
            Ok(value) => {
                if json_value_size_bounded(
                    &value,
                    self.limits.value_bytes,
                    self.limits.runtime_depth,
                ) > self.limits.value_bytes
                    || json_encoded_len_bounded(&value, self.limits.value_bytes).is_none()
                {
                    let _ = machine.lock().expect("effect machine lock poisoned").fail();
                    self.emit_effect_terminal(&node_id, json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "status": "failed", "error": "[redacted]"})).await?;
                    return Err(RuntimeError::Limit(
                        "effect result value budget exceeded".into(),
                    ));
                }
                let result_preview = if sensitive {
                    "[redacted]".to_string()
                } else {
                    json_preview_bounded(&value, self.limits.preview_bytes)
                };
                if self.reserve_preview(&result_preview).is_err() {
                    let _ = machine.lock().expect("effect machine lock poisoned").fail();
                    self.emit_effect_terminal(&node_id, json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "status": "failed", "error": "[redacted]"})).await?;
                    return Err(RuntimeError::Limit("effect/output budget exceeded".into()));
                }
                machine
                    .lock()
                    .expect("effect machine lock poisoned")
                    .complete()
                    .map_err(|error| RuntimeError::Effect {
                        name: "EffectStateError".into(),
                        message: error.to_string(),
                        payload: None,
                    })?;
                self.emit_effect_terminal(&node_id, json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "status": "completed", "resultPreview": result_preview})).await?;
                let value = Value::from_json(value);
                self.ensure_value(&value)?;
                Ok(value)
            }
            Err(error) => {
                let _ = machine.lock().expect("effect machine lock poisoned").fail();
                let error_preview = if sensitive {
                    "[redacted]".to_string()
                } else {
                    bounded_display(&error, self.limits.preview_bytes)
                };
                if self.reserve_preview(&error_preview).is_err() {
                    self.emit_effect_terminal(&node_id, json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "status": "failed", "error": "[redacted]"})).await?;
                    return Err(RuntimeError::Limit("effect/output budget exceeded".into()));
                }
                self.emit_effect_terminal(&node_id, json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "status": "failed", "error": error_preview})).await?;
                Err(host_error(error))
            }
        }
    }

    fn clear_active_effect(&self, node_id: &str) {
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
        {
            active.effects.remove(node_id);
        }
    }

    async fn emit_effect_terminal(&self, node_id: &str, payload: JsonValue) -> RuntimeResult<()> {
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
            && let Some(effect) = active.effects.get_mut(node_id)
        {
            effect.terminal = Some(payload.clone());
        }
        self.emit("effect_result", payload).await?;
        self.clear_active_effect(node_id);
        Ok(())
    }

    fn interpolate<'a>(&'a mut self, source: &'a str) -> LocalBoxFuture<'a, RuntimeResult<String>> {
        Box::pin(async move {
            let mut output = String::new();
            let chars: Vec<char> = source.chars().collect();
            let mut index = 0;
            while index < chars.len() {
                if chars[index] == '\\' && chars.get(index + 1) == Some(&'#') {
                    output.push('#');
                    index += 2;
                    continue;
                }
                if chars[index] != '#' {
                    output.push(chars[index]);
                    index += 1;
                    continue;
                }
                if chars.get(index + 1) == Some(&'{') {
                    let start = index + 2;
                    let mut depth = 1;
                    let mut end = start;
                    while end < chars.len() && depth > 0 {
                        match chars[end] {
                            '{' => depth += 1,
                            '}' => depth -= 1,
                            _ => {}
                        }
                        if depth > 0 {
                            end += 1;
                        }
                    }
                    if depth != 0 {
                        return Err(RuntimeError::Type(
                            "unterminated string interpolation".into(),
                        ));
                    }
                    let fragment: String = chars[start..end].iter().collect();
                    let cell = parse_bounded(
                        &fragment,
                        self.limits.source_bytes,
                        self.limits.syntax_nodes,
                        self.limits.parse_depth,
                    )?;
                    let [
                        Form {
                            node: FormKind::Expr(expr),
                            ..
                        },
                    ] = cell.forms.as_slice()
                    else {
                        return Err(RuntimeError::Type(
                            "interpolation must contain one expression".into(),
                        ));
                    };
                    let value = self.expr(expr).await?;
                    let rendered = render_value_bounded(
                        &value,
                        self.limits.value_bytes.saturating_sub(output.len()),
                        self.limits.runtime_depth,
                    )?;
                    self.push_interpolation(&mut output, &rendered)?;
                    index = end + 1;
                    continue;
                }
                let start = index + 1;
                let mut end = start;
                while end < chars.len() && (chars[end] == '_' || chars[end].is_alphanumeric()) {
                    end += 1;
                }
                if end == start {
                    output.push('#');
                    index += 1;
                    continue;
                }
                let name: String = chars[start..end].iter().collect();
                let value = self.env.get(&name).ok_or_else(|| {
                    RuntimeError::Type(format!("unbound interpolation name {name}"))
                })?;
                let rendered = render_value_bounded(
                    value,
                    self.limits.value_bytes.saturating_sub(output.len()),
                    self.limits.runtime_depth,
                )?;
                self.push_interpolation(&mut output, &rendered)?;
                index = end;
            }
            Ok(output)
        })
    }

    async fn emit(&self, event: &str, payload: JsonValue) -> RuntimeResult<()> {
        self.invocation
            .events
            .emit(event, payload)
            .await
            .map_err(runtime_event_error)
    }

    async fn emit_cell_terminal(&self, payload: JsonValue) -> RuntimeResult<()> {
        self.terminal_selected.store(true, AtomicOrdering::Release);
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
        {
            active.cell_terminal = Some(payload.clone());
        }
        let result = self.emit("cell_result", payload).await;
        *self.active.lock().expect("active execution lock poisoned") = None;
        result
    }

    async fn terminal_error(&self, payload: JsonValue, error: RuntimeError) -> RuntimeError {
        match self.emit_cell_terminal(payload).await {
            Ok(()) => error,
            Err(persistence) => persistence,
        }
    }
    fn step(&mut self, _span: Span) -> RuntimeResult<()> {
        self.charge_work(1)
    }

    fn enter_runtime_depth(&mut self) -> RuntimeResult<()> {
        if self.depth >= self.limits.runtime_depth {
            return Err(RuntimeError::Limit(
                "runtime nesting budget exceeded".into(),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave_runtime_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn charge_work(&self, units: u64) -> RuntimeResult<()> {
        self.steps
            .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |used| {
                used.checked_add(units)
                    .filter(|next| *next <= self.limits.steps)
            })
            .map(|_| ())
            .map_err(|_| RuntimeError::Limit("step budget exceeded".into()))
    }

    fn values_equal(&self, left: &Value, right: &Value) -> RuntimeResult<bool> {
        let remaining = self.limits.steps.saturating_sub(
            self.steps
                .load(AtomicOrdering::Relaxed)
                .min(self.limits.steps),
        );
        let Some((equal, visits)) = json_semantic_eq_bounded(left, right, remaining as usize)
        else {
            return Err(RuntimeError::Limit("step budget exceeded".into()));
        };
        self.charge_work(visits as u64)?;
        Ok(equal)
    }

    fn match_pattern(
        &self,
        pattern: &Pattern,
        value: &Value,
    ) -> RuntimeResult<Option<Environment>> {
        let remaining = self.limits.steps.saturating_sub(
            self.steps
                .load(AtomicOrdering::Relaxed)
                .min(self.limits.steps),
        );
        let (bindings, visits) = match_pattern_counted(
            pattern,
            value,
            remaining as usize,
            self.limits.runtime_depth,
        );
        if visits > remaining as usize {
            return Err(RuntimeError::Limit("step budget exceeded".into()));
        }
        self.charge_work(visits as u64)?;
        Ok(bindings)
    }

    async fn cooperate(&self, iterations: &mut u64) -> RuntimeResult<()> {
        self.charge_work(1)?;
        *iterations = iterations.saturating_add(1);
        if (*iterations).is_multiple_of(256) {
            tokio::task::yield_now().await;
        }
        Ok(())
    }
    fn push_stdout(&mut self, text: &str) -> RuntimeResult<()> {
        let mut stdout = self.stdout.lock().expect("stdout lock poisoned");
        let bytes = text.len().saturating_add(1);
        let next = stdout.len().saturating_add(bytes);
        if next > self.limits.print_bytes || self.reserve_output(bytes).is_err() {
            return Err(RuntimeError::Limit("print/output budget exceeded".into()));
        }
        stdout.push_str(text);
        stdout.push('\n');
        Ok(())
    }

    fn reserve_output(&self, bytes: usize) -> RuntimeResult<()> {
        self.output_used
            .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= self.output_bytes)
            })
            .map(|_| ())
            .map_err(|_| RuntimeError::Limit("output budget exceeded".into()))
    }

    fn reserve_preview(&self, preview: &str) -> RuntimeResult<()> {
        let bytes = json_string_encoded_len_bounded(preview, self.remaining_output())
            .ok_or_else(|| RuntimeError::Limit("output budget exceeded".into()))?;
        self.reserve_output(bytes)
    }

    fn remaining_output(&self) -> usize {
        self.output_bytes.saturating_sub(
            self.output_used
                .load(AtomicOrdering::Relaxed)
                .min(self.output_bytes),
        )
    }

    fn ensure_value(&self, value: &Value) -> RuntimeResult<()> {
        let size = value_size_bounded(value, self.limits.value_bytes, self.limits.runtime_depth);
        if size > self.limits.value_bytes {
            Err(RuntimeError::Limit(
                "intermediate value budget exceeded".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn ensure_capture(&self, binding_name_bytes: usize) -> RuntimeResult<()> {
        let limit = self.limits.environment_bytes;
        let retained = environment_size_bounded(&self.env, limit, self.limits.runtime_depth);
        if retained > limit {
            return Err(RuntimeError::Limit(
                "persistent environment budget exceeded".into(),
            ));
        }
        let remaining = limit.saturating_sub(retained);
        let clone_cost =
            environment_clone_size_bounded(&self.env, remaining, self.limits.runtime_depth);
        let projected = retained
            .saturating_add(clone_cost)
            .saturating_add(binding_name_bytes)
            .saturating_add(std::mem::size_of::<Callable>())
            .saturating_add(std::mem::size_of::<Value>());
        if projected > limit {
            Err(RuntimeError::Limit(
                "persistent environment budget exceeded".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn add_value_size(
        &self,
        allocated: &mut usize,
        value: &Value,
        metadata_bytes: usize,
    ) -> RuntimeResult<()> {
        let remaining = self.limits.value_bytes.saturating_sub(*allocated);
        let value_bytes = value_size_bounded(value, remaining, self.limits.runtime_depth);
        let next = allocated
            .saturating_add(metadata_bytes)
            .saturating_add(value_bytes);
        if next > self.limits.value_bytes {
            return Err(RuntimeError::Limit(
                "intermediate value budget exceeded".into(),
            ));
        }
        *allocated = next;
        Ok(())
    }

    fn push_interpolation(&self, output: &mut String, value: &str) -> RuntimeResult<()> {
        if output.len().saturating_add(value.len()) > self.limits.value_bytes {
            return Err(RuntimeError::Limit(
                "intermediate value budget exceeded".into(),
            ));
        }
        output.push_str(value);
        Ok(())
    }
}

struct TracingApproval {
    inner: Arc<dyn ApprovalPolicy>,
    events: Arc<dyn HostEventSink>,
    cell_id: String,
    node_id: String,
    machine: Arc<Mutex<EffectMachine>>,
    sensitive: bool,
    preview_bytes: usize,
    output_used: Arc<AtomicUsize>,
    output_bytes: usize,
    parent_node_id: Option<String>,
    event_failure: Arc<Mutex<Option<String>>>,
}

impl TracingApproval {
    async fn emit_runtime_event(&self, event: &str, payload: JsonValue) -> tm_host::Result<()> {
        match self.events.emit(event, payload).await {
            Ok(()) => Ok(()),
            Err(error) => {
                *self
                    .event_failure
                    .lock()
                    .expect("event failure lock poisoned") =
                    Some(bounded_display(&error, 64 * 1024));
                Err(error)
            }
        }
    }
}

#[async_trait::async_trait]
impl ApprovalPolicy for TracingApproval {
    async fn request(
        &self,
        action: &str,
        timeout: std::time::Duration,
    ) -> tm_host::Result<ApprovalDecision> {
        let token = self
            .machine
            .lock()
            .expect("effect machine lock poisoned")
            .suspend()
            .map_err(|error| HostError::HostCall(error.to_string()))?;
        let action_preview = if self.sensitive {
            "[redacted]".to_string()
        } else {
            bounded(action, self.preview_bytes)
        };
        let remaining = self.output_bytes.saturating_sub(
            self.output_used
                .load(AtomicOrdering::Relaxed)
                .min(self.output_bytes),
        );
        let encoded_bytes = json_string_encoded_len_bounded(&action_preview, remaining)
            .ok_or_else(|| HostError::HostCall("effect/output budget exceeded".into()))?;
        self.output_used
            .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |used| {
                used.checked_add(encoded_bytes)
                    .filter(|next| *next <= self.output_bytes)
            })
            .map_err(|_| HostError::HostCall("effect/output budget exceeded".into()))?;
        self.emit_runtime_event(
                "effect_suspended",
                json!({"cellId": self.cell_id, "nodeId": self.node_id, "parentNodeId": self.parent_node_id, "action": action_preview}),
            )
            .await?;
        let decision = self.inner.request(action, timeout).await;
        match decision {
            Ok(decision) => {
                self.machine
                    .lock()
                    .expect("effect machine lock poisoned")
                    .resume(&token)
                    .map_err(|error| HostError::HostCall(error.to_string()))?;
                let decision_name = match decision {
                    ApprovalDecision::Approved => "approved",
                    ApprovalDecision::Denied => "denied",
                };
                self.emit_runtime_event("effect_resumed", json!({"cellId": self.cell_id, "nodeId": self.node_id, "parentNodeId": self.parent_node_id, "decision": decision_name})).await?;
                Ok(decision)
            }
            Err(error) => {
                self.machine
                    .lock()
                    .expect("effect machine lock poisoned")
                    .fail()
                    .map_err(|error| HostError::HostCall(error.to_string()))?;
                Err(error)
            }
        }
    }
}

fn host_error(error: HostError) -> RuntimeError {
    const MAX_HOST_ERROR_BYTES: usize = 64 * 1024;
    let retained_bytes = match &error {
        HostError::UnknownScheme { scheme, registered } => {
            registered.iter().fold(scheme.len(), |bytes, value| {
                bytes.saturating_add(value.len())
            })
        }
        HostError::CapabilityDenied(value)
        | HostError::ApprovalDenied(value)
        | HostError::ApprovalTimeout(value)
        | HostError::NotFound(value)
        | HostError::InvalidArgs(value)
        | HostError::InvalidPath(value)
        | HostError::NotImplemented(value)
        | HostError::QuotaExceeded(value)
        | HostError::Timeout(value)
        | HostError::OutputTruncated(value)
        | HostError::HostCall(value) => value.len(),
    };
    let message = bounded_display(&error, MAX_HOST_ERROR_BYTES);
    let payload = if retained_bytes <= MAX_HOST_ERROR_BYTES {
        serde_json::to_value(error.to_payload())
            .expect("HostErrorPayload serialization cannot fail")
    } else {
        json!({"name": error.sdk_name(), "message": message.clone(), "detailsTruncated": true})
    };
    RuntimeError::Effect {
        name: error.sdk_name().into(),
        message,
        payload: Some(payload),
    }
}

fn runtime_event_error(error: HostError) -> RuntimeError {
    RuntimeError::Persistence(bounded_display(&error, 64 * 1024))
}

fn runtime_error_payload(error: &RuntimeError, max_bytes: usize, max_depth: usize) -> Value {
    match error {
        RuntimeError::Effect {
            payload: Some(payload),
            ..
        } if json_value_size_bounded(payload, max_bytes, max_depth) <= max_bytes
            && json_encoded_len_bounded(payload, max_bytes).is_some() =>
        {
            Value::from_json(payload.clone())
        }
        _ => Value::Record(BTreeMap::from([(
            "message".into(),
            Value::String(bounded_display(error, max_bytes)),
        )])),
    }
}

fn rethrow_error(value: &Value, preview_bytes: usize, max_depth: usize) -> RuntimeError {
    let Value::Tagged { name, payload } = value else {
        return RuntimeError::Effect {
            name: "Rethrown".into(),
            message: render_value_bounded(value, preview_bytes, max_depth)
                .unwrap_or_else(|_| "[rethrow payload exceeded budget]".into()),
            payload: None,
        };
    };
    let payload_value = payload.as_deref();
    let message = match payload_value {
        Some(Value::Record(fields)) => fields
            .get("message")
            .and_then(|value| match value {
                Value::String(message) => Some(bounded(message, preview_bytes)),
                _ => None,
            })
            .unwrap_or_else(|| {
                payload_value
                    .and_then(|payload| {
                        render_value_bounded(payload, preview_bytes, max_depth).ok()
                    })
                    .unwrap_or_else(|| "[rethrow payload exceeded budget]".into())
            }),
        Some(payload) => render_value_bounded(payload, preview_bytes, max_depth)
            .unwrap_or_else(|_| "[rethrow payload exceeded budget]".into()),
        None => bounded(name, preview_bytes),
    };
    let payload = payload_value.and_then(|payload| {
        (value_size_bounded(payload, preview_bytes, max_depth) <= preview_bytes)
            .then(|| value_json_bounded(payload, preview_bytes, false, max_depth))
            .flatten()
            .map(|_| payload.to_json())
    });
    RuntimeError::Effect {
        name: name.clone(),
        message,
        payload,
    }
}

fn bounded(value: &str, bytes: usize) -> String {
    if value.len() <= bytes {
        value.into()
    } else {
        let marker = if bytes >= 3 { "..." } else { "" };
        let mut end = bytes.saturating_sub(marker.len()).min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}{marker}", &value[..end])
    }
}

struct BoundedDisplay {
    text: String,
    max_bytes: usize,
}

impl std::fmt::Write for BoundedDisplay {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        let remaining = self.max_bytes.saturating_sub(self.text.len());
        if value.len() <= remaining {
            self.text.push_str(value);
            return Ok(());
        }
        let mut end = remaining.min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&value[..end]);
        Err(std::fmt::Error)
    }
}

fn bounded_display(value: &impl std::fmt::Display, max_bytes: usize) -> String {
    use std::fmt::Write as _;

    let mut output = BoundedDisplay {
        text: String::with_capacity(max_bytes.min(4096)),
        max_bytes,
    };
    let _ = write!(&mut output, "{value}");
    output.text
}

struct BudgetWriter {
    bytes: Vec<u8>,
    written: usize,
    limit: usize,
    exceeded: bool,
}

impl BudgetWriter {
    fn new(limit: usize, retain: bool) -> Self {
        Self {
            bytes: if retain {
                Vec::with_capacity(limit.min(4096))
            } else {
                Vec::new()
            },
            written: 0,
            limit,
            exceeded: false,
        }
    }

    fn finish(self) -> (usize, String, bool) {
        let Self {
            mut bytes,
            written,
            exceeded,
            ..
        } = self;
        let valid_bytes = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(error) => error.valid_up_to(),
        };
        bytes.truncate(valid_bytes);
        (
            written,
            String::from_utf8(bytes).expect("validated UTF-8 prefix"),
            exceeded,
        )
    }
}

impl Write for BudgetWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.written);
        if !self.bytes.is_empty() || self.bytes.capacity() > 0 {
            self.bytes
                .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        }
        self.written = self.written.saturating_add(bytes.len());
        if bytes.len() > remaining {
            self.exceeded = true;
            return Err(io::Error::other("encoded value budget exceeded"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn json_encoded_len_bounded(value: &JsonValue, limit: usize) -> Option<usize> {
    let mut writer = BudgetWriter::new(limit, false);
    let result = serde_json::to_writer(&mut writer, value);
    let (written, _, exceeded) = writer.finish();
    (result.is_ok() && !exceeded).then_some(written)
}

fn json_string_encoded_len_bounded(value: &str, limit: usize) -> Option<usize> {
    let mut writer = BudgetWriter::new(limit, false);
    let result = serde_json::to_writer(&mut writer, value);
    let (written, _, exceeded) = writer.finish();
    (result.is_ok() && !exceeded).then_some(written)
}

fn json_preview_bounded(value: &JsonValue, limit: usize) -> String {
    let mut writer = BudgetWriter::new(limit, true);
    let result = serde_json::to_writer(&mut writer, value);
    let (_, mut rendered, exceeded) = writer.finish();
    if result.is_err() || exceeded {
        if limit >= 3 {
            let mut end = limit.saturating_sub(3).min(rendered.len());
            while !rendered.is_char_boundary(end) {
                end -= 1;
            }
            rendered.truncate(end);
            rendered.push_str("...");
        } else {
            let mut end = limit.min(rendered.len());
            while !rendered.is_char_boundary(end) {
                end -= 1;
            }
            rendered.truncate(end);
        }
    }
    rendered
}

fn write_value_json<W: Write>(
    writer: &mut W,
    value: &Value,
    depth: usize,
    max_depth: usize,
) -> io::Result<()> {
    if depth >= max_depth {
        return Err(io::Error::other("value nesting budget exceeded"));
    }
    match value {
        Value::Null => writer.write_all(b"null"),
        Value::Bool(value) => writer.write_all(if *value { b"true" } else { b"false" }),
        Value::Int(value) => write!(writer, "{value}"),
        Value::Decimal(value) => match serde_json::Number::from_f64(*value) {
            Some(value) => serde_json::to_writer(writer, &value).map_err(io::Error::other),
            None => writer.write_all(b"null"),
        },
        Value::String(value) | Value::Uri(value) => {
            serde_json::to_writer(writer, value).map_err(io::Error::other)
        }
        Value::List(values) => {
            writer.write_all(b"[")?;
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    writer.write_all(b",")?;
                }
                write_value_json(writer, value, depth + 1, max_depth)?;
            }
            writer.write_all(b"]")
        }
        Value::Record(fields) => {
            writer.write_all(b"{")?;
            for (index, (name, value)) in fields.iter().enumerate() {
                if index > 0 {
                    writer.write_all(b",")?;
                }
                serde_json::to_writer(&mut *writer, name).map_err(io::Error::other)?;
                writer.write_all(b":")?;
                write_value_json(writer, value, depth + 1, max_depth)?;
            }
            writer.write_all(b"}")
        }
        Value::Tagged { name, payload } => {
            writer.write_all(b"{\"tag\":")?;
            serde_json::to_writer(&mut *writer, name).map_err(io::Error::other)?;
            if let Some(payload) = payload {
                writer.write_all(b",\"value\":")?;
                write_value_json(writer, payload, depth + 1, max_depth)?;
            }
            writer.write_all(b"}")
        }
        Value::Callable(_) => writer.write_all(b"\"<function>\""),
    }
}

fn value_json_bounded(
    value: &Value,
    limit: usize,
    retain: bool,
    max_depth: usize,
) -> Option<(usize, String)> {
    let mut writer = BudgetWriter::new(limit, retain);
    let result = write_value_json(&mut writer, value, 0, max_depth);
    let (written, rendered, exceeded) = writer.finish();
    (result.is_ok() && !exceeded).then_some((written, rendered))
}

fn render_value_bounded(value: &Value, limit: usize, max_depth: usize) -> RuntimeResult<String> {
    match value {
        Value::String(value) | Value::Uri(value) if value.len() <= limit => Ok(value.clone()),
        Value::String(_) | Value::Uri(_) => Err(RuntimeError::Limit(
            "intermediate value budget exceeded".into(),
        )),
        _ => value_json_bounded(value, limit, true, max_depth)
            .map(|(_, rendered)| rendered)
            .ok_or_else(|| RuntimeError::Limit("intermediate value budget exceeded".into())),
    }
}
fn compare_sort_keys(left: &Value, right: &Value) -> Ordering {
    match (left, right) {
        (Value::Int(left), Value::Int(right)) => left.cmp(right),
        (Value::Int(left), Value::Decimal(right)) => {
            (*left as f64).partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (Value::Decimal(left), Value::Int(right)) => left
            .partial_cmp(&(*right as f64))
            .unwrap_or(Ordering::Equal),
        (Value::Decimal(left), Value::Decimal(right)) => {
            left.partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (Value::Int(_) | Value::Decimal(_), _) => Ordering::Less,
        (_, Value::Int(_) | Value::Decimal(_)) => Ordering::Greater,
        _ => compare_values(left, right),
    }
}

fn compare_values(left: &Value, right: &Value) -> Ordering {
    fn rank(value: &Value) -> u8 {
        match value {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Int(_) | Value::Decimal(_) => 2,
            Value::String(_) => 3,
            Value::Uri(_) => 4,
            Value::List(_) => 5,
            Value::Record(_) => 6,
            Value::Tagged { .. } => 7,
            Value::Callable(_) => 8,
        }
    }
    match (left, right) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Bool(left), Value::Bool(right)) => left.cmp(right),
        (Value::String(left), Value::String(right)) | (Value::Uri(left), Value::Uri(right)) => {
            left.cmp(right)
        }
        (Value::List(left), Value::List(right)) => left
            .iter()
            .zip(right)
            .map(|(left, right)| compare_sort_keys(left, right))
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or_else(|| left.len().cmp(&right.len())),
        (Value::Record(left), Value::Record(right)) => left
            .iter()
            .zip(right)
            .map(|((left_name, left), (right_name, right))| {
                left_name
                    .cmp(right_name)
                    .then_with(|| compare_sort_keys(left, right))
            })
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or_else(|| left.len().cmp(&right.len())),
        (
            Value::Tagged {
                name: left_name,
                payload: left_payload,
            },
            Value::Tagged {
                name: right_name,
                payload: right_payload,
            },
        ) => left_name.cmp(right_name).then_with(|| {
            left_payload
                .as_deref()
                .zip(right_payload.as_deref())
                .map(|(left, right)| compare_sort_keys(left, right))
                .unwrap_or_else(|| left_payload.is_some().cmp(&right_payload.is_some()))
        }),
        _ => rank(left).cmp(&rank(right)),
    }
}
fn error_name(error: &RuntimeError) -> &str {
    match error {
        RuntimeError::Effect { name, .. } => name,
        RuntimeError::Type(_) => "TypeError",
        RuntimeError::Limit(_) => "ResourceLimitError",
        RuntimeError::Persistence(_) => "RuntimePersistenceError",
        RuntimeError::Cancelled => "CancellationError",
        RuntimeError::Diagnostic(_) => "DiagnosticError",
    }
}

fn logical_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        _ => unreachable!("logical_name only accepts boolean operators"),
    }
}
fn prelude() -> Environment {
    [
        ("print", 1),
        ("display", 2),
        ("length", 1),
        ("lines", 1),
        ("split", 2),
        ("contains", 2),
        ("take", 2),
        ("merge", 2),
        ("map", 2),
        ("flatmap", 2),
        ("filter", 2),
        ("where", 2),
        ("select", 2),
        ("sort_by", 3),
        ("group_by", 2),
        ("aggregate", 2),
        ("sum", 1),
        ("table", 1),
        ("par", 1),
        ("help", 1),
        ("rethrow", 1),
    ]
    .into_iter()
    .map(|(name, arity)| {
        (
            name.into(),
            Value::Callable(Rc::new(Callable::Builtin {
                name: name.into(),
                args: Vec::new(),
                arity,
            })),
        )
    })
    .chain([
        ("asc".into(), Value::String("asc".into())),
        ("desc".into(), Value::String("desc".into())),
    ])
    .collect()
}
fn list_call_args(args: Vec<Value>, name: &str) -> RuntimeResult<(Value, Vec<Value>)> {
    let mut args = args.into_iter();
    match (args.next(), args.next()) {
        (Some(function @ Value::Callable(_)), Some(Value::List(values))) => Ok((function, values)),
        _ => Err(RuntimeError::Type(format!(
            "{name} requires function and list"
        ))),
    }
}
fn match_pattern_counted(
    pattern: &Pattern,
    value: &Value,
    max_visits: usize,
    max_depth: usize,
) -> (Option<Environment>, usize) {
    let mut env = Environment::new();
    let mut visits = 0usize;
    fn visit(visits: &mut usize, max_visits: usize) -> bool {
        if *visits >= max_visits {
            *visits = visits.saturating_add(1);
            false
        } else {
            *visits += 1;
            true
        }
    }
    fn walk_list(
        pattern: &Pattern,
        values: &[Value],
        env: &mut Environment,
        visits: &mut usize,
        max_visits: usize,
        depth: usize,
        max_depth: usize,
    ) -> bool {
        if depth >= max_depth {
            *visits = max_visits.saturating_add(1);
            return false;
        }
        if !visit(visits, max_visits) {
            return false;
        }
        match &pattern.node {
            PatternKind::Wildcard => true,
            PatternKind::Bind(name) => {
                // A bound tail must own a value, but nested cons matching otherwise walks the
                // original slice and never repeatedly clones progressively smaller tails.
                let next = visits.saturating_add(values.len());
                if next > max_visits {
                    *visits = max_visits.saturating_add(1);
                    return false;
                }
                *visits = next;
                env.insert(name.clone(), Value::List(values.to_vec()));
                true
            }
            PatternKind::List(patterns) if patterns.len() == values.len() => {
                patterns.iter().zip(values).all(|(pattern, value)| {
                    walk(
                        pattern,
                        value,
                        env,
                        visits,
                        max_visits,
                        depth + 1,
                        max_depth,
                    )
                })
            }
            PatternKind::Cons { head, tail } if !values.is_empty() => {
                walk(
                    head,
                    &values[0],
                    env,
                    visits,
                    max_visits,
                    depth + 1,
                    max_depth,
                ) && walk_list(
                    tail,
                    &values[1..],
                    env,
                    visits,
                    max_visits,
                    depth + 1,
                    max_depth,
                )
            }
            _ => false,
        }
    }

    fn walk(
        pattern: &Pattern,
        value: &Value,
        env: &mut Environment,
        visits: &mut usize,
        max_visits: usize,
        depth: usize,
        max_depth: usize,
    ) -> bool {
        if depth >= max_depth {
            *visits = max_visits.saturating_add(1);
            return false;
        }
        if !visit(visits, max_visits) {
            return false;
        }
        match (&pattern.node, value) {
            (PatternKind::Wildcard, _) => true,
            (PatternKind::Bind(name), value) => {
                env.insert(name.clone(), value.clone());
                true
            }
            (PatternKind::String(a), Value::String(b)) => a == b,
            (PatternKind::Int(a), Value::Int(b)) => a == b,
            (PatternKind::Bool(a), Value::Bool(b)) => a == b,
            (PatternKind::Null, Value::Null) => true,
            (
                PatternKind::Constructor { name, payload },
                Value::Tagged {
                    name: actual,
                    payload: value,
                },
            ) if name == actual => match (payload, value) {
                (None, None) => true,
                (Some(pattern), Some(value)) => walk(
                    pattern,
                    value,
                    env,
                    visits,
                    max_visits,
                    depth + 1,
                    max_depth,
                ),
                _ => false,
            },
            (PatternKind::List(patterns), Value::List(values))
                if patterns.len() == values.len() =>
            {
                patterns.iter().zip(values).all(|(pattern, value)| {
                    walk(
                        pattern,
                        value,
                        env,
                        visits,
                        max_visits,
                        depth + 1,
                        max_depth,
                    )
                })
            }
            (PatternKind::Cons { .. }, Value::List(values)) => walk_list(
                pattern,
                values,
                env,
                visits,
                max_visits,
                depth + 1,
                max_depth,
            ),
            (PatternKind::Record { fields, rest }, Value::Record(values)) => {
                (*rest || fields.len() == values.len())
                    && fields.iter().all(|(name, pattern)| {
                        values.get(name).is_some_and(|value| {
                            walk(
                                pattern,
                                value,
                                env,
                                visits,
                                max_visits,
                                depth + 1,
                                max_depth,
                            )
                        })
                    })
            }
            _ => false,
        }
    }
    let matched = walk(
        pattern,
        value,
        &mut env,
        &mut visits,
        max_visits,
        0,
        max_depth,
    );
    (matched.then_some(env), visits)
}
fn unary(op: UnaryOp, value: Value) -> RuntimeResult<Value> {
    match (op, value) {
        (UnaryOp::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
        (UnaryOp::Negate, Value::Int(value)) => value
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(|| RuntimeError::Type("integer overflow in negation".into())),
        (UnaryOp::Negate, Value::Decimal(value)) => Ok(Value::Decimal(-value)),
        (_, value) => Err(RuntimeError::Type(format!(
            "invalid unary operand {}",
            value.kind()
        ))),
    }
}
fn binary(op: BinaryOp, left: Value, right: Value) -> RuntimeResult<Value> {
    use BinaryOp::*;
    match op {
        Equal => Ok(Value::Bool(left == right)),
        NotEqual => Ok(Value::Bool(left != right)),
        And | Or => match (left, right) {
            (Value::Bool(left), Value::Bool(right)) => Ok(Value::Bool(match op {
                And => left && right,
                Or => left || right,
                _ => unreachable!(),
            })),
            (left, right) => Err(RuntimeError::Type(format!(
                "{} requires boolean operands; found {} and {}",
                logical_name(op),
                left.kind(),
                right.kind()
            ))),
        },
        Cons => match right {
            Value::List(mut values) => {
                values.insert(0, left);
                Ok(Value::List(values))
            }
            _ => Err(RuntimeError::Type(":: tail must be a list".into())),
        },
        Add => match (left, right) {
            (Value::String(mut left), Value::String(right)) => {
                left.push_str(&right);
                Ok(Value::String(left))
            }
            (left, right) => {
                numeric_binary(left, right, i64::checked_add, |a, b| a + b, "addition")
            }
        },
        Subtract => numeric_binary(left, right, i64::checked_sub, |a, b| a - b, "subtraction"),
        Multiply => numeric_binary(
            left,
            right,
            i64::checked_mul,
            |a, b| a * b,
            "multiplication",
        ),
        Divide if is_numeric_zero(&right) => Err(RuntimeError::Effect {
            name: "DivisionByZero".into(),
            message: "division by zero".into(),
            payload: Some(json!({"operation": "division"})),
        }),
        Divide => numeric_binary(left, right, i64::checked_div, |a, b| a / b, "division"),
        Modulo if is_numeric_zero(&right) => Err(RuntimeError::Effect {
            name: "DivisionByZero".into(),
            message: "modulo by zero".into(),
            payload: Some(json!({"operation": "modulo"})),
        }),
        Modulo => numeric_binary(left, right, i64::checked_rem, |a, b| a % b, "modulo"),
        Less | LessEqual | Greater | GreaterEqual => {
            let (a, b) = numbers(left, right)?;
            Ok(Value::Bool(match op {
                Less => a < b,
                LessEqual => a <= b,
                Greater => a > b,
                GreaterEqual => a >= b,
                _ => unreachable!(),
            }))
        }
    }
}
fn is_numeric_zero(value: &Value) -> bool {
    match value {
        Value::Int(value) => *value == 0,
        Value::Decimal(value) => *value == 0.0,
        _ => false,
    }
}
fn numeric_binary(
    left: Value,
    right: Value,
    ints: impl FnOnce(i64, i64) -> Option<i64>,
    decimals: impl FnOnce(f64, f64) -> f64,
    operation: &str,
) -> RuntimeResult<Value> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => ints(a, b)
            .map(Value::Int)
            .ok_or_else(|| RuntimeError::Type(format!("integer overflow in {operation}"))),
        (a, b) => {
            let (a, b) = numbers(a, b)?;
            Ok(Value::Decimal(decimals(a, b)))
        }
    }
}
fn numbers(left: Value, right: Value) -> RuntimeResult<(f64, f64)> {
    let number = |value| match value {
        Value::Int(v) => Some(v as f64),
        Value::Decimal(v) => Some(v),
        _ => None,
    };
    let a = number(left).ok_or_else(|| RuntimeError::Type("expected number".into()))?;
    let b = number(right).ok_or_else(|| RuntimeError::Type("expected number".into()))?;
    Ok((a, b))
}

fn normalize_effect_args(name: &str, argument: Value) -> JsonValue {
    match (name, argument) {
        ("fs.read", Value::String(path) | Value::Uri(path)) => json!({"path": path}),
        (_, argument) => argument.to_json(),
    }
}

const MAP_SLOT_OVERHEAD: usize = std::mem::size_of::<String>() + 3 * std::mem::size_of::<usize>();

struct RetainedSizer {
    size: usize,
    limit: usize,
    depth: usize,
    max_depth: usize,
    callables: BTreeSet<usize>,
}

impl RetainedSizer {
    fn new(limit: usize, max_depth: usize) -> Self {
        Self {
            size: 0,
            limit,
            depth: 0,
            max_depth,
            callables: BTreeSet::new(),
        }
    }

    fn add(&mut self, bytes: usize) -> bool {
        self.size = self.size.saturating_add(bytes);
        self.size <= self.limit
    }

    fn entry(&mut self, name: &str, value: &Value) -> bool {
        self.add(MAP_SLOT_OVERHEAD.saturating_add(name.len())) && self.value(value)
    }

    fn enter_depth(&mut self) -> bool {
        if self.depth >= self.max_depth {
            return false;
        }
        self.depth += 1;
        true
    }

    fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn environment(&mut self, environment: &Environment) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = self.add(std::mem::size_of::<Environment>())
            && environment
                .iter()
                .all(|(name, value)| self.entry(name, value));
        self.leave_depth();
        within_budget
    }

    fn pattern_heap(&mut self, pattern: &Pattern) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = match &pattern.node {
            PatternKind::Wildcard
            | PatternKind::Int(_)
            | PatternKind::Bool(_)
            | PatternKind::Null => true,
            PatternKind::Bind(value) | PatternKind::String(value) => self.add(value.capacity()),
            PatternKind::Constructor { name, payload } => {
                self.add(name.capacity())
                    && payload.as_deref().is_none_or(|payload| {
                        self.add(std::mem::size_of::<Pattern>()) && self.pattern_heap(payload)
                    })
            }
            PatternKind::List(values) => {
                self.add(
                    values
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Pattern>()),
                ) && values.iter().all(|value| self.pattern_heap(value))
            }
            PatternKind::Cons { head, tail } => {
                self.add(2usize.saturating_mul(std::mem::size_of::<Pattern>()))
                    && self.pattern_heap(head)
                    && self.pattern_heap(tail)
            }
            PatternKind::Record { fields, .. } => {
                self.add(
                    fields
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(String, Pattern)>()),
                ) && fields
                    .iter()
                    .all(|(name, value)| self.add(name.capacity()) && self.pattern_heap(value))
            }
        };
        self.leave_depth();
        within_budget
    }

    fn type_term_heap(&mut self, term: &TypeTerm) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = match term {
            TypeTerm::Named(name) => self.add(name.capacity()),
            TypeTerm::List(value) | TypeTerm::Option(value) => {
                self.add(std::mem::size_of::<TypeTerm>()) && self.type_term_heap(value)
            }
            TypeTerm::Record(fields) => {
                self.add(
                    fields
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(String, TypeTerm)>()),
                ) && fields
                    .iter()
                    .all(|(name, value)| self.add(name.capacity()) && self.type_term_heap(value))
            }
        };
        self.leave_depth();
        within_budget
    }

    fn type_decl_heap(&mut self, decl: &TypeDecl) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = self.add(decl.name.capacity())
            && self.add(
                decl.variants
                    .capacity()
                    .saturating_mul(std::mem::size_of::<crate::VariantDecl>()),
            )
            && decl.variants.iter().all(|variant| {
                self.add(variant.name.capacity())
                    && variant
                        .payload
                        .as_ref()
                        .is_none_or(|payload| self.type_term_heap(payload))
            });
        self.leave_depth();
        within_budget
    }

    fn match_arm_heap(&mut self, arm: &MatchArm) -> bool {
        self.pattern_heap(&arm.pattern) && self.expr_heap(&arm.value)
    }

    fn form_heap(&mut self, form: &Form) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = match &form.node {
            FormKind::Type(decl) => self.type_decl_heap(decl),
            FormKind::Let { pattern, value } => self.pattern_heap(pattern) && self.expr_heap(value),
            FormKind::Fun { name, params, body } => {
                self.add(name.capacity())
                    && self.add(
                        params
                            .capacity()
                            .saturating_mul(std::mem::size_of::<Pattern>()),
                    )
                    && params.iter().all(|param| self.pattern_heap(param))
                    && self.expr_heap(body)
            }
            FormKind::Expr(expr) => self.expr_heap(expr),
        };
        self.leave_depth();
        within_budget
    }

    fn boxed_expr_heap(&mut self, expr: &Expr) -> bool {
        self.add(std::mem::size_of::<Expr>()) && self.expr_heap(expr)
    }

    fn expr_heap(&mut self, expr: &Expr) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = match &expr.node {
            ExprKind::String(value)
            | ExprKind::Uri(value)
            | ExprKind::Name(value)
            | ExprKind::Constructor(value)
            | ExprKind::Capability(value) => self.add(value.capacity()),
            ExprKind::Int(_) | ExprKind::Decimal(_) | ExprKind::Bool(_) | ExprKind::Null => true,
            ExprKind::List(values) => {
                self.add(
                    values
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Expr>()),
                ) && values.iter().all(|value| self.expr_heap(value))
            }
            ExprKind::Record(fields) => {
                self.add(
                    fields
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(String, Expr)>()),
                ) && fields
                    .iter()
                    .all(|(name, value)| self.add(name.capacity()) && self.expr_heap(value))
            }
            ExprKind::Lambda { params, body } => {
                self.add(
                    params
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Pattern>()),
                ) && params.iter().all(|param| self.pattern_heap(param))
                    && self.boxed_expr_heap(body)
            }
            ExprKind::Apply { function, argument } => {
                self.boxed_expr_heap(function) && self.boxed_expr_heap(argument)
            }
            ExprKind::Field { target, field } => {
                self.boxed_expr_heap(target) && self.add(field.capacity())
            }
            ExprKind::Unary { value, .. } => self.boxed_expr_heap(value),
            ExprKind::Binary { left, right, .. }
            | ExprKind::Pipe {
                value: left,
                target: right,
            } => self.boxed_expr_heap(left) && self.boxed_expr_heap(right),
            ExprKind::If {
                condition,
                then_value,
                else_value,
            } => {
                self.boxed_expr_heap(condition)
                    && self.boxed_expr_heap(then_value)
                    && self.boxed_expr_heap(else_value)
            }
            ExprKind::Match { value, arms } | ExprKind::Handle { value, arms } => {
                self.boxed_expr_heap(value)
                    && self.add(
                        arms.capacity()
                            .saturating_mul(std::mem::size_of::<MatchArm>()),
                    )
                    && arms.iter().all(|arm| self.match_arm_heap(arm))
            }
            ExprKind::Block(forms) => {
                self.add(forms.capacity().saturating_mul(std::mem::size_of::<Form>()))
                    && forms.iter().all(|form| self.form_heap(form))
            }
        };
        self.leave_depth();
        within_budget
    }

    fn value(&mut self, value: &Value) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = (|| {
            if !self.add(std::mem::size_of::<Value>()) {
                return false;
            }
            match value {
                Value::Null | Value::Bool(_) | Value::Int(_) | Value::Decimal(_) => true,
                Value::String(value) | Value::Uri(value) => self.add(value.capacity()),
                Value::List(values) => {
                    self.add(
                        values
                            .capacity()
                            .saturating_sub(values.len())
                            .saturating_mul(std::mem::size_of::<Value>()),
                    ) && values.iter().all(|value| self.value(value))
                }
                Value::Record(fields) => {
                    self.add(std::mem::size_of::<Environment>())
                        && fields.iter().all(|(name, value)| self.entry(name, value))
                }
                Value::Tagged { name, payload } => {
                    self.add(name.capacity())
                        && payload.as_deref().is_none_or(|payload| {
                            self.add(std::mem::size_of::<Value>()) && self.value(payload)
                        })
                }
                Value::Callable(callable) => self.callable(callable),
            }
        })();
        self.leave_depth();
        within_budget
    }

    fn callable(&mut self, callable: &Rc<Callable>) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = (|| {
            let pointer = Rc::as_ptr(callable) as usize;
            if !self.callables.insert(pointer) {
                return true;
            }
            if !self.add(
                std::mem::size_of::<Callable>().saturating_add(2 * std::mem::size_of::<usize>()),
            ) {
                return false;
            }
            match callable.as_ref() {
                Callable::Builtin { name, args, .. } => {
                    self.add(name.capacity()) && args.iter().all(|value| self.value(value))
                }
                Callable::BuiltinDataLast {
                    name, args, data, ..
                } => {
                    self.add(name.capacity())
                        && args.iter().all(|value| self.value(value))
                        && self.value(data)
                }
                Callable::User {
                    params,
                    body,
                    captured,
                    args,
                } => {
                    self.add(
                        params
                            .capacity()
                            .saturating_mul(std::mem::size_of::<Pattern>()),
                    ) && params.iter().all(|param| self.pattern_heap(param))
                        && self.expr_heap(body)
                        && self.environment(captured)
                        && args.iter().all(|value| self.value(value))
                }
                Callable::Row { body, captured } => {
                    self.expr_heap(body) && self.environment(captured)
                }
                Callable::Capability { name } | Callable::Constructor { name } => {
                    self.add(name.capacity())
                }
            }
        })();
        self.leave_depth();
        within_budget
    }
}

fn value_size_bounded(value: &Value, limit: usize, max_depth: usize) -> usize {
    let mut sizer = RetainedSizer::new(limit, max_depth);
    if sizer.value(value) {
        sizer.size
    } else {
        limit.saturating_add(1)
    }
}

fn environment_size_bounded(environment: &Environment, limit: usize, max_depth: usize) -> usize {
    let mut sizer = RetainedSizer::new(limit, max_depth);
    if sizer.environment(environment) {
        sizer.size
    } else {
        limit.saturating_add(1)
    }
}

fn merged_environment_size_bounded(
    base: &Environment,
    overlay: &Environment,
    committed: &BTreeSet<String>,
    limit: usize,
    max_depth: usize,
) -> usize {
    let mut sizer = RetainedSizer::new(limit, max_depth);
    let within_budget = sizer.add(std::mem::size_of::<Environment>())
        && base
            .iter()
            .filter(|(name, _)| !committed.contains(*name))
            .all(|(name, value)| sizer.entry(name, value))
        && committed.iter().all(|name| {
            overlay
                .get(name)
                .is_some_and(|value| sizer.entry(name, value))
        });
    if within_budget {
        sizer.size
    } else {
        limit.saturating_add(1)
    }
}

fn environment_clone_size_bounded(
    environment: &Environment,
    limit: usize,
    max_depth: usize,
) -> usize {
    fn add(size: &mut usize, bytes: usize, limit: usize) -> bool {
        *size = size.saturating_add(bytes);
        *size <= limit
    }

    fn value_heap(
        value: &Value,
        size: &mut usize,
        limit: usize,
        depth: usize,
        max_depth: usize,
    ) -> bool {
        if depth >= max_depth {
            return false;
        }
        match value {
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Decimal(_) => true,
            Value::String(value) | Value::Uri(value) => add(size, value.len(), limit),
            Value::List(values) => {
                add(
                    size,
                    values.len().saturating_mul(std::mem::size_of::<Value>()),
                    limit,
                ) && values
                    .iter()
                    .all(|value| value_heap(value, size, limit, depth + 1, max_depth))
            }
            Value::Record(fields) => fields.iter().all(|(name, value)| {
                add(size, MAP_SLOT_OVERHEAD.saturating_add(name.len()), limit)
                    && value_heap(value, size, limit, depth + 1, max_depth)
            }),
            Value::Tagged { name, payload } => {
                add(size, name.len(), limit)
                    && payload.as_deref().is_none_or(|payload| {
                        add(size, std::mem::size_of::<Value>(), limit)
                            && value_heap(payload, size, limit, depth + 1, max_depth)
                    })
            }
            Value::Callable(_) => true,
        }
    }

    let mut size = 0usize;
    let within_budget = environment.iter().all(|(name, value)| {
        add(
            &mut size,
            MAP_SLOT_OVERHEAD.saturating_add(name.len()),
            limit,
        ) && value_heap(value, &mut size, limit, 0, max_depth)
    });
    if within_budget {
        size
    } else {
        limit.saturating_add(1)
    }
}

fn json_value_size_bounded(value: &JsonValue, limit: usize, max_depth: usize) -> usize {
    fn walk(
        value: &JsonValue,
        size: &mut usize,
        limit: usize,
        depth: usize,
        max_depth: usize,
    ) -> bool {
        if depth >= max_depth {
            return false;
        }
        *size = size.saturating_add(std::mem::size_of::<JsonValue>());
        if *size > limit {
            return false;
        }
        match value {
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) => true,
            JsonValue::String(value) => {
                *size = size.saturating_add(value.capacity());
                *size <= limit
            }
            JsonValue::Array(values) => values
                .iter()
                .all(|value| walk(value, size, limit, depth + 1, max_depth)),
            JsonValue::Object(fields) => fields.iter().all(|(name, value)| {
                *size = size
                    .saturating_add(MAP_SLOT_OVERHEAD)
                    .saturating_add(name.len());
                *size <= limit && walk(value, size, limit, depth + 1, max_depth)
            }),
        }
    }

    let mut size = 0usize;
    if walk(value, &mut size, limit, 0, max_depth) {
        size
    } else {
        limit.saturating_add(1)
    }
}

fn group_key(value: &Value, limit: usize, max_depth: usize) -> RuntimeResult<String> {
    fn write_key(writer: &mut BudgetWriter, bytes: &[u8]) -> RuntimeResult<()> {
        writer
            .write_all(bytes)
            .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))
    }

    fn append(
        value: &Value,
        writer: &mut BudgetWriter,
        depth: usize,
        max_depth: usize,
    ) -> RuntimeResult<()> {
        if depth >= max_depth {
            return Err(RuntimeError::Limit(
                "group key nesting budget exceeded".into(),
            ));
        }
        match value {
            Value::Null => write_key(writer, b"n")?,
            Value::Bool(value) => write_key(writer, if *value { b"b1" } else { b"b0" })?,
            Value::Int(value) => write!(writer, "i{value}")
                .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))?,
            Value::Decimal(value) => write!(writer, "d{:016x}", value.to_bits())
                .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))?,
            Value::String(value) => {
                write_key(writer, b"s")?;
                serde_json::to_writer(writer, value)
                    .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))?;
            }
            Value::Uri(value) => {
                write_key(writer, b"u")?;
                serde_json::to_writer(writer, value)
                    .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))?;
            }
            Value::List(values) => {
                write_key(writer, b"l[")?;
                for value in values {
                    append(value, writer, depth + 1, max_depth)?;
                    write_key(writer, b";")?;
                }
                write_key(writer, b"]")?;
            }
            Value::Record(fields) => {
                write_key(writer, b"r{")?;
                for (name, value) in fields {
                    serde_json::to_writer(&mut *writer, name).map_err(|_| {
                        RuntimeError::Limit("group key value budget exceeded".into())
                    })?;
                    write_key(writer, b":")?;
                    append(value, writer, depth + 1, max_depth)?;
                    write_key(writer, b";")?;
                }
                write_key(writer, b"}")?;
            }
            Value::Tagged { name, payload } => {
                write_key(writer, b"t")?;
                serde_json::to_writer(&mut *writer, name)
                    .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))?;
                if let Some(payload) = payload {
                    write_key(writer, b":")?;
                    append(payload, writer, depth + 1, max_depth)?;
                }
            }
            Value::Callable(_) => {
                return Err(RuntimeError::Type(
                    "group_by key cannot be a function".into(),
                ));
            }
        }
        Ok(())
    }

    let mut writer = BudgetWriter::new(limit, true);
    append(value, &mut writer, 0, max_depth)?;
    let (_, key, exceeded) = writer.finish();
    if exceeded {
        Err(RuntimeError::Limit(
            "group key value budget exceeded".into(),
        ))
    } else {
        Ok(key)
    }
}

pub fn catalog_from_registry(
    registry: &HostRegistry,
    invocation: &InvocationCtx,
    schemes: impl IntoIterator<Item = String>,
) -> CapabilityCatalog {
    let mut catalog = CapabilityCatalog::new();
    for scheme in schemes {
        catalog = catalog.scheme(scheme);
    }
    for tool in registry.search("", None, usize::MAX, invocation) {
        if !tool.granted {
            continue;
        }
        if let Ok(docs) = registry.docs(&tool.name, invocation) {
            let mut signature = EffectSignature::new(&tool.name, ValueType::Any, ValueType::Any);
            signature.errors = docs.errors.into_iter().map(|error| error.name).collect();
            signature.approval = docs.approval;
            signature.sensitive = docs.sensitive;
            signature.resumable = signature.approval != "none";
            catalog = catalog.register(signature).allow(tool.name);
        }
    }
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_environment_deduplicates_callable_graphs_but_charges_alias_slots() {
        let max_depth = RuntimeLimits::default().runtime_depth;
        let callable = Rc::new(Callable::Builtin {
            name: "fixture".into(),
            args: vec![Value::String("payload".repeat(32))],
            arity: 2,
        });
        let mut environment =
            Environment::from([("first".into(), Value::Callable(callable.clone()))]);
        let one_size = environment_size_bounded(&environment, usize::MAX, max_depth);
        environment.insert("alias".into(), Value::Callable(callable));
        let alias_size = environment_size_bounded(&environment, usize::MAX, max_depth);
        environment.insert(
            "distinct".into(),
            Value::Callable(Rc::new(Callable::Builtin {
                name: "fixture".into(),
                args: vec![Value::String("payload".repeat(32))],
                arity: 2,
            })),
        );
        let distinct_size = environment_size_bounded(&environment, usize::MAX, max_depth);

        assert!(alias_size > one_size, "the alias map slot must be charged");
        assert!(
            distinct_size - alias_size > alias_size - one_size,
            "a distinct callable graph must cost more than an Rc alias"
        );
    }

    #[test]
    fn value_budget_charges_scalar_container_slots_and_json_trees() {
        let max_depth = RuntimeLimits::default().runtime_depth;
        let values = Value::List(vec![Value::Null; 256]);
        assert!(value_size_bounded(&values, 8 * 1024, max_depth) > 8 * 1024);
        let json_values = JsonValue::Array(vec![JsonValue::Null; 256]);
        assert!(json_value_size_bounded(&json_values, 8 * 1024, max_depth) > 8 * 1024);
    }

    #[test]
    fn retained_sizing_and_rendering_reject_excessive_value_depth() {
        let deep = (0..64).fold(Value::Null, |value, _| Value::List(vec![value]));
        let limit = 1024 * 1024;

        assert!(value_size_bounded(&deep, limit, 16) > limit);
        assert!(value_json_bounded(&deep, limit, true, 16).is_none());
    }
}

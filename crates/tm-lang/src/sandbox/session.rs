use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
    future::Future,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures::StreamExt;
use tm_core::{CancellationToken, CellBudget, EvalOutput, Result, Session};

use crate::{
    Interpreter, RuntimeError, RuntimeLimits, RuntimeOutput, RuntimeResult,
    batch::binding_usage_bounded,
};

pub(super) struct TmSession {
    pub(super) interpreter: Interpreter,
    pub(super) cancellation: Option<Arc<dyn CancellationToken>>,
    pub(super) limits: RuntimeLimits,
}

enum EvaluationRace {
    Completed(RuntimeResult<RuntimeOutput>),
    Cancelled,
    TimedOut,
    TerminalPersistenceTimedOut,
}

const TERMINAL_PERSISTENCE_GRACE: Duration = Duration::from_secs(1);

struct BoundedFormatter {
    text: String,
    max_bytes: usize,
}

impl BoundedFormatter {
    fn new(max_bytes: usize) -> Self {
        Self {
            text: String::with_capacity(max_bytes.min(1024)),
            max_bytes,
        }
    }
}

impl fmt::Write for BoundedFormatter {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let remaining = self.max_bytes.saturating_sub(self.text.len());
        if remaining == 0 {
            return Err(fmt::Error);
        }
        let mut end = remaining.min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&value[..end]);
        if end == value.len() {
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }
}

fn bounded_display(value: &impl fmt::Display, max_bytes: usize) -> String {
    let mut output = BoundedFormatter::new(max_bytes);
    let _ = write!(&mut output, "{value}");
    output.text
}

async fn race_evaluation(
    interpreter: &mut Interpreter,
    code: &str,
    budget: CellBudget,
    cancellation: Option<&dyn CancellationToken>,
) -> EvaluationRace {
    let terminal_selected = interpreter.terminal_selected_handle();
    terminal_selected.store(false, std::sync::atomic::Ordering::Release);
    let evaluation = interpreter.eval(code, budget.output_bytes);
    tokio::pin!(evaluation);
    let wall = tokio::time::sleep(Duration::from_millis(budget.wall_ms));
    tokio::pin!(wall);
    if let Some(token) = cancellation {
        tokio::select! {
            result = &mut evaluation => EvaluationRace::Completed(result),
            _ = token.cancelled() => {
                if terminal_selected.load(std::sync::atomic::Ordering::Acquire) {
                    match tokio::time::timeout(TERMINAL_PERSISTENCE_GRACE, &mut evaluation).await {
                        Ok(result) => EvaluationRace::Completed(result),
                        Err(_) => EvaluationRace::TerminalPersistenceTimedOut,
                    }
                } else {
                    EvaluationRace::Cancelled
                }
            },
            _ = &mut wall => {
                if terminal_selected.load(std::sync::atomic::Ordering::Acquire) {
                    match tokio::time::timeout(TERMINAL_PERSISTENCE_GRACE, &mut evaluation).await {
                        Ok(result) => EvaluationRace::Completed(result),
                        Err(_) => EvaluationRace::TerminalPersistenceTimedOut,
                    }
                } else {
                    EvaluationRace::TimedOut
                }
            },
        }
    } else {
        tokio::select! {
            result = &mut evaluation => EvaluationRace::Completed(result),
            _ = &mut wall => {
                if terminal_selected.load(std::sync::atomic::Ordering::Acquire) {
                    match tokio::time::timeout(TERMINAL_PERSISTENCE_GRACE, &mut evaluation).await {
                        Ok(result) => EvaluationRace::Completed(result),
                        Err(_) => EvaluationRace::TerminalPersistenceTimedOut,
                    }
                } else {
                    EvaluationRace::TimedOut
                }
            },
        }
    }
}

async fn cancel_active_bounded(
    interpreter: &mut Interpreter,
    status: &str,
    reason: &str,
) -> Result<()> {
    match tokio::time::timeout(
        TERMINAL_PERSISTENCE_GRACE,
        interpreter.cancel_active_eval(status, reason),
    )
    .await
    {
        Ok(result) => result.map_err(|error| tm_core::Error::Sandbox(error.to_string())),
        Err(_) => {
            interpreter.abandon_active_eval();
            Err(tm_core::Error::Sandbox(
                "terminal event persistence deadline exceeded".into(),
            ))
        }
    }
}

async fn emit_immediate_bounded(
    interpreter: &mut Interpreter,
    code: &str,
    status: &str,
    reason: &str,
) -> Result<()> {
    persist_terminal_bounded(interpreter.emit_immediate_terminal(code, status, reason)).await
}

async fn emit_dependency_failure_bounded(
    interpreter: &mut Interpreter,
    code: &str,
    reason: &str,
) -> Result<()> {
    persist_terminal_bounded(interpreter.emit_dependency_failure(code, reason)).await
}

async fn persist_terminal_bounded(
    persistence: impl Future<Output = RuntimeResult<()>>,
) -> Result<()> {
    match tokio::time::timeout(TERMINAL_PERSISTENCE_GRACE, persistence).await {
        Ok(result) => result.map_err(|error| tm_core::Error::Sandbox(error.to_string())),
        Err(_) => Err(tm_core::Error::Sandbox(
            "terminal event persistence deadline exceeded".into(),
        )),
    }
}

#[async_trait(?Send)]
impl Session for TmSession {
    fn handles_cancellation(&self) -> bool {
        self.cancellation.is_some()
    }

    async fn eval(&mut self, code: &str, budget: CellBudget) -> Result<EvalOutput> {
        if budget.wall_ms == 0 {
            emit_immediate_bounded(
                &mut self.interpreter,
                code,
                "timed_out",
                "cell exceeded wall-clock budget",
            )
            .await?;
            return Ok(timeout_output(budget.output_bytes));
        }
        if self
            .cancellation
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            emit_immediate_bounded(&mut self.interpreter, code, "cancelled", "cell cancelled")
                .await?;
            return Ok(cancelled_output(budget.output_bytes));
        }
        let result = race_evaluation(
            &mut self.interpreter,
            code,
            budget,
            self.cancellation.as_deref(),
        )
        .await;
        match result {
            EvaluationRace::Completed(Ok(output)) => Ok(EvalOutput {
                stdout: output.stdout,
                result: Some(output.value.to_json()),
                error: None,
            }),
            EvaluationRace::Completed(Err(RuntimeError::Persistence(error))) => {
                self.interpreter.abandon_active_eval();
                Err(tm_core::Error::Sandbox(error))
            }
            EvaluationRace::Completed(Err(error)) => Ok(EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some(bounded_display(&error, budget.output_bytes)),
            }),
            EvaluationRace::Cancelled => {
                cancel_active_bounded(&mut self.interpreter, "cancelled", "cell cancelled").await?;
                Ok(cancelled_output(budget.output_bytes))
            }
            EvaluationRace::TimedOut => {
                cancel_active_bounded(
                    &mut self.interpreter,
                    "timed_out",
                    "cell exceeded wall-clock budget",
                )
                .await?;
                Ok(timeout_output(budget.output_bytes))
            }
            EvaluationRace::TerminalPersistenceTimedOut => {
                let _ = cancel_active_bounded(
                    &mut self.interpreter,
                    "failed",
                    "terminal event persistence deadline exceeded",
                )
                .await;
                Err(tm_core::Error::Sandbox(
                    "terminal event persistence deadline exceeded".into(),
                ))
            }
        }
    }

    async fn eval_batch(
        &mut self,
        codes: &[String],
        budget: CellBudget,
    ) -> Result<Vec<EvalOutput>> {
        let usages = codes
            .iter()
            .map(|code| {
                binding_usage_bounded(
                    code,
                    self.limits.source_bytes,
                    self.limits.syntax_nodes,
                    self.limits.parse_depth,
                )
                .ok()
            })
            .collect::<Vec<_>>();
        let dependencies = batch_dependencies(&usages);
        let state_writing = usages
            .iter()
            .any(|usage| usage.as_ref().is_none_or(|usage| !usage.writes.is_empty()));
        if state_writing {
            // A fork emits `binding_committed` inside its own commit shield. The coordinator
            // cannot make that fork's private environment visible atomically if the outer batch
            // future is dropped, so state-writing (or unanalyzable) batches run directly on the
            // owning interpreter. Read/effect-only batches retain bounded parallel execution.
            let mut outputs = Vec::with_capacity(codes.len());
            let mut committed_by_cell = Vec::<Option<BTreeSet<String>>>::with_capacity(codes.len());
            for (index, code) in codes.iter().enumerate() {
                if let Some((dependency, names)) =
                    dependencies[index].iter().find_map(|(dependency, names)| {
                        committed_by_cell[*dependency]
                            .is_none()
                            .then_some((*dependency, names))
                    })
                {
                    let bindings = names.iter().cloned().collect::<Vec<_>>().join(", ");
                    let message = format!(
                        "BatchDependencyError: execute call {} requires binding(s) [{}] from failed execute call {}",
                        index + 1,
                        bindings,
                        dependency + 1
                    );
                    emit_dependency_failure_bounded(&mut self.interpreter, code, &message).await?;
                    let error = bounded_display(&message, budget.output_bytes);
                    outputs.push(EvalOutput {
                        error: Some(error),
                        ..EvalOutput::default()
                    });
                    committed_by_cell.push(None);
                    continue;
                }
                let cancellation = self.cancellation.clone();
                let (output, committed) =
                    eval_interpreter(&mut self.interpreter, code, budget, cancellation.as_deref())
                        .await?;
                outputs.push(output);
                committed_by_cell.push(committed);
            }
            return Ok(outputs);
        }

        let base = self.interpreter.clone();
        let mut pending = (0..codes.len()).collect::<BTreeSet<_>>();
        let mut results: Vec<Option<(Interpreter, EvalOutput, Option<BTreeSet<String>>)>> =
            vec![None; codes.len()];

        while !pending.is_empty() {
            let ready = pending
                .iter()
                .copied()
                .filter(|index| {
                    dependencies[*index]
                        .keys()
                        .all(|dependency| results[*dependency].is_some())
                })
                .collect::<Vec<_>>();
            debug_assert!(
                !ready.is_empty(),
                "forward-only batch graph must make progress"
            );

            let evaluations = ready.iter().map(|index| {
                let index = *index;
                let mut interpreter = base.fork_for_parallel(index as u64);
                let failed = dependencies[index].iter().find_map(|(dependency, names)| {
                    results[*dependency]
                        .as_ref()
                        .and_then(|(_, _, committed)| {
                            committed
                                .is_none()
                                .then_some((*dependency, names.iter().cloned().collect::<Vec<_>>()))
                        })
                });
                let successful_dependencies = dependencies[index]
                    .keys()
                    .filter_map(|dependency| {
                        results[*dependency].as_ref().and_then(
                            |(fork, _, committed)| {
                                committed
                                    .as_ref()
                                    .map(|names| (fork.clone(), names.clone()))
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                for (fork, committed) in successful_dependencies {
                    interpreter.merge_committed_from(&fork, &committed);
                }
                let cancellation = self.cancellation.clone();
                let code = &codes[index];
                async move {
                    let output = if let Some((dependency, names)) = failed {
                        let bindings = names.join(", ");
                        let message = format!(
                            "BatchDependencyError: execute call {} requires binding(s) [{}] from failed execute call {}",
                            index + 1,
                            bindings,
                            dependency + 1
                        );
                        match emit_dependency_failure_bounded(
                            &mut interpreter,
                            code,
                            &message,
                        )
                        .await
                        {
                            Ok(()) => Ok((
                                EvalOutput {
                                    error: Some(bounded_display(&message, budget.output_bytes)),
                                    ..EvalOutput::default()
                                },
                                None,
                            )),
                            Err(error) => Err(error),
                        }
                    } else {
                        eval_interpreter(
                            &mut interpreter,
                            code,
                            budget,
                            cancellation.as_deref(),
                        )
                        .await
                    };
                    (index, interpreter, output)
                }
            }).collect::<Vec<_>>();

            let mut evaluations =
                futures::stream::iter(evaluations).buffer_unordered(self.limits.parallelism.max(1));
            let mut wave_error = None;
            while let Some(evaluation) = evaluations.next().await {
                let (index, interpreter, evaluation) = evaluation;
                pending.remove(&index);
                match evaluation {
                    Ok((output, committed)) => {
                        results[index] = Some((interpreter, output, committed));
                    }
                    Err(error) if wave_error.is_none() => wave_error = Some(error),
                    Err(_) => {}
                }
            }
            if let Some(error) = wave_error {
                // Some siblings may already have durably emitted `binding_committed`. Merge every
                // successful fork in response order before surfacing a later sink/runtime error.
                for (fork, _, committed) in results.iter().flatten() {
                    if let Some(committed) = committed {
                        self.interpreter.merge_committed_from(fork, committed);
                    }
                }
                return Err(error);
            }
        }

        let mut outputs = Vec::with_capacity(results.len());
        for result in results.into_iter().flatten() {
            let (fork, output, committed) = result;
            if let Some(committed) = &committed {
                self.interpreter.merge_committed_from(&fork, committed);
            }
            outputs.push(output);
        }
        self.interpreter.finish_parallel_batch(codes.len() as u64);
        Ok(outputs)
    }

    async fn reset(&mut self) -> Result<()> {
        self.interpreter.reset();
        Ok(())
    }
}

fn batch_dependencies(
    usages: &[Option<crate::batch::BindingUsage>],
) -> Vec<BTreeMap<usize, BTreeSet<String>>> {
    let mut writers = BTreeMap::<String, usize>::new();
    let mut unknown_writers = Vec::new();
    let mut dependencies = Vec::with_capacity(usages.len());
    for (index, usage) in usages.iter().enumerate() {
        let mut cell = BTreeMap::<usize, BTreeSet<String>>::new();
        for writer in &unknown_writers {
            cell.entry(*writer)
                .or_default()
                .insert("<unknown bindings>".to_string());
        }
        if let Some(usage) = usage {
            for name in &usage.reads {
                if let Some(writer) = writers.get(name) {
                    cell.entry(*writer).or_default().insert(name.clone());
                }
            }
            for name in &usage.writes {
                writers.insert(name.clone(), index);
            }
        } else {
            unknown_writers.push(index);
        }
        dependencies.push(cell);
    }
    dependencies
}

async fn eval_interpreter(
    interpreter: &mut Interpreter,
    code: &str,
    budget: CellBudget,
    cancellation: Option<&dyn CancellationToken>,
) -> Result<(EvalOutput, Option<BTreeSet<String>>)> {
    if budget.wall_ms == 0 {
        emit_immediate_bounded(
            interpreter,
            code,
            "timed_out",
            "cell exceeded wall-clock budget",
        )
        .await?;
        return Ok((timeout_output(budget.output_bytes), None));
    }
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        emit_immediate_bounded(interpreter, code, "cancelled", "cell cancelled").await?;
        return Ok((cancelled_output(budget.output_bytes), None));
    }
    let result = race_evaluation(interpreter, code, budget, cancellation).await;
    match result {
        EvaluationRace::Completed(Ok(output)) => {
            let committed = output.committed.clone();
            Ok((
                EvalOutput {
                    stdout: output.stdout,
                    result: Some(output.value.to_json()),
                    error: None,
                },
                Some(committed),
            ))
        }
        EvaluationRace::Completed(Err(RuntimeError::Persistence(error))) => {
            interpreter.abandon_active_eval();
            Err(tm_core::Error::Sandbox(error))
        }
        EvaluationRace::Completed(Err(error)) => Ok((
            EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some(bounded_display(&error, budget.output_bytes)),
            },
            None,
        )),
        EvaluationRace::Cancelled => {
            cancel_active_bounded(interpreter, "cancelled", "cell cancelled").await?;
            Ok((cancelled_output(budget.output_bytes), None))
        }
        EvaluationRace::TimedOut => {
            cancel_active_bounded(interpreter, "timed_out", "cell exceeded wall-clock budget")
                .await?;
            Ok((timeout_output(budget.output_bytes), None))
        }
        EvaluationRace::TerminalPersistenceTimedOut => {
            let _ = cancel_active_bounded(
                interpreter,
                "failed",
                "terminal event persistence deadline exceeded",
            )
            .await;
            Err(tm_core::Error::Sandbox(
                "terminal event persistence deadline exceeded".into(),
            ))
        }
    }
}

fn cancelled_output(output_bytes: usize) -> EvalOutput {
    EvalOutput {
        stdout: String::new(),
        result: None,
        error: Some(bounded_display(
            &"CancellationError: cell cancelled",
            output_bytes,
        )),
    }
}

fn timeout_output(output_bytes: usize) -> EvalOutput {
    EvalOutput {
        stdout: String::new(),
        result: None,
        error: Some(bounded_display(
            &"TimeoutError: cell exceeded wall-clock budget",
            output_bytes,
        )),
    }
}

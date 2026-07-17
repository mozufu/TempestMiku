use std::{collections::HashSet, future::Future, sync::Arc, task::Poll};

use async_trait::async_trait;
use futures::{
    FutureExt, StreamExt,
    future::{BoxFuture, Either, select},
};

use crate::{
    Accumulator, CellBudget, ChatRequest, Error, EventSink, ExecuteCall, LlmClient, Message,
    Result, Sandbox, SessionConfig, StreamEvent, ToolChoice, ToolSpec,
    prompt::{DEFAULT_SYSTEM_PROMPT, TM_RUNTIME_BOOT_CONTRACT},
    shape::shape_result_capped,
};

/// Optional source of actor inbox messages for the agent loop.
///
/// Product layers can provide this without making `tm-core` depend on any
/// concrete actor crate. Each drained string is appended as a user message before
/// the next model turn.
#[async_trait]
pub trait InboxDrain: Send + Sync {
    async fn drain(&self) -> Result<Vec<String>>;
}

/// Optional cancellation check for long-running agent loops.
///
/// Product layers can provide their own token implementation while `tm-core`
/// only observes whether the current run should stop.
pub trait CancellationToken: Send + Sync {
    fn is_cancelled(&self) -> bool;

    /// Resolve when cancellation is requested.
    ///
    /// Implementations should override this with their native notification primitive. The
    /// compatibility default preserves existing tokens and repeatedly yields through the
    /// executor until [`CancellationToken::is_cancelled`] changes.
    fn cancelled(&self) -> BoxFuture<'_, ()> {
        futures::future::poll_fn(|cx| {
            if self.is_cancelled() {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .boxed()
    }
}

/// How the model is asked to run code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    /// Native function calling: the `execute` tool (design §5.2). The default.
    #[default]
    NativeTool,
    /// For endpoints without function calling: the model emits one fenced ```run block,
    /// the loop parses and runs it, and feeds the result back as a user message (design §5.3).
    FencedBlock,
}

/// Static configuration for one agent.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub system_prompt: String,
    pub max_turns: usize,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub protocol: Protocol,
    pub cell_budget: CellBudget,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            max_turns: 12,
            temperature: None,
            max_tokens: None,
            protocol: Protocol::NativeTool,
            cell_budget: CellBudget::default(),
        }
    }
}

/// Appended to the system prompt in [`Protocol::FencedBlock`] mode.
const FENCED_INSTRUCTIONS: &str = "\n\nThis endpoint has no function-calling. To run code, emit \
EXACTLY ONE fenced block:\n```run\n<your code>\n```\nThe runtime executes it and returns the \
result as the next message. Emit no fenced block when you have the final answer.";

enum TurnAction {
    Execute(Vec<ExecuteCall>),
    Final,
}

const MAX_NATIVE_EXECUTE_CALLS_PER_TURN: usize = 16;

/// The orchestrator: owns the message list and runs the streaming loop.
pub struct Agent {
    llm: Arc<dyn LlmClient>,
    sandbox: Arc<dyn Sandbox>,
    cfg: AgentConfig,
}

impl Agent {
    pub fn new(llm: Arc<dyn LlmClient>, sandbox: Arc<dyn Sandbox>, cfg: AgentConfig) -> Self {
        Self { llm, sandbox, cfg }
    }

    pub fn config(&self) -> &AgentConfig {
        &self.cfg
    }

    fn system_message(&self) -> Message {
        let mut prompt = String::from(TM_RUNTIME_BOOT_CONTRACT);
        if !self.cfg.system_prompt.trim().is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&self.cfg.system_prompt);
        }
        if self.cfg.protocol == Protocol::FencedBlock {
            prompt.push_str(FENCED_INSTRUCTIONS);
        }
        Message::system(prompt)
    }

    fn build_request(&self, messages: &[Message]) -> ChatRequest {
        let (tools, tool_choice) = match self.cfg.protocol {
            Protocol::NativeTool => (vec![ToolSpec::execute()], ToolChoice::Auto),
            Protocol::FencedBlock => (Vec::new(), ToolChoice::None),
        };
        ChatRequest {
            model: self.cfg.model.clone(),
            messages: messages.to_vec(),
            tools,
            tool_choice,
            temperature: self.cfg.temperature,
            max_tokens: self.cfg.max_tokens,
        }
    }

    /// Run the agent to a final answer, streaming events to `sink` as they arrive.
    pub async fn run(&self, user: &str, sink: &dyn EventSink) -> Result<String> {
        self.run_with_inbox(user, sink, None).await
    }

    /// Run the agent with an optional actor inbox drain.
    ///
    /// Pending inbox messages are appended before each model turn, preserving the
    /// existing streaming/tool loop while letting live actor messages wake the
    /// next turn without adding another runtime loop.
    ///
    pub async fn run_with_inbox(
        &self,
        user: &str,
        sink: &dyn EventSink,
        inbox: Option<&dyn InboxDrain>,
    ) -> Result<String> {
        self.run_with_controls(user, sink, inbox, None).await
    }

    /// Run the agent with optional product-layer controls.
    pub async fn run_with_controls(
        &self,
        user: &str,
        sink: &dyn EventSink,
        inbox: Option<&dyn InboxDrain>,
        cancellation: Option<&dyn CancellationToken>,
    ) -> Result<String> {
        self.run_with_prior_messages_and_controls(user, &[], sink, inbox, cancellation)
            .await
    }

    /// Open a fresh sandbox session, then run with caller-bounded prior conversation messages.
    pub async fn run_with_prior_messages_and_controls(
        &self,
        user: &str,
        prior_messages: &[Message],
        sink: &dyn EventSink,
        inbox: Option<&dyn InboxDrain>,
        cancellation: Option<&dyn CancellationToken>,
    ) -> Result<String> {
        check_cancelled(cancellation)?;
        let mut session =
            await_cancellable(self.sandbox.open(SessionConfig::default()), cancellation).await??;
        self.run_with_session_and_controls(
            user,
            prior_messages,
            session.as_mut(),
            sink,
            inbox,
            cancellation,
        )
        .await
    }

    /// Run against an already-open sandbox session with caller-bounded conversation history.
    ///
    /// `prior_messages` are inserted after the current system message and before the new user
    /// message. The caller owns history selection and must keep the slice within its context
    /// budget. Reusing `session` preserves REPL state across otherwise independent agent runs.
    pub async fn run_with_session(
        &self,
        user: &str,
        prior_messages: &[Message],
        session: &mut dyn crate::Session,
        sink: &dyn EventSink,
    ) -> Result<String> {
        self.run_with_session_and_controls(user, prior_messages, session, sink, None, None)
            .await
    }

    /// Run the single streaming/tool loop against an already-open session with all optional
    /// product-layer controls enabled.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_with_session_and_controls(
        &self,
        user: &str,
        prior_messages: &[Message],
        session: &mut dyn crate::Session,
        sink: &dyn EventSink,
        inbox: Option<&dyn InboxDrain>,
        cancellation: Option<&dyn CancellationToken>,
    ) -> Result<String> {
        let mut messages = Vec::with_capacity(prior_messages.len() + 2);
        messages.push(self.system_message());
        messages.extend_from_slice(prior_messages);
        messages.push(Message::user(user));

        for _ in 0..self.cfg.max_turns {
            check_cancelled(cancellation)?;
            if let Some(inbox) = inbox {
                let pending = await_cancellable(inbox.drain(), cancellation).await??;
                for message in pending {
                    check_cancelled(cancellation)?;
                    messages.push(Message::user(format!("[actor inbox]\n{message}")));
                }
            }

            let request = self.build_request(&messages);

            // Stream the turn; assistant tokens reach the sink the instant they land.
            let mut stream =
                await_cancellable(self.llm.chat_stream(&request), cancellation).await??;
            let mut acc = Accumulator::new();
            while let Some(ev) = await_cancellable(stream.next(), cancellation).await? {
                check_cancelled(cancellation)?;
                let ev = ev?;
                match &ev {
                    StreamEvent::Reasoning(r) => sink.try_on_reasoning(r)?,
                    StreamEvent::Text(t) => sink.try_on_text(t)?,
                    StreamEvent::ToolCall {
                        name: Some(name), ..
                    } if !name.is_empty() => sink.try_on_tool_call(name)?,
                    _ => {}
                }
                acc.push(ev);
            }
            sink.try_on_turn_end()?;

            let turn = acc.into_turn();
            let action = match self.cfg.protocol {
                Protocol::NativeTool => validate_native_turn(&turn)?,
                Protocol::FencedBlock => {
                    validate_completion_state(&turn)?;
                    if !turn.tool_calls.is_empty() {
                        return Err(Error::Protocol(
                            "fenced-block endpoint returned native tool calls".to_string(),
                        ));
                    }
                    match parse_fenced(&turn.text)? {
                        Some(code) => TurnAction::Execute(vec![ExecuteCall {
                            id: String::new(),
                            code,
                        }]),
                        None => TurnAction::Final,
                    }
                }
            };
            messages.push(turn.to_message());

            let calls = match action {
                TurnAction::Final => {
                    sink.try_on_final(&turn.text)?;
                    return Ok(turn.text);
                }
                TurnAction::Execute(calls) => calls,
            };

            for call in &calls {
                check_cancelled(cancellation)?;
                sink.try_on_cell_start(&call.code)?;
            }
            let codes = calls
                .iter()
                .map(|call| call.code.clone())
                .collect::<Vec<_>>();
            let outputs = if session.handles_cancellation() {
                // Stateful runtimes must finish their own cancellation/terminal-event protocol.
                // Racing this future from the outside could drop it between an in-memory commit
                // and its durable event (or vice versa).
                session.eval_batch(&codes, self.cfg.cell_budget).await?
            } else {
                await_cancellable(
                    session.eval_batch(&codes, self.cfg.cell_budget),
                    cancellation,
                )
                .await??
            };
            if outputs.len() != calls.len() {
                return Err(Error::Sandbox(format!(
                    "session returned {} results for {} execute calls",
                    outputs.len(),
                    calls.len()
                )));
            }
            for (call, out) in calls.into_iter().zip(outputs) {
                check_cancelled(cancellation)?;
                let shaped = shape_result_capped(&out, self.cfg.cell_budget.output_bytes);
                sink.try_on_cell_result(&shaped)?;

                match self.cfg.protocol {
                    Protocol::NativeTool => messages.push(Message::tool_result(&call.id, shaped)),
                    Protocol::FencedBlock => {
                        messages.push(Message::user(format!("[execution result]\n{shaped}")))
                    }
                }
            }
        }

        Err(Error::TurnBudget(self.cfg.max_turns))
    }
}

fn check_cancelled(cancellation: Option<&dyn CancellationToken>) -> Result<()> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

async fn await_cancellable<F, T>(
    future: F,
    cancellation: Option<&dyn CancellationToken>,
) -> Result<T>
where
    F: Future<Output = T>,
{
    let Some(cancellation) = cancellation else {
        return Ok(future.await);
    };
    check_cancelled(Some(cancellation))?;
    futures::pin_mut!(future);
    let cancelled = cancellation.cancelled();
    futures::pin_mut!(cancelled);
    match select(future, cancelled).await {
        Either::Left((output, _)) => Ok(output),
        Either::Right(((), _)) => Err(Error::Cancelled),
    }
}

fn validate_completion_state(turn: &crate::AssistantTurn) -> Result<()> {
    match turn.finish_reason.as_deref() {
        Some("length") => {
            return Err(Error::Protocol(
                "completion ended at the token limit".to_string(),
            ));
        }
        Some("content_filter") => {
            return Err(Error::Protocol(
                "completion was stopped by a content filter".to_string(),
            ));
        }
        _ => {}
    }
    if turn.text.is_empty() && turn.tool_calls.is_empty() && turn.finish_reason.is_none() {
        return Err(Error::Protocol(
            "completion stream ended without content, tool calls, or a finish reason".to_string(),
        ));
    }
    Ok(())
}

fn validate_native_turn(turn: &crate::AssistantTurn) -> Result<TurnAction> {
    validate_completion_state(turn)?;
    if turn.tool_calls.len() > MAX_NATIVE_EXECUTE_CALLS_PER_TURN {
        return Err(Error::Protocol(format!(
            "expected at most {MAX_NATIVE_EXECUTE_CALLS_PER_TURN} execute tool calls, received {}",
            turn.tool_calls.len()
        )));
    }
    if turn.tool_calls.is_empty() {
        if turn.finish_reason.as_deref() == Some("tool_calls") {
            return Err(Error::Protocol(
                "completion reported tool_calls without a complete tool call".to_string(),
            ));
        }
        return Ok(TurnAction::Final);
    }

    let mut ids = HashSet::with_capacity(turn.tool_calls.len());
    let mut calls = Vec::with_capacity(turn.tool_calls.len());
    for call in &turn.tool_calls {
        if call.id.trim().is_empty() {
            return Err(Error::Protocol(format!(
                "tool {} is missing a call id",
                call.name
            )));
        }
        if !ids.insert(call.id.as_str()) {
            return Err(Error::Protocol(format!(
                "duplicate tool call id {}",
                call.id
            )));
        }
        if call.name != "execute" {
            return Err(Error::Protocol(format!(
                "model returned non-execute tool {}",
                call.name
            )));
        }
        let arguments = call.arguments.as_object().ok_or_else(|| {
            Error::Protocol(format!(
                "tool {} arguments are not a JSON object",
                call.name
            ))
        })?;
        if arguments.len() != 1 || !arguments.contains_key("code") {
            return Err(Error::Protocol(
                "execute accepts exactly one code argument".to_string(),
            ));
        }
        let code = arguments
            .get("code")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                Error::Protocol("execute requires a string code argument".to_string())
            })?;
        if code.trim().is_empty() {
            return Err(Error::Protocol(
                "execute requires non-empty code".to_string(),
            ));
        }
        calls.push(ExecuteCall {
            id: call.id.clone(),
            code: code.to_string(),
        });
    }
    Ok(TurnAction::Execute(calls))
}

/// Extract one explicitly executable `run` fence only when it is the response's sole complete
/// fenced block. Mixed or malformed fenced content fails closed instead of letting prose smuggle
/// an executable block alongside another block.
fn parse_fenced(text: &str) -> Result<Option<String>> {
    let mut blocks = Vec::<(String, String)>::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in text.lines() {
        let marker = line.trim();
        match &mut current {
            Some((label, body)) if marker == "```" => {
                let code = body.join("\n").trim().to_string();
                if code.is_empty() {
                    return Err(Error::Protocol(
                        "fenced block contains no content".to_string(),
                    ));
                }
                blocks.push((std::mem::take(label), code));
                current = None;
            }
            Some(_) if marker.starts_with("```") => {
                return Err(Error::Protocol(
                    "fenced block has a malformed closing marker".to_string(),
                ));
            }
            Some((_, body)) => body.push(line),
            None if marker.starts_with("```") => {
                current = Some((marker[3..].to_string(), Vec::new()));
            }
            None => {}
        }
    }
    if current.is_some() {
        return Err(Error::Protocol("unterminated fenced block".to_string()));
    }
    match blocks.as_slice() {
        [] => Ok(None),
        [(label, code)] if label == "run" => Ok(Some(code.clone())),
        [_, _, ..] => Err(Error::Protocol(format!(
            "expected at most one fenced block, received {}",
            blocks.len()
        ))),
        [_] => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_run_fence() {
        let text = "Sure, let me check.\n```run\ndisplay(1 + 1);\n```\nDone.";
        assert_eq!(
            parse_fenced(text).unwrap().as_deref(),
            Some("display(1 + 1);")
        );
    }

    #[test]
    fn no_fence_is_none() {
        assert_eq!(parse_fenced("just prose, the final answer").unwrap(), None);
    }

    #[test]
    fn empty_fence_is_protocol_error() {
        assert!(matches!(
            parse_fenced("```run\n\n```"),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn json_and_javascript_fences_are_not_executable() {
        assert_eq!(parse_fenced("```json\n{\"ok\":true}\n```").unwrap(), None);
        assert_eq!(parse_fenced("```js\ndisplay('no')\n```").unwrap(), None);
    }

    #[test]
    fn non_run_fence_plus_run_fence_is_rejected() {
        assert!(matches!(
            parse_fenced("```js\ndisplay('decoy')\n```\n```run\ndisplay('unsafe')\n```"),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn malformed_fence_plus_run_fence_is_rejected() {
        assert!(matches!(
            parse_fenced("```json\n{\"open\":true}\n```run\ndisplay('unsafe')\n```"),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn multiple_or_unterminated_run_fences_are_protocol_errors() {
        assert!(matches!(
            parse_fenced("```run\n1\n```\n```run\n2\n```"),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(parse_fenced("```run\n1"), Err(Error::Protocol(_))));
    }
}

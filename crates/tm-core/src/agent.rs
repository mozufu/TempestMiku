use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;

use crate::{
    Accumulator, CellBudget, ChatRequest, Error, EventSink, ExecuteCall, LlmClient, Message,
    Result, Sandbox, SessionConfig, StreamEvent, ToolChoice, ToolMediator, ToolSpec,
    prompt::DEFAULT_SYSTEM_PROMPT, shape::shape_result,
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
        let mut prompt = self.cfg.system_prompt.clone();
        if self.cfg.protocol == Protocol::FencedBlock {
            prompt.push_str(FENCED_INSTRUCTIONS);
        }
        Message::system(prompt)
    }

    fn build_request(&self, messages: &[Message], extra_tools: &[ToolSpec]) -> ChatRequest {
        let (tools, tool_choice) = match self.cfg.protocol {
            Protocol::NativeTool => (
                std::iter::once(ToolSpec::execute())
                    .chain(extra_tools.iter().cloned())
                    .collect(),
                ToolChoice::Auto,
            ),
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
        self.run_with_inbox(user, sink, None, None).await
    }

    /// Run the agent with an optional actor inbox drain and an optional [`ToolMediator`].
    ///
    /// Pending inbox messages are appended before each model turn, preserving the
    /// existing streaming/tool loop while letting live actor messages wake the
    /// next turn without adding another runtime loop.
    ///
    /// A mediator's tool specs are advertised alongside `execute`; when the model calls one
    /// of them instead of `execute`, the loop hands the call to the mediator and feeds its
    /// returned string back as that call's tool result, then continues to the next turn —
    /// `tm-core` never learns what the mediated tool actually does.
    pub async fn run_with_inbox(
        &self,
        user: &str,
        sink: &dyn EventSink,
        inbox: Option<&dyn InboxDrain>,
        mediator: Option<&dyn ToolMediator>,
    ) -> Result<String> {
        self.run_with_controls(user, sink, inbox, mediator, None)
            .await
    }

    /// Run the agent with optional product-layer controls.
    pub async fn run_with_controls(
        &self,
        user: &str,
        sink: &dyn EventSink,
        inbox: Option<&dyn InboxDrain>,
        mediator: Option<&dyn ToolMediator>,
        cancellation: Option<&dyn CancellationToken>,
    ) -> Result<String> {
        let mut messages = vec![self.system_message(), Message::user(user)];
        let mut session = self.sandbox.open(SessionConfig::default()).await?;
        let extra_tools = mediator.map(|m| m.tool_specs()).unwrap_or_default();

        for _ in 0..self.cfg.max_turns {
            check_cancelled(cancellation)?;
            if let Some(inbox) = inbox {
                for message in inbox.drain().await? {
                    check_cancelled(cancellation)?;
                    messages.push(Message::user(format!("[actor inbox]\n{message}")));
                }
            }

            let request = self.build_request(&messages, &extra_tools);

            // Stream the turn; assistant tokens reach the sink the instant they land.
            let mut stream = self.llm.chat_stream(&request).await?;
            let mut acc = Accumulator::new();
            while let Some(ev) = stream.next().await {
                check_cancelled(cancellation)?;
                let ev = ev?;
                match &ev {
                    StreamEvent::Text(t) => sink.on_text(t),
                    StreamEvent::ToolCall {
                        name: Some(name), ..
                    } if !name.is_empty() => sink.on_tool_call(name),
                    _ => {}
                }
                acc.push(ev);
            }
            sink.on_turn_end();

            let turn = acc.into_turn();
            messages.push(turn.to_message());

            // Decide whether the model wants to run code, per protocol.
            let call = match self.cfg.protocol {
                Protocol::NativeTool => turn.execute_call(),
                Protocol::FencedBlock => parse_fenced(&turn.text).map(|code| ExecuteCall {
                    id: String::new(),
                    code,
                }),
            };

            let Some(call) = call else {
                // No `execute` call this turn. If a mediator is attached and the model
                // called one of its tools instead, hand it off and keep looping; otherwise
                // this is the final answer.
                let mediated = mediator.zip(turn.tool_calls.iter().find(|tc| tc.name != "execute"));
                let Some((mediator, tool_call)) = mediated else {
                    sink.on_final(&turn.text);
                    return Ok(turn.text);
                };
                let result = mediator
                    .handle(&tool_call.name, &tool_call.arguments)
                    .await
                    .unwrap_or_else(|err| format!("{} failed: {err}", tool_call.name));
                messages.push(Message::tool_result(&tool_call.id, result));
                continue;
            };

            check_cancelled(cancellation)?;
            sink.on_cell_start(&call.code);
            let out = session.eval(&call.code, self.cfg.cell_budget).await?;
            check_cancelled(cancellation)?;
            let shaped = shape_result(&out);
            sink.on_cell_result(&shaped);

            match self.cfg.protocol {
                Protocol::NativeTool => messages.push(Message::tool_result(&call.id, shaped)),
                Protocol::FencedBlock => {
                    messages.push(Message::user(format!("[execution result]\n{shaped}")))
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

/// Extract the body of the first fenced code block the loop knows how to run. Recognized
/// fences, in priority order: ```run, ```tm, then the common language fences.
fn parse_fenced(text: &str) -> Option<String> {
    const FENCES: &[&str] = &[
        "```run",
        "```tm",
        "```ts",
        "```typescript",
        "```js",
        "```javascript",
    ];
    for fence in FENCES {
        let Some(open) = text.find(fence) else {
            continue;
        };
        let after = &text[open + fence.len()..];
        // Skip the rest of the opening fence line.
        let body_start = after.find('\n').map(|i| i + 1).unwrap_or(after.len());
        let body = &after[body_start..];
        if let Some(end) = body.find("```") {
            let code = body[..end].trim();
            if !code.is_empty() {
                return Some(code.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_run_fence() {
        let text = "Sure, let me check.\n```run\ndisplay(1 + 1);\n```\nDone.";
        assert_eq!(parse_fenced(text).as_deref(), Some("display(1 + 1);"));
    }

    #[test]
    fn no_fence_is_none() {
        assert_eq!(parse_fenced("just prose, the final answer"), None);
    }

    #[test]
    fn empty_fence_is_none() {
        assert_eq!(parse_fenced("```run\n\n```"), None);
    }
}

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{Message, Role, ToolCall, Usage};

/// One incremental event from a streamed completion. The transport ([`crate::LlmClient`])
/// emits these as bytes arrive; the loop forwards text to the UI and feeds everything to an
/// [`Accumulator`].
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// A fragment of private assistant reasoning (chain-of-thought). Some OpenAI-compatible
    /// providers surface reasoning tokens separately from the visible answer via
    /// `delta.reasoning` / `delta.reasoning_content`; we forward them so a UI can render a
    /// collapsible "thinking" trace when the provider returns one. The fragments concatenate
    /// like [`StreamEvent::Text`] do.
    Reasoning(String),
    /// A fragment of assistant text.
    Text(String),
    /// A fragment of a tool call. `id`/`name` arrive once (usually first); `arguments`
    /// streams in pieces that concatenate into a JSON string.
    ToolCall {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
    /// The choice finished; carries the provider's `finish_reason` when present.
    Finish { reason: Option<String> },
    /// Token usage, typically on the final chunk.
    Usage(Usage),
}

#[derive(Debug, Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Stitches a stream of [`StreamEvent`]s into one [`AssistantTurn`].
#[derive(Debug, Default)]
pub struct Accumulator {
    text: String,
    reasoning: String,
    calls: BTreeMap<usize, PartialToolCall>,
    finish_reason: Option<String>,
    usage: Option<Usage>,
}

impl Accumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one event into the in-progress turn.
    pub fn push(&mut self, ev: StreamEvent) {
        match ev {
            StreamEvent::Reasoning(r) => self.reasoning.push_str(&r),
            StreamEvent::Text(t) => self.text.push_str(&t),
            StreamEvent::ToolCall {
                index,
                id,
                name,
                arguments,
            } => {
                let slot = self.calls.entry(index).or_default();
                if let Some(id) = id
                    && !id.is_empty()
                {
                    slot.id = id;
                }
                if let Some(name) = name
                    && !name.is_empty()
                {
                    slot.name = name;
                }
                if let Some(args) = arguments {
                    slot.arguments.push_str(&args);
                }
            }
            StreamEvent::Finish { reason } => self.finish_reason = reason,
            StreamEvent::Usage(u) => self.usage = Some(u),
        }
    }

    /// Finalize. Tool-call argument strings are parsed to JSON; an unparseable blob is kept
    /// verbatim as a JSON string so the loop can still surface it rather than crash.
    pub fn into_turn(self) -> AssistantTurn {
        let tool_calls = self
            .calls
            .into_values()
            .filter(|c| !c.name.is_empty())
            .map(|c| {
                let arguments = if c.arguments.trim().is_empty() {
                    Value::Object(serde_json::Map::new())
                } else {
                    serde_json::from_str(&c.arguments).unwrap_or(Value::String(c.arguments))
                };
                ToolCall {
                    id: c.id,
                    name: c.name,
                    arguments,
                }
            })
            .collect();

        AssistantTurn {
            text: self.text,
            reasoning: self.reasoning,
            tool_calls,
            finish_reason: self.finish_reason,
            usage: self.usage,
        }
    }
}

/// A complete assistant turn assembled from a delta stream.
#[derive(Debug, Clone, PartialEq)]
pub struct AssistantTurn {
    pub text: String,
    /// Private chain-of-thought the provider returned alongside `text`. Never re-sent to the
    /// model; surfaced to the UI only.
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
}

/// A resolved `execute` call: the tool-call id plus the extracted `code`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecuteCall {
    pub id: String,
    pub code: String,
}

impl AssistantTurn {
    /// Project back into a [`Message`] to append to the running conversation.
    pub fn to_message(&self) -> Message {
        Message {
            role: Role::Assistant,
            content: self.text.clone(),
            tool_calls: self.tool_calls.clone(),
            tool_call_id: None,
        }
    }

    /// The `execute` call in this turn, if any, with its `code` argument extracted.
    pub fn execute_call(&self) -> Option<ExecuteCall> {
        self.tool_calls
            .iter()
            .find(|c| c.name == "execute")
            .map(|c| ExecuteCall {
                id: c.id.clone(),
                code: c
                    .arguments
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
    }
}
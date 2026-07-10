use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Conversation roles, matching the OpenAI chat schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// A resolved tool call. During streaming, argument fragments are stitched together by
/// [`crate::Accumulator`]; by the time a `ToolCall` exists, `arguments` is parsed JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// One chat message. `content` holds plain text (rich content blocks land with the real
/// sandbox in M1); `tool_calls` is set on assistant turns; `tool_call_id` keys a tool result.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
}

impl Message {
    fn bare(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::bare(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::bare(Role::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::bare(Role::Assistant, content)
    }

    /// A `role: "tool"` message carrying the shaped result of an `execute` call.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// Tool-selection policy sent to the endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolChoice {
    Auto,
    None,
    Required,
}

impl ToolChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolChoice::Auto => "auto",
            ToolChoice::None => "none",
            ToolChoice::Required => "required",
        }
    }
}

/// The `function` body of a tool spec; serializes straight to the OpenAI wire shape.
#[derive(Debug, Clone, Serialize)]
pub struct FunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// A tool definition. TempestMiku exposes exactly one — [`ToolSpec::execute`].
#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionSpec,
}

impl ToolSpec {
    /// The single `execute(code)` tool that is the model's entire tool surface (design §5.2).
    pub fn execute() -> Self {
        ToolSpec {
            kind: "function".into(),
            function: FunctionSpec {
                name: "execute".into(),
                description: "Run JavaScript/TypeScript in your persistent sandbox REPL. \
                    Variables persist across calls and top-level await is supported. Only what you \
                    display()/return reaches you; everything else stays in the sandbox. Discover \
                    capabilities with await tools.search()/tools.docs()."
                    .into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "code": {
                            "type": "string",
                            "description": "Source to evaluate in the session."
                        }
                    },
                    "required": ["code"]
                }),
            },
        }
    }
}

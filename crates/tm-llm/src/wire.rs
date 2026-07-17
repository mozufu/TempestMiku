//! OpenAI chat-completions wire mapping: TempestMiku types <-> JSON.

use serde::Deserialize;
use serde_json::{Map, Value};

use tm_core::{ChatRequest, Message, Role, Usage};

/// Build the JSON request body. `stream` is always true (streaming is the one transport).
pub fn build_body(req: &ChatRequest, include_usage: bool, reasoning_effort: Option<&str>) -> Value {
    let messages: Vec<Value> = req.messages.iter().map(wire_message).collect();

    let mut body = serde_json::json!({
        "model": req.model,
        "messages": messages,
        "stream": true,
    });
    let map = body.as_object_mut().expect("object literal");

    if !req.tools.is_empty() {
        map.insert(
            "tools".into(),
            serde_json::to_value(&req.tools).expect("tool spec serializes"),
        );
        map.insert(
            "tool_choice".into(),
            Value::String(req.tool_choice.as_str().into()),
        );
        // tm-core prevalidates and bounds the complete batch before the session executes it.
        map.insert("parallel_tool_calls".into(), Value::Bool(true));
    }
    if include_usage {
        map.insert(
            "stream_options".into(),
            serde_json::json!({ "include_usage": true }),
        );
    }
    if let Some(effort) = reasoning_effort {
        map.insert("reasoning_effort".into(), Value::String(effort.to_string()));
    }
    if let Some(t) = req.temperature {
        map.insert("temperature".into(), serde_json::json!(t));
    }
    if let Some(m) = req.max_tokens {
        map.insert("max_tokens".into(), serde_json::json!(m));
    }
    body
}

#[cfg(test)]
mod tests {
    use tm_core::{ToolChoice, ToolSpec};

    use super::*;

    #[test]
    fn includes_requested_reasoning_effort() {
        let body = build_body(
            &ChatRequest {
                model: "gpt-5.6-sol".to_string(),
                messages: Vec::new(),
                tools: vec![ToolSpec::execute()],
                tool_choice: ToolChoice::Auto,
                temperature: None,
                max_tokens: None,
            },
            true,
            Some("medium"),
        );

        assert_eq!(body["model"], "gpt-5.6-sol");
        assert_eq!(body["reasoning_effort"], "medium");
        assert_eq!(body["parallel_tool_calls"], true);
    }
}

/// Convert one [`Message`] to its OpenAI wire object. Assistant tool-call arguments are
/// emitted as a JSON *string*, as the API requires.
fn wire_message(m: &Message) -> Value {
    let mut obj = Map::new();
    obj.insert("role".into(), Value::String(m.role.as_str().into()));

    match m.role {
        Role::Assistant if !m.tool_calls.is_empty() => {
            obj.insert(
                "content".into(),
                if m.content.is_empty() {
                    Value::Null
                } else {
                    Value::String(m.content.clone())
                },
            );
            let calls: Vec<Value> = m
                .tool_calls
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "type": "function",
                        "function": {
                            "name": c.name,
                            "arguments": serde_json::to_string(&c.arguments)
                                .unwrap_or_else(|_| "{}".into()),
                        }
                    })
                })
                .collect();
            obj.insert("tool_calls".into(), Value::Array(calls));
        }
        Role::Tool => {
            obj.insert("content".into(), Value::String(m.content.clone()));
            if let Some(id) = &m.tool_call_id {
                obj.insert("tool_call_id".into(), Value::String(id.clone()));
            }
        }
        _ => {
            obj.insert("content".into(), Value::String(m.content.clone()));
        }
    }

    Value::Object(obj)
}

// --- streamed-chunk deserialization ---

#[derive(Debug, Deserialize)]
pub struct Chunk {
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    #[serde(default)]
    pub delta: Delta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Delta {
    #[serde(default)]
    pub content: Option<String>,
    /// Private chain-of-thought. OpenAI reasoning models use `reasoning`; DeepSeek R1 and
    /// several OpenAI-compatible bridges use `reasoning_content`. We accept either and
    /// forward whichever the provider sends.
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallDelta>,
}

#[derive(Debug, Deserialize)]
pub struct ToolCallDelta {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<FnDelta>,
}

#[derive(Debug, Deserialize)]
pub struct FnDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

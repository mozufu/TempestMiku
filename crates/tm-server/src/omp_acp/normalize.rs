use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, Diff, SessionUpdate, TextContent, ToolCall, ToolCallContent,
    ToolCallUpdate,
};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NormalizedOmpEvent {
    pub event_type: String,
    pub payload: Value,
    pub transcript_text: Option<String>,
}

pub(crate) fn normalize_session_update(update: &SessionUpdate) -> Vec<NormalizedOmpEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(ContentChunk {
            content: ContentBlock::Text(TextContent { text, .. }),
            ..
        }) => vec![NormalizedOmpEvent {
            event_type: "text".to_string(),
            payload: json!({ "event": "text", "delta": text }),
            transcript_text: Some(text.clone()),
        }],
        SessionUpdate::ToolCall(tool_call) => {
            let raw = raw_json(update);
            let mut events = vec![NormalizedOmpEvent {
                event_type: "tool_call".to_string(),
                payload: json!({ "backend": "omp-acp", "toolCall": raw["toolCall"].clone() }),
                transcript_text: None,
            }];
            events.extend(diff_events_for_tool_call(tool_call));
            events
        }
        SessionUpdate::ToolCallUpdate(tool_call_update) => {
            let raw = raw_json(update);
            let mut events = vec![NormalizedOmpEvent {
                event_type: "tool_call_update".to_string(),
                payload: json!({ "backend": "omp-acp", "toolCall": raw["toolCallUpdate"].clone() }),
                transcript_text: None,
            }];
            events.extend(diff_events_for_tool_call_update(tool_call_update));
            events
        }
        SessionUpdate::AgentThoughtChunk(_)
        | SessionUpdate::Plan(_)
        | SessionUpdate::AvailableCommandsUpdate(_)
        | SessionUpdate::CurrentModeUpdate(_)
        | SessionUpdate::ConfigOptionUpdate(_)
        | SessionUpdate::SessionInfoUpdate(_)
        | SessionUpdate::UsageUpdate(_) => vec![NormalizedOmpEvent {
            event_type: "progress".to_string(),
            payload: json!({ "backend": "omp-acp", "update": raw_json(update) }),
            transcript_text: None,
        }],
        _ => vec![NormalizedOmpEvent {
            event_type: "progress".to_string(),
            payload: json!({ "backend": "omp-acp", "update": raw_json(update) }),
            transcript_text: None,
        }],
    }
}

fn diff_events_for_tool_call(tool_call: &ToolCall) -> Vec<NormalizedOmpEvent> {
    tool_call
        .content
        .iter()
        .filter_map(|content| diff_event(&tool_call.tool_call_id.to_string(), content))
        .collect()
}

fn diff_events_for_tool_call_update(tool_call_update: &ToolCallUpdate) -> Vec<NormalizedOmpEvent> {
    tool_call_update
        .fields
        .content
        .as_ref()
        .into_iter()
        .flatten()
        .filter_map(|content| diff_event(&tool_call_update.tool_call_id.to_string(), content))
        .collect()
}

fn diff_event(tool_call_id: &str, content: &ToolCallContent) -> Option<NormalizedOmpEvent> {
    let ToolCallContent::Diff(Diff {
        path,
        old_text,
        new_text,
        ..
    }) = content
    else {
        return None;
    };
    Some(NormalizedOmpEvent {
        event_type: "diff".to_string(),
        payload: json!({
            "backend": "omp-acp",
            "toolCallId": tool_call_id,
            "path": path,
            "oldText": old_text,
            "newText": new_text,
        }),
        transcript_text: None,
    })
}

fn raw_json<T: serde::Serialize + std::fmt::Debug>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|_| json!({ "debug": format!("{value:?}") }))
}

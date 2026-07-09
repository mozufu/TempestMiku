use std::collections::BTreeMap;

use serde_json::Value;
use tm_memory::DreamInputMessage;

use crate::SessionEvent;

pub(super) struct RedactedMessage {
    pub(super) seq: i64,
    pub(super) role: String,
    pub(super) content: String,
    pub(super) had_redactions: bool,
}

impl From<&DreamInputMessage> for RedactedMessage {
    fn from(message: &DreamInputMessage) -> Self {
        Self {
            seq: message.seq,
            role: message.role.clone(),
            content: message.content.clone(),
            had_redactions: message.had_redactions,
        }
    }
}

pub(super) fn event_uris(events: &[SessionEvent]) -> Vec<String> {
    let mut uris = Vec::new();
    for event in events {
        if let Some(uri) = event.artifact_uri.as_ref().or(event.history_uri.as_ref()) {
            uris.push(uri.clone());
        }
        collect_uri_strings(&event.payload_json, &mut uris);
    }
    uris.sort();
    uris.dedup();
    uris.into_iter().take(8).collect()
}

fn collect_uri_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if looks_like_resource_uri(text) => out.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                collect_uri_strings(item, out);
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                collect_uri_strings(value, out);
            }
        }
        _ => {}
    }
}

fn looks_like_resource_uri(text: &str) -> bool {
    [
        "artifact://",
        "workspace://",
        "linked://",
        "project://",
        "memory://",
        "agent://",
        "history://",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
}

pub(super) fn message_lines_with(messages: &[RedactedMessage], needles: &[&str]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| message.content.lines())
        .map(str::trim)
        .filter(|line| {
            let lower = line.to_lowercase();
            needles.iter().any(|needle| lower.contains(needle))
        })
        .map(|line| preview_text(line, 180))
        .take(6)
        .collect()
}

pub(super) fn unresolved_approvals(events: &[SessionEvent]) -> Vec<String> {
    let mut approvals = BTreeMap::<String, String>::new();
    for event in events {
        match event.event_type.as_str() {
            "approval" => {
                if let Some(id) = event.payload_json.get("approvalId").and_then(Value::as_str) {
                    approvals.insert(
                        id.to_string(),
                        event
                            .payload_json
                            .get("action")
                            .and_then(Value::as_str)
                            .unwrap_or("approval")
                            .to_string(),
                    );
                }
            }
            "approval_resolved" => {
                if let Some(id) = event.payload_json.get("approvalId").and_then(Value::as_str) {
                    approvals.remove(id);
                }
            }
            _ => {}
        }
    }
    approvals.into_values().take(6).collect()
}

pub(super) fn reusable_workflow_signal(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "workflow:",
        "playbook:",
        "reusable workflow",
        "reuse this workflow",
        "my process is",
        "our process is",
        "when i ask",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub(super) fn skill_name(scenario: &str) -> String {
    let normalized = normalize_key(scenario);
    let suffix = normalized
        .split('-')
        .filter(|part| !part.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join("-");
    if suffix.is_empty() {
        "dream-distilled-workflow".to_string()
    } else {
        format!("dream-{suffix}")
    }
}

pub(super) fn normalize_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

pub(super) fn preview_text(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut out = compact
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct E2eEvent {
    pub id: Option<i64>,
    pub event_type: String,
    pub data: Value,
}

pub fn parse_sse_block(block: &str) -> Result<Option<E2eEvent>> {
    let mut id = None;
    let mut event_type = None;
    let mut data_lines = Vec::new();

    for raw in block.lines() {
        let line = raw.trim_end();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.trim().parse::<i64>().context("invalid SSE id")?);
        } else if let Some(value) = line.strip_prefix("event:") {
            event_type = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim_start().to_string());
        }
    }

    if id.is_none() && event_type.is_none() && data_lines.is_empty() {
        return Ok(None);
    }

    let data_text = data_lines.join("\n");
    let data = if data_text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&data_text).unwrap_or(Value::String(data_text))
    };
    ensure!(
        event_type.as_deref() == Some("session_event"),
        "unsupported SSE event name {}",
        event_type.as_deref().unwrap_or("message")
    );
    let Value::Object(mut envelope) = data else {
        bail!("session_event data must be an object");
    };
    let event_type = envelope
        .remove("type")
        .and_then(|value| value.as_str().map(str::to_string))
        .context("session_event envelope is missing type")?;
    let payload = envelope
        .remove("payload")
        .context("session_event envelope is missing payload")?;
    ensure!(
        envelope.get("createdAt").and_then(Value::as_str).is_some(),
        "session_event envelope is missing createdAt"
    );
    ensure!(
        envelope
            .get("turnId")
            .is_some_and(|value| value.is_null() || value.is_string()),
        "session_event envelope has invalid turnId"
    );
    Ok(Some(E2eEvent {
        id,
        event_type,
        data: payload,
    }))
}

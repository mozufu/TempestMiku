use std::{collections::BTreeMap, env, path::PathBuf};

use chrono::Utc;
use serde_json::Value;

pub fn default_run_dir(label: &str) -> PathBuf {
    let stamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    PathBuf::from("target")
        .join("tm-e2e")
        .join("runs")
        .join(format!("{stamp}-{label}"))
}

pub fn timestamp() -> String {
    Utc::now().to_rfc3339()
}

pub fn redact_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut redacted = serde_json::Map::new();
            for (key, value) in map {
                if is_secret_key(key) {
                    redacted.insert(key.clone(), Value::String("[REDACTED]".to_string()));
                } else {
                    redacted.insert(key.clone(), redact_json(value));
                }
            }
            Value::Object(redacted)
        }
        Value::Array(values) => Value::Array(values.iter().map(redact_json).collect()),
        Value::String(value) => Value::String(redact_string(value)),
        other => other.clone(),
    }
}

fn redact_string(value: &str) -> String {
    if value.to_ascii_lowercase().contains("bearer ") {
        return "[REDACTED]".to_string();
    }
    value.to_string()
}

fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.contains("token")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("secret")
        || lower.contains("authorization")
        || lower.contains("password")
}

pub(super) fn truncate_json(value: &Value) -> Value {
    match value {
        Value::String(text) if text.len() > 8_192 => {
            Value::String(format!("{}...[truncated]", &text[..8_192]))
        }
        Value::Array(items) => Value::Array(items.iter().map(truncate_json).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), truncate_json(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub(super) fn sanitized_environment() -> BTreeMap<String, String> {
    let mut env_out = BTreeMap::new();
    for key in [
        "TM_MIKU_BASE_URL",
        "TM_MIKU_BEARER_TOKEN",
        "TM_MIKU_TOKEN",
        "TM_MIKU_E2E_TIMEOUT_MS",
        "TM_E2E_REQUIRE_ARTIFACT",
        "TM_LLM_E2E_LIVE",
        "TM_E2E_SPEAKER_MODEL",
        "OPENAI_API_KEY",
        "OPENAI_MODEL",
        "OPENAI_BASE_URL",
        "TM_OMP_ACP_ENABLED",
        "TM_DATABASE_URL",
    ] {
        if let Ok(value) = env::var(key) {
            let value = if is_secret_key(key) {
                "[REDACTED]".to_string()
            } else if value.trim().is_empty() {
                String::new()
            } else {
                redact_string(&value)
            };
            env_out.insert(key.to_string(), value);
        }
    }
    env_out
}

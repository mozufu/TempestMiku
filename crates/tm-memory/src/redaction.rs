use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RedactionReport {
    pub text: String,
    pub redactions: Vec<Redaction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Redaction {
    pub kind: String,
    pub count: usize,
}

static REDACTION_RULES: LazyLock<Vec<(&'static str, Regex, &'static str)>> = LazyLock::new(|| {
    [
        (
            "private_key",
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            "[REDACTED_PRIVATE_KEY]",
        ),
        (
            "authorization_header",
            r"(?im)^(authorization|x-api-key|api-key|x-tempestmiku-bootstrap)\s*:\s*.+$",
            "$1: [REDACTED_SECRET]",
        ),
        (
            "tempestmiku_device_token",
            r"\btmk_dev_[A-Fa-f0-9]{64}\b",
            "[REDACTED_DEVICE_TOKEN]",
        ),
        (
            "tempestmiku_pairing_link",
            r#"tempestmiku://pair\?[^\s"'<>]*"#,
            "[REDACTED_PAIRING_LINK]",
        ),
        (
            "pairing_code",
            r#"(?i)\b(code|pairing[_-]?code)\s*[:=]\s*['\"]?[a-f0-9]{64}"#,
            "$1=[REDACTED_PAIRING_CODE]",
        ),
        (
            "bearer_token",
            r"(?i)\bbearer\s+[A-Za-z0-9._~+/-]{12,}=*",
            "Bearer [REDACTED_TOKEN]",
        ),
        (
            "uri_userinfo",
            r"(?i)\b(postgres(?:ql)?|https?)://[^/\s:@]+:[^@\s/]+@",
            "$1://[REDACTED_CREDENTIAL]@",
        ),
        (
            "api_secret_assignment",
            r#"(?i)\b(api[_-]?key|token|password|secret)\s*[:=]\s*['"]?[^'"\s]{6,}"#,
            "$1=[REDACTED_SECRET]",
        ),
        (
            "provider_token",
            r"\b(sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{12,}|xox[baprs]-[A-Za-z0-9-]{12,}|hf_[A-Za-z0-9]{12,}|AIza[A-Za-z0-9_-]{20,}|AKIA[A-Z0-9]{16})\b",
            "[REDACTED_TOKEN]",
        ),
        (
            "email",
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
            "[REDACTED_EMAIL]",
        ),
    ]
    .into_iter()
    .map(|(kind, pattern, replacement)| {
        (
            kind,
            Regex::new(pattern).expect("redaction regex compiles"),
            replacement,
        )
    })
    .collect()
});

pub fn contains_sensitive_data(input: &str) -> bool {
    REDACTION_RULES
        .iter()
        .any(|(_, regex, _)| regex.is_match(input))
}

pub fn redact_dream_text(input: &str) -> RedactionReport {
    let mut text = input.to_string();
    let mut redactions = Vec::new();
    for (kind, regex, replacement) in REDACTION_RULES.iter() {
        let count = regex.find_iter(&text).count();
        if count > 0 {
            text = regex.replace_all(&text, *replacement).to_string();
            redactions.push(Redaction {
                kind: (*kind).to_string(),
                count,
            });
        }
    }
    RedactionReport { text, redactions }
}

/// Redact secrets recursively before a structured payload crosses a persistence boundary.
///
/// String values are scanned with the shared detector. Values under explicit credential keys are
/// replaced even when the credential is too short or unusual to match a token-shaped regex.
pub fn redact_json_value(value: &mut Value) -> usize {
    redact_json_value_at_key(value, None)
}

fn redact_json_value_at_key(value: &mut Value, key: Option<&str>) -> usize {
    if key.is_some_and(is_secret_key) && !value.is_null() {
        *value = Value::String("[REDACTED_SECRET]".to_string());
        return 1;
    }
    match value {
        Value::String(text) => {
            let report = redact_dream_text(text);
            let count = report.redactions.iter().map(|item| item.count).sum();
            *text = report.text;
            count
        }
        Value::Array(items) => items
            .iter_mut()
            .map(|item| redact_json_value_at_key(item, None))
            .sum(),
        Value::Object(object) => object
            .iter_mut()
            .map(|(key, item)| redact_json_value_at_key(item, Some(key)))
            .sum(),
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

fn is_secret_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "apikey"
            | "accesstoken"
            | "authtoken"
            | "authorization"
            | "bearertoken"
            | "bootstrapsecret"
            | "clientsecret"
            | "cookie"
            | "credential"
            | "credentials"
            | "devicetoken"
            | "pairingcode"
            | "password"
            | "passphrase"
            | "providercredential"
            | "refreshtoken"
            | "secret"
            | "setcookie"
            | "token"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_obvious_secret_and_pii_patterns() {
        let input = "Authorization: Bearer sk-testsecret123456\nemail me at brian@example.com\n-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----";

        let report = redact_dream_text(input);

        assert!(!report.text.contains("sk-testsecret123456"));
        assert!(!report.text.contains("brian@example.com"));
        assert!(!report.text.contains("BEGIN PRIVATE KEY"));
        assert!(report.text.contains("Authorization: [REDACTED_SECRET]"));
        assert_eq!(
            report
                .redactions
                .iter()
                .map(|redaction| redaction.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["private_key", "authorization_header", "email"]
        );
    }

    #[test]
    fn detects_provider_tokens_without_redacting_a_copy() {
        assert!(contains_sensitive_data(
            "rotate sk-testsecret123456 tomorrow"
        ));
        assert!(!contains_sensitive_data("ordinary project reminder"));
    }

    #[test]
    fn redacts_device_pairing_and_bootstrap_credentials() {
        let device = format!("tmk_dev_{}", "a".repeat(64));
        let pairing_code = "b".repeat(64);
        let input = format!(
            "x-tempestmiku-bootstrap: bootstrap-secret\nAuthorization: Bearer opaque-token-123456\n{device}\ntempestmiku://pair?v=1&server=https%3A%2F%2Fmiku.example&code={pairing_code}\npairingCode={pairing_code}\npostgres://miku:database-password@db.internal/tempestmiku"
        );

        let report = redact_dream_text(&input);

        assert!(!report.text.contains("bootstrap-secret"));
        assert!(!report.text.contains("opaque-token-123456"));
        assert!(!report.text.contains(&device));
        assert!(!report.text.contains(&pairing_code));
        assert!(!report.text.contains("database-password"));
        assert!(contains_sensitive_data(&input));
    }

    #[test]
    fn recursively_redacts_structured_payloads_and_explicit_secret_keys() {
        let mut payload = serde_json::json!({
            "nested": [{
                "deviceToken": "short-but-secret",
                "accessToken": "opaque",
                "refreshToken": {"value": "nested-short-secret"},
                "message": "Authorization: Bearer opaque-token-123456",
                "tokenCount": 42
            }],
            "ordinary": "safe"
        });

        assert_eq!(redact_json_value(&mut payload), 4);
        assert_eq!(payload["nested"][0]["deviceToken"], "[REDACTED_SECRET]");
        assert_eq!(payload["nested"][0]["accessToken"], "[REDACTED_SECRET]");
        assert_eq!(payload["nested"][0]["refreshToken"], "[REDACTED_SECRET]");
        assert_eq!(
            payload["nested"][0]["message"],
            "Authorization: [REDACTED_SECRET]"
        );
        assert_eq!(payload["nested"][0]["tokenCount"], 42);
        assert_eq!(payload["ordinary"], "safe");
    }
}

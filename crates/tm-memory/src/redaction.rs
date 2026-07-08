use regex::Regex;
use serde::{Deserialize, Serialize};

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

pub fn redact_dream_text(input: &str) -> RedactionReport {
    let mut text = input.to_string();
    let mut redactions = Vec::new();
    for (kind, pattern, replacement) in [
        (
            "private_key",
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            "[REDACTED_PRIVATE_KEY]",
        ),
        (
            "authorization_header",
            r"(?im)^(authorization|x-api-key|api-key)\s*:\s*.+$",
            "$1: [REDACTED_SECRET]",
        ),
        (
            "api_secret_assignment",
            r#"(?i)\b(api[_-]?key|token|password|secret)\s*[:=]\s*['"]?[^'"\s]{6,}"#,
            "$1=[REDACTED_SECRET]",
        ),
        (
            "provider_token",
            r"\b(sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{12,}|xox[baprs]-[A-Za-z0-9-]{12,})\b",
            "[REDACTED_TOKEN]",
        ),
        (
            "email",
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
            "[REDACTED_EMAIL]",
        ),
    ] {
        let regex = Regex::new(pattern).expect("redaction regex compiles");
        let count = regex.find_iter(&text).count();
        if count > 0 {
            text = regex.replace_all(&text, replacement).to_string();
            redactions.push(Redaction {
                kind: kind.to_string(),
                count,
            });
        }
    }
    RedactionReport { text, redactions }
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
}

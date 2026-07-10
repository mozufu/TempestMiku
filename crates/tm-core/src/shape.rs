use serde_json::Value;

use crate::EvalOutput;

/// Default per-result byte cap before head+tail elision (design §5.4).
pub const DEFAULT_CAP: usize = 8 * 1024;

/// Turn an [`EvalOutput`] into the compact tool message the model sees.
pub fn shape_result(out: &EvalOutput) -> String {
    shape_result_capped(out, DEFAULT_CAP)
}

/// As [`shape_result`], with an explicit per-section byte cap.
pub fn shape_result_capped(out: &EvalOutput, cap: usize) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(err) = &out.error {
        parts.push(format!("error: {err}"));
    }
    if !out.stdout.is_empty() {
        parts.push(format!("stdout:\n{}", out.stdout));
    }
    if let Some(result) = &out.result {
        let rendered = match result {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
        };
        parts.push(format!("result:\n{rendered}"));
    }

    if parts.is_empty() {
        "(no output)".to_string()
    } else {
        cap_text(&parts.join("\n\n"), cap)
    }
}

/// Keep head + tail, eliding the middle, when `s` exceeds `cap` bytes. Cut points are snapped
/// to UTF-8 boundaries so the result is always valid.
fn cap_text(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let artifact_uris = artifact_uris(s);
    if artifact_uris.is_empty() {
        return format!(
            "error: ResultLimitError: backend returned {} bytes for a {cap}-byte result budget without a readable artifact reference",
            s.len()
        );
    }
    let head = floor_boundary(s, cap * 2 / 3);
    let tail = ceil_boundary(s, s.len().saturating_sub(cap - (cap * 2 / 3)));
    let omitted = tail.saturating_sub(head);
    let mut shaped = format!(
        "{}\n…[{} bytes elided]…\n{}",
        &s[..head],
        omitted,
        &s[tail..]
    );
    // Sandbox backends spill oversized material before it reaches this final aggregate cap.
    // Preserve those readable references even when the head/tail cut would otherwise remove
    // them; a few bytes of control metadata are preferable to an unusable truncation marker.
    for uri in artifact_uris.into_iter().take(8) {
        if !shaped.contains(uri) {
            shaped.push_str("\nfull output: ");
            shaped.push_str(uri);
        }
    }
    shaped
}

fn artifact_uris(input: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut offset = 0;
    while let Some(relative) = input[offset..].find("artifact://") {
        let start = offset + relative;
        let digits_start = start + "artifact://".len();
        let digits = input[digits_start..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if digits > 0 {
            let uri = &input[start..digits_start + digits];
            if !found.contains(&uri) {
                found.push(uri);
            }
        }
        offset = digits_start + digits.max(1);
        if offset >= input.len() {
            break;
        }
    }
    found
}

fn floor_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_result_and_stdout() {
        let out = EvalOutput {
            stdout: "hello".into(),
            result: Some(Value::String("world".into())),
            error: None,
        };
        let shaped = shape_result(&out);
        assert!(shaped.contains("stdout:\nhello"));
        assert!(shaped.contains("result:\nworld"));
    }

    #[test]
    fn empty_output_is_marked() {
        assert_eq!(shape_result(&EvalOutput::default()), "(no output)");
    }

    #[test]
    fn rejects_oversized_backend_output_without_an_artifact_reference() {
        let big = "é".repeat(10_000); // multi-byte chars stress the boundary snapping
        let out = EvalOutput {
            stdout: big,
            ..Default::default()
        };
        let shaped = shape_result_capped(&out, 1024);
        assert!(shaped.contains("ResultLimitError"));
        assert!(shaped.contains("without a readable artifact reference"));
        assert!(!shaped.contains('é'));
    }

    #[test]
    fn cap_applies_to_the_whole_shaped_result() {
        let out = EvalOutput {
            stdout: format!("{} artifact://7", "s".repeat(900)),
            result: Some(Value::String("r".repeat(900))),
            error: Some("e".repeat(900)),
        };
        let shaped = shape_result_capped(&out, 1024);
        assert!(shaped.contains("bytes elided"));
        assert!(shaped.contains("artifact://7"));
        assert!(shaped.len() < 1250, "shaped length: {}", shaped.len());
    }

    #[test]
    fn aggregate_elision_preserves_artifact_reference() {
        let out = EvalOutput {
            stdout: format!("{} artifact://42 {}", "a".repeat(900), "b".repeat(900)),
            result: None,
            error: None,
        };
        let shaped = shape_result_capped(&out, 128);
        assert!(shaped.contains("artifact://42"));
        assert!(shaped.contains("bytes elided"));
    }
}

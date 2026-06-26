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
        parts.push(format!("error: {}", cap_text(err, cap)));
    }
    if !out.stdout.is_empty() {
        parts.push(format!("stdout:\n{}", cap_text(&out.stdout, cap)));
    }
    if let Some(result) = &out.result {
        let rendered = match result {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
        };
        parts.push(format!("result:\n{}", cap_text(&rendered, cap)));
    }

    if parts.is_empty() {
        "(no output)".to_string()
    } else {
        parts.join("\n\n")
    }
}

/// Keep head + tail, eliding the middle, when `s` exceeds `cap` bytes. Cut points are snapped
/// to UTF-8 boundaries so the result is always valid.
fn cap_text(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let head = floor_boundary(s, cap * 2 / 3);
    let tail = ceil_boundary(s, s.len().saturating_sub(cap - (cap * 2 / 3)));
    let omitted = tail.saturating_sub(head);
    format!(
        "{}\n…[{} bytes elided]…\n{}",
        &s[..head],
        omitted,
        &s[tail..]
    )
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
    fn elides_oversized_and_stays_valid_utf8() {
        let big = "é".repeat(10_000); // multi-byte chars stress the boundary snapping
        let out = EvalOutput {
            stdout: big,
            ..Default::default()
        };
        let shaped = shape_result_capped(&out, 1024);
        assert!(shaped.contains("bytes elided"));
        assert!(shaped.len() < 2048);
    }
}

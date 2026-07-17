use std::{
    fs::File,
    io::{BufRead, BufReader},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{HostError, Result};

use super::MAX_APPROVAL_ACTION_BYTES;

const DEFAULT_READ_LINES: usize = 200;
const DEFAULT_READ_BYTES: usize = 64 * 1024;
const MAX_READ_LINES: usize = 1_000;
const MAX_READ_BYTES: usize = 256 * 1024;

pub(in crate::linked) fn read_text_page_from_file(
    file: File,
    selector: Option<&str>,
) -> Result<(String, bool)> {
    let (start, end, byte_limit) = parse_text_selector(selector)?;
    let normalize_selected_lines = selector.is_some();
    let mut reader = BufReader::new(file);
    for _ in 1..start {
        if reader
            .skip_until(b'\n')
            .map_err(|err| HostError::HostCall(err.to_string()))?
            == 0
        {
            return Ok((String::new(), false));
        }
    }

    let mut selected = Vec::new();
    let mut truncated = false;
    for (selected_lines, _) in (start..=end).enumerate() {
        let separator = usize::from(normalize_selected_lines && selected_lines > 0);
        let remaining = byte_limit.saturating_sub(selected.len().saturating_add(separator));
        let Some((mut line, line_truncated)) = read_bounded_line(&mut reader, remaining)? else {
            break;
        };
        if normalize_selected_lines {
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if separator == 1 {
                selected.push(b'\n');
            }
        }
        selected.extend_from_slice(&line);
        if line_truncated {
            truncated = true;
            break;
        }
    }
    let has_more = truncated
        || !reader
            .fill_buf()
            .map_err(|err| HostError::HostCall(err.to_string()))?
            .is_empty();
    if let Err(error) = std::str::from_utf8(&selected) {
        if error.error_len().is_some() {
            return Err(HostError::InvalidArgs(
                "fs.read supports UTF-8 text only".to_string(),
            ));
        }
        selected.truncate(error.valid_up_to());
    }
    let selected = String::from_utf8(selected)
        .map_err(|_| HostError::InvalidArgs("fs.read supports UTF-8 text only".to_string()))?;
    Ok((selected, has_more))
}

pub(in crate::linked) fn approval_action(operation: &str, details: Value) -> String {
    const MAX_FIELD_BYTES: usize = 512;
    const MAX_COLLECTION_ENTRIES: usize = 64;

    fn utf8_prefix(value: &str, limit: usize) -> &str {
        let mut end = value.len().min(limit);
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        &value[..end]
    }

    fn sanitize(value: Value) -> Value {
        match value {
            Value::String(value) => {
                let redacted = tm_memory::redact_dream_text(&value).text;
                if redacted.len() > MAX_FIELD_BYTES {
                    Value::String(format!(
                        "{}...[bounded:{} bytes]",
                        utf8_prefix(&redacted, MAX_FIELD_BYTES),
                        redacted.len()
                    ))
                } else {
                    Value::String(redacted)
                }
            }
            Value::Array(values) => {
                let original_len = values.len();
                let mut values = values
                    .into_iter()
                    .take(MAX_COLLECTION_ENTRIES)
                    .map(sanitize)
                    .collect::<Vec<_>>();
                if original_len > MAX_COLLECTION_ENTRIES {
                    values.push(serde_json::json!({
                        "boundedRemainingEntries": original_len - MAX_COLLECTION_ENTRIES
                    }));
                }
                Value::Array(values)
            }
            Value::Object(values) => Value::Object(
                values
                    .into_iter()
                    .take(MAX_COLLECTION_ENTRIES)
                    .map(|(key, value)| (key, sanitize(value)))
                    .collect(),
            ),
            value => value,
        }
    }

    let action = serde_json::json!({
        "operation": operation,
        "details": sanitize(details),
    });
    let encoded = serde_json::to_string(&action)
        .unwrap_or_else(|_| format!(r#"{{"operation":"{operation}","details":"unavailable"}}"#));
    if encoded.len() <= MAX_APPROVAL_ACTION_BYTES {
        return encoded;
    }
    let digest = Sha256::digest(encoded.as_bytes());
    serde_json::json!({
        "operation": operation,
        "details": {
            "bounded": true,
            "redactedSha256": hex::encode(digest),
            "encodedBytes": encoded.len(),
        }
    })
    .to_string()
}

fn parse_text_selector(selector: Option<&str>) -> Result<(usize, usize, usize)> {
    let Some(selector) = selector else {
        return Ok((1, DEFAULT_READ_LINES, DEFAULT_READ_BYTES));
    };
    let (start, end) = selector
        .split_once('-')
        .ok_or_else(|| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let start: usize = start
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let end: usize = end
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let line_count = end
        .checked_sub(start)
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    if start == 0 || end < start || line_count > MAX_READ_LINES {
        return Err(HostError::InvalidArgs(format!(
            "invalid selector {selector}"
        )));
    }
    Ok((start, end, MAX_READ_BYTES))
}

fn read_bounded_line<R: BufRead>(reader: &mut R, limit: usize) -> Result<Option<(Vec<u8>, bool)>> {
    let mut line = Vec::new();
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|err| HostError::HostCall(err.to_string()))?;
        if buffer.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some((line, false)))
            };
        }
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let content = &buffer[..=newline];
            let available = limit.saturating_sub(line.len());
            let copied = content.len().min(available);
            line.extend_from_slice(&content[..copied]);
            if copied < content.len() {
                reader.consume(copied);
                return Ok(Some((line, true)));
            }
            reader.consume(newline + 1);
            return Ok(Some((line, false)));
        }

        let available = limit.saturating_sub(line.len());
        let buffer_len = buffer.len();
        let copied = buffer_len.min(available);
        line.extend_from_slice(&buffer[..copied]);
        reader.consume(copied);
        if copied < buffer_len || available == 0 {
            return Ok(Some((line, true)));
        }
    }
}

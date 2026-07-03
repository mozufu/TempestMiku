use sha2::{Digest, Sha256};
use tm_host::{HostError, Result as HostResult};
use uuid::Uuid;

use crate::{Result, ServerError};

pub(crate) fn encode_memory_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '~') {
            encoded.push(ch);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub(crate) fn decode_memory_segment(value: &str) -> HostResult<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(HostError::InvalidPath(format!(
                    "invalid memory uri segment {value}"
                )));
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|_| {
                HostError::InvalidPath(format!("invalid memory uri segment {value}"))
            })?;
            let byte = u8::from_str_radix(hex, 16).map_err(|_| {
                HostError::InvalidPath(format!("invalid memory uri segment {value}"))
            })?;
            decoded.push(byte);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| HostError::InvalidPath(format!("invalid memory uri segment {value}")))
}

pub(crate) fn short_id(id: Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

pub(crate) fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    chars.div_ceil(4).max(1)
}

pub(crate) fn clean_required(field: &str, value: String) -> Result<String> {
    let cleaned = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        Err(ServerError::InvalidRequest(format!(
            "memory {field} cannot be empty"
        )))
    } else {
        Ok(cleaned)
    }
}

pub(crate) fn memory_dedupe_key(parts: &[&str]) -> String {
    let normalized = parts
        .iter()
        .map(|part| normalize_for_dedupe(part))
        .collect::<Vec<_>>()
        .join("\x1f");
    let digest = Sha256::digest(normalized.as_bytes());
    format!("sha256:{}", hex::encode(&digest[..16]))
}

pub(crate) fn memory_record_id(namespace: &str, dedupe_key: &str) -> Uuid {
    let digest = Sha256::digest(format!("{namespace}:{dedupe_key}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

pub(crate) fn normalize_for_dedupe(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

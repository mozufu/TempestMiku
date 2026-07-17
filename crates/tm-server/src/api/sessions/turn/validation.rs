use super::super::*;

pub(super) fn validate_client_message_id(value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));
    if valid {
        Ok(())
    } else {
        Err(ServerError::InvalidRequest(
            "clientMessageId must be 1-128 safe ASCII characters".to_string(),
        ))
    }
}

pub(super) fn validate_message_content(value: &str) -> Result<()> {
    const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
    if value.trim().is_empty() {
        return Err(ServerError::InvalidRequest(
            "message content must not be empty".to_string(),
        ));
    }
    if value.len() > MAX_MESSAGE_BYTES {
        return Err(ServerError::InvalidRequest(format!(
            "message content exceeds {MAX_MESSAGE_BYTES} bytes"
        )));
    }
    Ok(())
}

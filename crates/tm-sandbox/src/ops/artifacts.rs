use super::*;

#[op2]
#[serde]
pub(super) fn op_tm_artifact_put(
    state: &mut OpState,
    #[serde] data: serde_json::Value,
    #[serde] opts: serde_json::Value,
) -> std::result::Result<ArtifactRef, JsErrorBox> {
    let host_state = state.borrow::<RuntimeHostState>().clone();
    let title = opts
        .get("title")
        .and_then(Value::as_str)
        .map(|title| tm_memory::redact_dream_text(title).text);
    let mime = opts
        .get("mime")
        .and_then(Value::as_str)
        .unwrap_or("text/plain");
    validate_mime(mime)?;
    let content = match data {
        Value::String(s) => s,
        other => serde_json::to_string_pretty(&other).map_err(js_error)?,
    };
    let content = tm_memory::redact_dream_text(&content).text;
    host_state
        .artifact_store
        .put_text(content, title, mime)
        .map_err(js_error)
}

fn validate_mime(mime: &str) -> std::result::Result<(), JsErrorBox> {
    const MAX_MIME_BYTES: usize = 127;
    let Some((kind, subtype)) = mime.split_once('/') else {
        return Err(js_error("artifact MIME must be a type/subtype token"));
    };
    let valid_token = |token: &str| {
        !token.is_empty()
            && token.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                    )
            })
    };
    if mime.len() > MAX_MIME_BYTES
        || subtype.contains('/')
        || !valid_token(kind)
        || !valid_token(subtype)
    {
        return Err(js_error(
            "artifact MIME must be a bounded ASCII type/subtype token",
        ));
    }
    Ok(())
}

#[op2]
#[serde]
pub(super) fn op_tm_artifact_list(state: &mut OpState) -> Vec<ArtifactRef> {
    state.borrow::<RuntimeHostState>().artifact_store.list()
}

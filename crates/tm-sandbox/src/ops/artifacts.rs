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
        .map(str::to_string);
    let mime = opts
        .get("mime")
        .and_then(Value::as_str)
        .unwrap_or("text/plain");
    let content = match data {
        Value::String(s) => s,
        other => serde_json::to_string_pretty(&other).map_err(js_error)?,
    };
    host_state
        .artifact_store
        .put_text(content, title, mime)
        .map_err(js_error)
}

#[op2]
#[serde]
pub(super) fn op_tm_artifact_list(state: &mut OpState) -> Vec<ArtifactRef> {
    state.borrow::<RuntimeHostState>().artifact_store.list()
}

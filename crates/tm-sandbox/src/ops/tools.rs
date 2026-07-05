use super::*;

#[op2]
#[serde]
pub(super) fn op_tm_tools_search(
    state: &mut OpState,
    #[string] query: String,
    #[serde] opts: serde_json::Value,
) -> Vec<ToolSummary> {
    let host_state = state.borrow::<RuntimeHostState>().clone();
    let namespace = opts
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::to_string);
    let limit = (opts.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize).max(1);
    let mut summaries = host_state.host_registry.search(
        &query,
        namespace.as_deref(),
        limit,
        &host_state.invocation_ctx,
    );
    let core_docs = core_tool_docs();
    for docs in core_docs.values() {
        if summaries.len() >= limit {
            break;
        }
        if summaries.iter().any(|summary| summary.name == docs.name) {
            continue;
        }
        if !core_doc_matches(docs, &query, namespace.as_deref()) {
            continue;
        }
        summaries.push(ToolSummary {
            name: docs.name.clone(),
            namespace: docs.namespace.clone(),
            summary: docs.summary.clone(),
            sensitive: docs.sensitive,
            granted: core_doc_granted(&docs.name, &host_state.invocation_ctx),
        });
    }
    summaries
}

#[op2]
#[serde]
pub(super) fn op_tm_tools_docs(state: &mut OpState, #[string] name: String) -> serde_json::Value {
    let host_state = state.borrow::<RuntimeHostState>().clone();
    let mut core_docs = core_tool_docs();
    let docs = host_state
        .host_registry
        .docs(&name, &host_state.invocation_ctx)
        .or_else(|err| match core_docs.remove(&name) {
            Some(docs) => Ok(docs),
            None => Err(err),
        });
    sdk_result(docs)
}

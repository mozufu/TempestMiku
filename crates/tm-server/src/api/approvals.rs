use super::*;

pub(super) async fn resolve_approval<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, approval_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<ResolveApprovalRequest>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    state.store.get_session(session_id).await?;
    state
        .approval_broker
        .resolve(session_id, approval_id, payload)?;
    Ok(Json(json!({ "status": "resolved" })))
}

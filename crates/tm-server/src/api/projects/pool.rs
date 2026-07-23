use super::*;

use super::assign::ProjectResponse;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JoinPoolRequest {
    pub pool_id: String,
}

/// Join a project to a memory pool (§30.7): symmetric recall fan-out with the pool's other active
/// members. Rejects if either entity is archived, or if the project already belongs to a pool —
/// leave it first, no silent moves between pools.
pub(crate) async fn join_pool<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(project_id): Path<String>,
    Json(payload): Json<JoinPoolRequest>,
) -> Result<Json<ProjectResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let project_id = tm_drive::slug(&project_id);
    let record = state
        .store
        .join_memory_pool(&project_id, &payload.pool_id)
        .await?;
    Ok(Json(ProjectResponse::from_record(record)))
}

/// Remove a project from its memory pool (§30.7). Idempotent: a project with no pool is returned
/// unchanged.
pub(crate) async fn leave_pool<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(project_id): Path<String>,
) -> Result<Json<ProjectResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let project_id = tm_drive::slug(&project_id);
    let record = state.store.leave_memory_pool(&project_id).await?;
    Ok(Json(ProjectResponse::from_record(record)))
}

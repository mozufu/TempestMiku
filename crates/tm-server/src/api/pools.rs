use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatePoolRequest {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryPoolResponse {
    pub id: String,
    pub title: String,
    pub status: String,
}

impl MemoryPoolResponse {
    fn from_record(record: MemoryPoolRecord) -> Self {
        Self {
            id: record.id,
            title: record.title,
            status: record.status.as_str().to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryPoolListResponse {
    pub pools: Vec<MemoryPoolResponse>,
}

/// Create (or return) a memory pool entity (§30.7). Owner-initiated; idempotent on id.
pub(crate) async fn create_pool<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Json(payload): Json<CreatePoolRequest>,
) -> Result<Json<MemoryPoolResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let id = tm_drive::slug(&payload.id);
    if id.is_empty() {
        return Err(ServerError::InvalidRequest(
            "memory pool id must be a non-empty slug".to_string(),
        ));
    }
    let title = payload.title.unwrap_or_else(|| payload.id.clone());
    let record = state.store.ensure_memory_pool(&id, &title).await?;
    Ok(Json(MemoryPoolResponse::from_record(record)))
}

/// List active memory pools.
pub(crate) async fn list_pools<S, M, C>(
    State(state): State<AppState<S, M, C>>,
) -> Result<Json<MemoryPoolListResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let records = state.store.memory_pools(false).await?;
    Ok(Json(MemoryPoolListResponse {
        pools: records
            .into_iter()
            .map(MemoryPoolResponse::from_record)
            .collect(),
    }))
}

/// Archive a memory pool (§30.7): a pure status flip. Member projects keep their `pool_id`
/// unchanged; recall fan-out stops seeing them because fan-out is gated on the pool's status at
/// read time, not on a membership list that needs cleaning up.
pub(crate) async fn archive_pool<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(pool_id): Path<String>,
) -> Result<Json<MemoryPoolResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let record = state.store.archive_memory_pool(&pool_id).await?;
    Ok(Json(MemoryPoolResponse::from_record(record)))
}

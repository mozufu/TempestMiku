use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateProjectRequest {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub default_memory_policy: Option<crate::MemoryPolicy>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectResponse {
    pub id: String,
    pub title: String,
    pub status: String,
    pub memory_scope: String,
    pub project_uri: String,
    pub default_memory_policy: crate::MemoryPolicy,
}

impl ProjectResponse {
    fn from_record(record: ProjectRecord) -> Self {
        let id = record.id;
        Self {
            title: record.title,
            status: record.status.as_str().to_string(),
            memory_scope: format!("project:{id}"),
            project_uri: format!("project://{id}"),
            default_memory_policy: record.default_memory_policy,
            id,
        }
    }
}

/// Create (or return) a project entity (§30.2). Owner-initiated; idempotent on id.
pub(crate) async fn create_project<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Json(payload): Json<CreateProjectRequest>,
) -> Result<Json<ProjectResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let id = tm_drive::slug(&payload.id);
    if id.is_empty() {
        return Err(ServerError::InvalidRequest(
            "project id must be a non-empty slug".to_string(),
        ));
    }
    let title = payload.title.unwrap_or_else(|| payload.id.clone());
    let record = state
        .store
        .ensure_project(
            &id,
            &title,
            payload
                .default_memory_policy
                .unwrap_or(crate::MemoryPolicy::Project),
        )
        .await?;
    Ok(Json(ProjectResponse::from_record(record)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveProjectRequest {
    #[serde(default = "default_archive_reason")]
    pub reason: String,
}

fn default_archive_reason() -> String {
    "project archived".to_string()
}

/// Archive a project entity, tombstoning its memory scope (§30.4). This is the sole scope-killing
/// transition; folder unlink never reaches here.
pub(crate) async fn archive_project<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(project_id): Path<String>,
    Json(payload): Json<ArchiveProjectRequest>,
) -> Result<Json<ProjectResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let owner_subject = state.store.owner_subject().await?;
    let record = state
        .store
        .archive_project(&owner_subject, &project_id, &payload.reason)
        .await?;
    Ok(Json(ProjectResponse::from_record(record)))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssignSessionResponse {
    pub project_id: String,
    pub project_uri: String,
    pub assigned: usize,
}

/// Assign a closed session to a project (§30.6): a metadata declaration, not a copy. The server
/// re-runs the per-turn observation extraction over the session's message history so project items
/// grow through the one pipeline regardless of when assignment happened.
pub(crate) async fn assign_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((project_id, session_id)): Path<(String, Uuid)>,
) -> Result<Json<AssignSessionResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let project_id = tm_drive::slug(&project_id);
    let project = state
        .store
        .project(&project_id)
        .await?
        .ok_or_else(|| ServerError::NotFound(format!("project {project_id}")))?;
    if project.status != ProjectStatus::Active {
        return Err(ServerError::Conflict(format!(
            "project {project_id} is archived"
        )));
    }
    // A session is assigned only after it is closed; an active session declares its project through
    // POST /sessions/:id/scope instead.
    let session = state.store.get_session(session_id).await?;
    if session.status != "ended" {
        return Err(ServerError::Conflict(format!(
            "session {session_id} is still active; use POST /sessions/{session_id}/scope"
        )));
    }
    let messages = state.store.session_messages(session_id).await?;
    let mut assigned = 0;
    // Fold user/assistant pairs into the same observation pipeline used live (§27).
    let mut pending_user: Option<String> = None;
    for message in messages {
        match message.role.as_str() {
            "user" => pending_user = Some(message.content),
            "assistant" => {
                let user_content = pending_user.take().unwrap_or_default();
                let before = state.store.project_items(&project_id, None).await?.len();
                record_project_observations(
                    &state,
                    session_id,
                    project_id.clone(),
                    &user_content,
                    &message.content,
                )
                .await?;
                let after = state.store.project_items(&project_id, None).await?.len();
                assigned += after.saturating_sub(before);
            }
            _ => {}
        }
    }
    Ok(Json(AssignSessionResponse {
        project_id: project_id.clone(),
        project_uri: format!("project://{project_id}"),
        assigned,
    }))
}

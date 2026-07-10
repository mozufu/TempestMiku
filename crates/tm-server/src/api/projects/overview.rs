use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectQuery {
    project_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectOverview {
    project_id: String,
    project_uri: String,
    session_id: Uuid,
    status: String,
    open_loops: Vec<ProjectItemRecord>,
    decisions: Vec<ProjectItemRecord>,
    next_actions: Vec<ProjectItemRecord>,
    pub(crate) resources: Vec<ProjectItemRecord>,
    provenance: Value,
}

pub(crate) async fn project_overview<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<ProjectOverview>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    resources::util::validate_authorized_memory_scope(
        &state.linked_folders,
        &session.memory_scope,
    )?;
    let project_id = resources::util::authorized_project_id(&session, query.project_id.as_deref())?;
    Ok(Json(
        build_project_overview(&state, session_id, project_id).await?,
    ))
}

pub(crate) async fn project_open_loops<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<ProjectItemRecord>>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    resources::util::validate_authorized_memory_scope(
        &state.linked_folders,
        &session.memory_scope,
    )?;
    let project_id = resources::util::authorized_project_id(&session, query.project_id.as_deref())?;
    Ok(Json(
        state
            .store
            .project_items(&project_id, Some(ProjectItemKind::OpenLoop))
            .await?,
    ))
}

pub(crate) async fn project_decisions<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<ProjectItemRecord>>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    resources::util::validate_authorized_memory_scope(
        &state.linked_folders,
        &session.memory_scope,
    )?;
    let project_id = resources::util::authorized_project_id(&session, query.project_id.as_deref())?;
    Ok(Json(
        state
            .store
            .project_items(&project_id, Some(ProjectItemKind::Decision))
            .await?,
    ))
}

pub(crate) async fn project_next_actions<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<ProjectItemRecord>>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    resources::util::validate_authorized_memory_scope(
        &state.linked_folders,
        &session.memory_scope,
    )?;
    let project_id = resources::util::authorized_project_id(&session, query.project_id.as_deref())?;
    Ok(Json(
        state
            .store
            .project_items(&project_id, Some(ProjectItemKind::NextAction))
            .await?,
    ))
}

pub(crate) async fn build_project_overview<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    project_id: String,
) -> Result<ProjectOverview>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let items = state.store.project_items(&project_id, None).await?;
    let mut by_kind: BTreeMap<ProjectItemKind, Vec<ProjectItemRecord>> = BTreeMap::new();
    for item in items {
        by_kind.entry(item.kind).or_default().push(item);
    }
    let summaries = by_kind
        .get(&ProjectItemKind::Summary)
        .cloned()
        .unwrap_or_default();
    let status = by_kind
        .get(&ProjectItemKind::Status)
        .and_then(|items| items.last())
        .map(|item| item.text.clone())
        .or_else(|| summaries.last().map(|item| item.text.clone()))
        .unwrap_or_else(|| format!("Session {session_id} has no project summary yet."));
    let mut resources = Vec::new();
    for kind in [
        ProjectItemKind::Resource,
        ProjectItemKind::Artifact,
        ProjectItemKind::Workspace,
        ProjectItemKind::Linked,
    ] {
        resources.extend(by_kind.get(&kind).cloned().unwrap_or_default());
    }
    Ok(ProjectOverview {
        project_uri: format!("project://{project_id}"),
        project_id,
        session_id,
        status,
        open_loops: by_kind
            .remove(&ProjectItemKind::OpenLoop)
            .unwrap_or_default(),
        decisions: by_kind
            .remove(&ProjectItemKind::Decision)
            .unwrap_or_default(),
        next_actions: by_kind
            .remove(&ProjectItemKind::NextAction)
            .unwrap_or_default(),
        resources,
        provenance: json!({
            "builtFrom": ["session_events", "project_items"],
            "sessionId": session_id,
        }),
    })
}

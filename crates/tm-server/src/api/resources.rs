use super::*;

#[derive(Debug, Default, Deserialize)]
pub(super) struct ArtifactReadQuery {
    selector: Option<String>,
}

pub(super) async fn list_artifacts<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    state.store.get_session(session_id).await?;
    let store = tm_artifacts::ArtifactStore::open(&state.artifact_root, session_id.to_string())
        .map_err(|err| ServerError::Store(err.to_string()))?;
    Ok(Json(serde_json::to_value(store.list())?))
}

pub(super) async fn read_artifact<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, artifact_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
    Query(query): Query<ArtifactReadQuery>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    state.store.get_session(session_id).await?;
    let store = tm_artifacts::ArtifactStore::open(&state.artifact_root, session_id.to_string())
        .map_err(|err| ServerError::Store(err.to_string()))?;
    let uri = format!("artifact://{artifact_id}");
    let content = store
        .read(&uri, query.selector.as_deref())
        .map_err(map_artifact_error)?;
    Ok(Json(serde_json::to_value(content)?))
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct ResourceQuery {
    uri: Option<String>,
    selector: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct DriveFeedQuery {
    limit: Option<usize>,
    project: Option<String>,
}

pub(super) async fn drive_feed<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<DriveFeedQuery>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    state.store.get_session(session_id).await?;
    let store = state
        .drive_store
        .as_ref()
        .ok_or_else(|| ServerError::Policy("drive store is not configured".to_string()))?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let recent = if query
        .project
        .as_ref()
        .is_some_and(|project| !project.trim().is_empty())
    {
        store
            .search(tm_drive::DriveSearchOptions {
                project: query.project.clone(),
                limit,
                return_snippets: true,
                ..tm_drive::DriveSearchOptions::default()
            })
            .map_err(|err| ServerError::Store(err.to_string()))?
            .into_iter()
            .map(|hit| {
                json!({
                    "uri": hit.uri,
                    "path": hit.path,
                    "title": hit.title,
                    "docKind": hit.doc_kind,
                    "project": hit.project,
                    "tags": hit.tags,
                    "contentHash": hit.content_hash,
                    "snippet": hit.snippet,
                    "selector": hit.selector,
                })
            })
            .collect::<Vec<_>>()
    } else {
        store
            .list(tm_drive::DriveListOptions {
                recursive: true,
                limit,
                include_archived: false,
                ..tm_drive::DriveListOptions::default()
            })
            .map_err(|err| ServerError::Store(err.to_string()))?
            .into_iter()
            .map(|entry| {
                json!({
                    "uri": entry.uri,
                    "path": entry.path,
                    "title": entry.title,
                    "docKind": entry.doc_kind,
                    "project": entry.project,
                    "tags": entry.tags,
                    "contentHash": entry.content_hash,
                    "sizeBytes": entry.size_bytes,
                    "updatedAt": entry.updated_at,
                    "summary": entry.summary,
                })
            })
            .collect::<Vec<_>>()
    };
    let proposals = store
        .proposals()
        .into_iter()
        .filter(|proposal| proposal.status == tm_drive::ProposalStatus::Pending)
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "recent": recent,
        "virtualDirs": [
            { "uri": "drive://recent", "name": "recent", "kind": "virtual_dir", "title": "Recent documents" },
            { "uri": "drive://by-project", "name": "by-project", "kind": "virtual_dir", "title": "Documents by project" },
            { "uri": "drive://by-type", "name": "by-type", "kind": "virtual_dir", "title": "Documents by type" },
            { "uri": "drive://by-tag", "name": "by-tag", "kind": "virtual_dir", "title": "Documents by tag" },
            { "uri": "drive://by-date", "name": "by-date", "kind": "virtual_dir", "title": "Documents by date" }
        ],
        "proposals": proposals,
        "pendingApprovals": []
    })))
}

pub(super) async fn resolve_resource<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<ResourceQuery>,
) -> Result<Json<ResourceContent>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    state.store.get_session(session_id).await?;
    let uri = query.uri.ok_or_else(|| {
        ServerError::InvalidRequest("uri query parameter is required".to_string())
    })?;
    read_resource_content(&state, session_id, &uri, query.selector.as_deref())
        .await
        .map(Json)
}

pub(super) async fn preview_resource<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<ResourceQuery>,
) -> Result<Json<ResourceContent>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    state.store.get_session(session_id).await?;
    let uri = query.uri.ok_or_else(|| {
        ServerError::InvalidRequest("uri query parameter is required".to_string())
    })?;
    preview_resource_content(&state, session_id, &uri)
        .await
        .map(Json)
}

pub(super) async fn list_resources<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<ResourceQuery>,
) -> Result<Json<Vec<ResourceEntry>>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    state.store.get_session(session_id).await?;
    list_resource_entries(&state, session_id, query.uri.as_deref())
        .await
        .map(Json)
}

async fn read_resource_content<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    uri: &str,
    selector: Option<&str>,
) -> Result<ResourceContent>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    match resource_scheme(uri).as_deref() {
        Some("artifact") => {
            let store =
                tm_artifacts::ArtifactStore::open(&state.artifact_root, session_id.to_string())
                    .map_err(|err| ServerError::Store(err.to_string()))?;
            store.read(uri, selector).map_err(map_artifact_error)
        }
        Some("workspace") => {
            read_workspace_resource(&state.artifact_root, session_id, uri, selector)
        }
        Some("linked") => read_linked_resource(&state.linked_folders, uri, selector).await,
        Some("project") => read_project_resource(state, session_id, uri, selector).await,
        Some("memory") => read_memory_resource(state, session_id, uri, selector).await,
        Some("agent") => read_agent_resource(state, uri, selector).await,
        Some("history") => read_history_resource(state, uri, selector).await,
        Some("cron") => read_cron_resource(state, uri).await,
        Some("drive") => read_drive_resource(state, uri, selector).await,
        Some(scheme) => Err(ServerError::Policy(format!(
            "unknown resource scheme {scheme}; registered: {}",
            registered_resource_schemes(state).join(", ")
        ))),
        None => Err(ServerError::InvalidRequest(format!(
            "invalid resource uri {uri}"
        ))),
    }
}

pub(super) async fn preview_resource_content<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    uri: &str,
) -> Result<ResourceContent>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let content = match resource_scheme(uri).as_deref() {
        Some("memory") => preview_memory_resource(state, session_id, uri).await?,
        _ => read_resource_content(state, session_id, uri, None).await?,
    };
    Ok(compact_resource_preview(content))
}

async fn list_resource_entries<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    uri: Option<&str>,
) -> Result<Vec<ResourceEntry>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let Some(uri) = uri.filter(|uri| !uri.is_empty()) else {
        return Ok(registered_resource_schemes(state)
            .into_iter()
            .map(|scheme| ResourceEntry {
                uri: format!("{scheme}://"),
                name: scheme.to_string(),
                kind: "scheme".to_string(),
                title: None,
                size_bytes: None,
                modified_at: None,
            })
            .collect());
    };
    match resource_scheme(uri).as_deref() {
        Some("artifact") => {
            let store =
                tm_artifacts::ArtifactStore::open(&state.artifact_root, session_id.to_string())
                    .map_err(|err| ServerError::Store(err.to_string()))?;
            Ok(store
                .list()
                .into_iter()
                .map(|artifact| ResourceEntry {
                    uri: artifact.uri,
                    name: artifact.id,
                    kind: artifact.kind,
                    title: artifact.title,
                    size_bytes: Some(artifact.size_bytes),
                    modified_at: None,
                })
                .collect())
        }
        Some("workspace") => list_workspace_resources(&state.artifact_root, session_id, uri),
        Some("linked") => list_linked_resources(&state.linked_folders, Some(uri)).await,
        Some("project") => list_project_resources(state, session_id, uri).await,
        Some("memory") => list_memory_resources(state, session_id, uri).await,
        Some("agent") => list_agent_resources(state, uri).await,
        Some("history") => Err(ServerError::Policy(
            "history:// listing not supported — read a specific history://<id>".to_string(),
        )),
        Some("cron") => list_cron_resources(state, uri).await,
        Some("drive") => list_drive_resources(state, uri).await,
        Some(scheme) => Err(ServerError::Policy(format!(
            "unknown resource scheme {scheme}; registered: {}",
            registered_resource_schemes(state).join(", ")
        ))),
        None => Err(ServerError::InvalidRequest(format!(
            "invalid resource uri {uri}"
        ))),
    }
}

fn registered_resource_schemes<S, M, C>(state: &AppState<S, M, C>) -> Vec<&'static str> {
    let mut schemes = vec![
        "artifact",
        "linked",
        "workspace",
        "project",
        "memory",
        "agent",
        "history",
        "cron",
    ];
    if state.drive_store.is_some() {
        schemes.push("drive");
    }
    schemes
}

async fn read_cron_resource<S, M, C>(
    state: &AppState<S, M, C>,
    uri: &str,
) -> Result<ResourceContent>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let payload = match parse_cron_uri(uri)? {
        CronUri::Root => serde_json::to_value(state.store.cron_jobs().await?)?,
        CronUri::Job { job_id } => {
            let job = state.store.cron_job(&job_id).await?;
            let runs = state.store.cron_runs(&job_id, 20).await?;
            json!({ "job": job, "runs": runs })
        }
        CronUri::Runs { job_id } => {
            serde_json::to_value(state.store.cron_runs(&job_id, 50).await?)?
        }
        CronUri::Run { job_id, run_id } => {
            let run = state
                .store
                .cron_runs(&job_id, 100)
                .await?
                .into_iter()
                .find(|run| run.id == run_id)
                .ok_or_else(|| ServerError::NotFound(format!("cron run {run_id}")))?;
            serde_json::to_value(run)?
        }
    };
    let content = serde_json::to_string_pretty(&payload)?;
    Ok(ResourceContent {
        uri: uri.to_string(),
        kind: "cron_view".to_string(),
        mime: "application/json".to_string(),
        title: Some(uri.to_string()),
        size_bytes: content.len(),
        selector: None,
        has_more: false,
        preview: preview(&content, 1024),
        content,
    })
}

async fn list_cron_resources<S, M, C>(
    state: &AppState<S, M, C>,
    uri: &str,
) -> Result<Vec<ResourceEntry>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    match parse_cron_uri(uri)? {
        CronUri::Root => Ok(state
            .store
            .cron_jobs()
            .await?
            .into_iter()
            .map(|job| ResourceEntry {
                uri: format!("cron://{}", job.id),
                name: job.id,
                kind: "cron_job".to_string(),
                title: Some(job.name),
                size_bytes: None,
                modified_at: Some(job.updated_at.to_rfc3339()),
            })
            .collect()),
        CronUri::Job { job_id } | CronUri::Runs { job_id } => Ok(state
            .store
            .cron_runs(&job_id, 50)
            .await?
            .into_iter()
            .map(|run| ResourceEntry {
                uri: format!("cron://{}/runs/{}", run.job_id, run.id),
                name: run.id.to_string(),
                kind: "cron_run".to_string(),
                title: Some(run.status),
                size_bytes: None,
                modified_at: run
                    .completed_at
                    .or(Some(run.started_at))
                    .map(|time| time.to_rfc3339()),
            })
            .collect()),
        CronUri::Run { .. } => Err(ServerError::Policy(
            "cron run listing not supported — list cron://<job>/runs".to_string(),
        )),
    }
}

enum CronUri {
    Root,
    Job { job_id: String },
    Runs { job_id: String },
    Run { job_id: String, run_id: Uuid },
}

fn parse_cron_uri(uri: &str) -> Result<CronUri> {
    let path = uri
        .strip_prefix("cron://")
        .ok_or_else(|| ServerError::Policy(format!("unsupported cron uri {uri}")))?;
    if path.is_empty() || path == "root" {
        return Ok(CronUri::Root);
    }
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        [job_id] if !job_id.is_empty() => Ok(CronUri::Job {
            job_id: (*job_id).to_string(),
        }),
        [job_id, "runs"] if !job_id.is_empty() => Ok(CronUri::Runs {
            job_id: (*job_id).to_string(),
        }),
        [job_id, "runs", run_id] if !job_id.is_empty() => Ok(CronUri::Run {
            job_id: (*job_id).to_string(),
            run_id: Uuid::parse_str(run_id)
                .map_err(|_| ServerError::Policy(format!("invalid cron uri {uri}")))?,
        }),
        _ => Err(ServerError::Policy(format!("unsupported cron uri {uri}"))),
    }
}

fn resource_scheme(uri: &str) -> Option<String> {
    uri.split_once("://").map(|(scheme, _)| scheme.to_string())
}

async fn read_linked_resource(
    linked_folders: &LinkedFolders,
    uri: &str,
    selector: Option<&str>,
) -> Result<ResourceContent> {
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(LinkedResourceHandler::new(linked_folders.clone())));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:linked"));
    registry
        .read(uri, selector, &ctx)
        .await
        .map_err(map_host_error)
}

async fn list_linked_resources(
    linked_folders: &LinkedFolders,
    uri: Option<&str>,
) -> Result<Vec<ResourceEntry>> {
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(LinkedResourceHandler::new(linked_folders.clone())));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:linked"));
    registry.list(uri, &ctx).await.map_err(map_host_error)
}

async fn read_drive_resource<S, M, C>(
    state: &AppState<S, M, C>,
    uri: &str,
    selector: Option<&str>,
) -> Result<ResourceContent>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let drive_store = state.drive_store.clone().ok_or_else(|| {
        ServerError::Policy(format!(
            "unknown resource scheme drive; registered: {}",
            registered_resource_schemes(state).join(", ")
        ))
    })?;
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(tm_drive::DriveResourceHandler::new(drive_store)));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:drive"));
    registry
        .read(uri, selector, &ctx)
        .await
        .map_err(map_host_error)
}

async fn list_drive_resources<S, M, C>(
    state: &AppState<S, M, C>,
    uri: &str,
) -> Result<Vec<ResourceEntry>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let drive_store = state.drive_store.clone().ok_or_else(|| {
        ServerError::Policy(format!(
            "unknown resource scheme drive; registered: {}",
            registered_resource_schemes(state).join(", ")
        ))
    })?;
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(tm_drive::DriveResourceHandler::new(drive_store)));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:drive"));
    registry.list(Some(uri), &ctx).await.map_err(map_host_error)
}

async fn read_memory_resource<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    uri: &str,
    selector: Option<&str>,
) -> Result<ResourceContent>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(crate::memory::MemoryResourceHandler::new(
        Arc::clone(&state.store),
        default_subject(),
        mode_profile(&state.persona, &session.mode_state.mode).default_scope,
    )));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:memory"));
    registry
        .read(uri, selector, &ctx)
        .await
        .map_err(map_host_error)
}

async fn preview_memory_resource<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    uri: &str,
) -> Result<ResourceContent>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(crate::memory::MemoryResourceHandler::new(
        Arc::clone(&state.store),
        default_subject(),
        mode_profile(&state.persona, &session.mode_state.mode).default_scope,
    )));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:memory"));
    registry.preview(uri, &ctx).await.map_err(map_host_error)
}

async fn list_memory_resources<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    uri: &str,
) -> Result<Vec<ResourceEntry>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(crate::memory::MemoryResourceHandler::new(
        Arc::clone(&state.store),
        default_subject(),
        mode_profile(&state.persona, &session.mode_state.mode).default_scope,
    )));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:memory"));
    registry.list(Some(uri), &ctx).await.map_err(map_host_error)
}

async fn read_agent_resource<S, M, C>(
    state: &AppState<S, M, C>,
    uri: &str,
    selector: Option<&str>,
) -> Result<ResourceContent>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(AgentResourceHandler::new(Arc::clone(
        &state.actor_roster,
    ))));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:agent"));
    registry
        .read(uri, selector, &ctx)
        .await
        .map_err(map_host_error)
}

async fn read_history_resource<S, M, C>(
    state: &AppState<S, M, C>,
    uri: &str,
    selector: Option<&str>,
) -> Result<ResourceContent>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(HistoryResourceHandler::new(Arc::clone(
        &state.actor_roster,
    ))));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:history"));
    registry
        .read(uri, selector, &ctx)
        .await
        .map_err(map_host_error)
}

async fn list_agent_resources<S, M, C>(
    state: &AppState<S, M, C>,
    uri: &str,
) -> Result<Vec<ResourceEntry>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(AgentResourceHandler::new(Arc::clone(
        &state.actor_roster,
    ))));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:agent"));
    registry.list(Some(uri), &ctx).await.map_err(map_host_error)
}

fn compact_resource_preview(mut content: ResourceContent) -> ResourceContent {
    let basis = if content.content.is_empty() {
        content.preview.clone()
    } else {
        content.content.clone()
    };
    content.preview = preview(&basis, SESSION_RESOURCE_PREVIEW_BYTES);
    content.has_more = content.has_more || basis.len() > SESSION_RESOURCE_PREVIEW_BYTES;
    content.content.clear();
    content
}

fn read_workspace_resource(
    artifact_root: &FsPath,
    session_id: Uuid,
    uri: &str,
    selector: Option<&str>,
) -> Result<ResourceContent> {
    let relative = workspace_relative(uri)?;
    let root = workspace_root(artifact_root, session_id);
    let path = root.join(&relative);
    let canonical_root = root
        .canonicalize()
        .map_err(|_| ServerError::NotFound(format!("workspace://session/")))?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| ServerError::NotFound(uri.to_string()))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(ServerError::Policy(format!(
            "workspace path escapes session root: {uri}"
        )));
    }
    let content =
        fs::read_to_string(&canonical_path).map_err(|err| ServerError::Store(err.to_string()))?;
    let (selected, has_more) = select_text(&content, selector)?;
    Ok(ResourceContent {
        uri: workspace_uri(&relative),
        kind: "text".to_string(),
        mime: "text/plain".to_string(),
        title: canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string),
        size_bytes: content.len(),
        selector: selector.map(str::to_string),
        has_more,
        preview: preview(&selected, 1024),
        content: selected,
    })
}

fn list_workspace_resources(
    artifact_root: &FsPath,
    session_id: Uuid,
    uri: &str,
) -> Result<Vec<ResourceEntry>> {
    let relative = workspace_relative(uri)?;
    let root = workspace_root(artifact_root, session_id);
    let dir = root.join(&relative);
    let canonical_root = root
        .canonicalize()
        .map_err(|_| ServerError::NotFound(format!("workspace://session/")))?;
    let canonical_dir = dir
        .canonicalize()
        .map_err(|_| ServerError::NotFound(uri.to_string()))?;
    if !canonical_dir.starts_with(&canonical_root) {
        return Err(ServerError::Policy(format!(
            "workspace path escapes session root: {uri}"
        )));
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&canonical_dir).map_err(|err| ServerError::Store(err.to_string()))? {
        let entry = entry.map_err(|err| ServerError::Store(err.to_string()))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let rel = path.strip_prefix(&canonical_root).unwrap_or(path.as_path());
        entries.push(ResourceEntry {
            uri: workspace_uri(rel),
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace")
                .to_string(),
            kind: if metadata.is_dir() { "dir" } else { "file" }.to_string(),
            title: None,
            size_bytes: metadata.is_file().then_some(metadata.len() as usize),
            modified_at: metadata
                .modified()
                .ok()
                .map(|time| chrono::DateTime::<Utc>::from(time).to_rfc3339()),
        });
    }
    entries.sort_by(|a, b| a.uri.cmp(&b.uri));
    Ok(entries)
}

fn workspace_root(artifact_root: &FsPath, session_id: Uuid) -> PathBuf {
    artifact_root
        .join("sessions")
        .join(session_id.to_string())
        .join("workspace")
}

fn workspace_relative(uri: &str) -> Result<PathBuf> {
    let path = uri
        .strip_prefix("workspace://session/")
        .ok_or_else(|| ServerError::Policy(format!("unsupported workspace uri {uri}")))?;
    validate_relative_path(path)
}

pub(super) fn validate_relative_path(path: &str) -> Result<PathBuf> {
    if path.contains('\0') {
        return Err(ServerError::Policy("path contains NUL byte".to_string()));
    }
    let relative = FsPath::new(path);
    if relative.is_absolute() {
        return Err(ServerError::Policy(format!(
            "absolute paths are not allowed: {path}"
        )));
    }
    let mut out = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ServerError::Policy(format!(
                    "path traversal is not allowed: {path}"
                )));
            }
        }
    }
    Ok(out)
}

fn workspace_uri(relative: &FsPath) -> String {
    let rel = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("workspace://session/{rel}")
}

async fn read_project_resource<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    uri: &str,
    selector: Option<&str>,
) -> Result<ResourceContent>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let (project_id, view) = parse_project_uri(uri)?;
    if let Some(view) = view.as_deref()
        && let Some(linked_uri) = project_linked_uri(view)
    {
        ensure_project_linked_alias_active(&project_id, view, &state.linked_folders)?;
        let mut content =
            read_linked_resource(&state.linked_folders, &linked_uri, selector).await?;
        content.uri = uri.to_string();
        return Ok(content);
    }
    let payload = match view.as_deref() {
        None | Some("") => serde_json::to_value(
            build_project_overview(state, session_id, project_id.clone()).await?,
        )?,
        Some("open-loops") => serde_json::to_value(
            state
                .store
                .project_items(&project_id, Some(ProjectItemKind::OpenLoop))
                .await?,
        )?,
        Some("decisions") => serde_json::to_value(
            state
                .store
                .project_items(&project_id, Some(ProjectItemKind::Decision))
                .await?,
        )?,
        Some("next-actions") => serde_json::to_value(
            state
                .store
                .project_items(&project_id, Some(ProjectItemKind::NextAction))
                .await?,
        )?,
        Some("resources") => {
            let overview = build_project_overview(state, session_id, project_id.clone()).await?;
            serde_json::to_value(overview.resources)?
        }
        Some("memory") => {
            let policy = active_project_link_policy(&project_id, &state.linked_folders)
                .ok_or_else(|| inactive_project_memory_scope_error(&project_id))?;
            project_memory_view(&project_id, &policy)
        }
        Some("linked-folders") => serde_json::to_value(project_linked_folder_entries(
            &project_id,
            &state.linked_folders,
        ))?,
        Some(other) => {
            return Err(ServerError::NotFound(format!(
                "project view project://{project_id}/{other}"
            )));
        }
    };
    let content = serde_json::to_string_pretty(&payload)?;
    Ok(ResourceContent {
        uri: uri.to_string(),
        kind: "project_view".to_string(),
        mime: "application/json".to_string(),
        title: Some(format!("project://{project_id}")),
        size_bytes: content.len(),
        selector: None,
        has_more: false,
        preview: preview(&content, 1024),
        content,
    })
}

async fn list_project_resources<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    uri: &str,
) -> Result<Vec<ResourceEntry>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let (project_id, view) = parse_project_uri(uri)?;
    if let Some(view) = view.as_deref() {
        if view == "memory" {
            let policy = active_project_link_policy(&project_id, &state.linked_folders)
                .ok_or_else(|| inactive_project_memory_scope_error(&project_id))?;
            return Ok(project_memory_entries(&project_id, &policy));
        }
        if view == "linked-folders" {
            return Ok(project_linked_folder_entries(
                &project_id,
                &state.linked_folders,
            ));
        }
        if let Some(linked_uri) = project_linked_uri(view) {
            ensure_project_linked_alias_active(&project_id, view, &state.linked_folders)?;
            let entries = list_linked_resources(&state.linked_folders, Some(&linked_uri)).await?;
            return Ok(entries
                .into_iter()
                .map(|entry| ResourceEntry {
                    uri: linked_to_project_uri(&project_id, &entry.uri),
                    ..entry
                })
                .collect());
        }
    }
    if view.is_some() {
        let overview = build_project_overview(state, session_id, project_id).await?;
        return Ok(overview
            .resources
            .into_iter()
            .map(|item| ResourceEntry {
                uri: item.target_uri,
                name: item.kind.as_str().to_string(),
                kind: item.kind.as_str().to_string(),
                title: Some(item.text),
                size_bytes: None,
                modified_at: Some(item.created_at.to_rfc3339()),
            })
            .collect());
    }
    let mut views = vec![
        "open-loops",
        "decisions",
        "next-actions",
        "resources",
        "linked-folders",
    ];
    if active_project_link_policy(&project_id, &state.linked_folders).is_some() {
        views.insert(4, "memory");
    }
    Ok(views
        .into_iter()
        .map(|view| ResourceEntry {
            uri: format!("project://{project_id}/{view}"),
            name: view.to_string(),
            kind: "project_view".to_string(),
            title: None,
            size_bytes: None,
            modified_at: None,
        })
        .collect())
}

fn project_linked_folder_entries(
    project_id: &str,
    linked_folders: &LinkedFolders,
) -> Vec<ResourceEntry> {
    linked_folders
        .policies()
        .into_iter()
        .filter(|policy| linked_alias_matches_project(project_id, &policy.alias))
        .map(|policy| ResourceEntry {
            uri: format!("project://{project_id}/linked-folders/{}/", policy.alias),
            name: policy.alias.clone(),
            kind: "linked_folder".to_string(),
            title: Some(format!("linked://{}/", policy.alias)),
            size_bytes: None,
            modified_at: None,
        })
        .collect()
}

fn project_memory_view(project_id: &str, policy: &tm_host::FsPolicy) -> Value {
    let scope = project_memory_scope(project_id);
    let encoded = crate::memory::encode_memory_segment(&scope);
    json!({
        "scope": scope,
        "chunksUri": format!("memory://scopes/{encoded}/chunks"),
        "mode": fs_mode_label(policy.mode),
        "linkedUri": format!("linked://{}/", policy.alias),
        "activeRootUri": "memory://root",
        "note": "memory://root and memory://summaries resolve using the active session scope; chunksUri is the explicit project-scope recall surface."
    })
}

fn project_memory_entries(project_id: &str, policy: &tm_host::FsPolicy) -> Vec<ResourceEntry> {
    let scope = project_memory_scope(project_id);
    let encoded = crate::memory::encode_memory_segment(&scope);
    vec![ResourceEntry {
        uri: format!("memory://scopes/{encoded}/chunks"),
        name: "chunks".to_string(),
        kind: "memory_scope".to_string(),
        title: Some(format!("{} ({})", scope, fs_mode_label(policy.mode))),
        size_bytes: None,
        modified_at: None,
    }]
}

fn project_memory_scope(project_id: &str) -> String {
    format!("project:{project_id}")
}

fn project_linked_uri(view: &str) -> Option<String> {
    view.strip_prefix("linked-folders/")
        .filter(|path| !path.trim().is_empty())
        .map(|path| format!("linked://{path}"))
}

fn ensure_project_linked_alias_active(
    project_id: &str,
    view: &str,
    linked_folders: &LinkedFolders,
) -> Result<()> {
    let alias = project_linked_alias(view).ok_or_else(|| {
        ServerError::Policy(format!(
            "project linked-folder view project://{project_id}/{view} is not active"
        ))
    })?;
    if !linked_alias_matches_project(project_id, alias) {
        return Err(ServerError::Policy(format!(
            "linked folder {alias} is not in project {project_id}"
        )));
    }
    linked_folders.policy(alias).map_err(map_host_error)?;
    Ok(())
}

fn active_project_link_policy(
    project_id: &str,
    linked_folders: &LinkedFolders,
) -> Option<tm_host::FsPolicy> {
    linked_folders
        .policies()
        .into_iter()
        .find(|policy| linked_alias_matches_project(project_id, &policy.alias))
}

fn inactive_project_memory_scope_error(project_id: &str) -> ServerError {
    ServerError::Policy(format!(
        "project memory scope {} is not active; link the project folder first",
        project_memory_scope(project_id)
    ))
}

fn project_linked_alias(view: &str) -> Option<&str> {
    view.strip_prefix("linked-folders/")
        .and_then(|path| path.split('/').find(|part| !part.trim().is_empty()))
}

fn linked_alias_matches_project(project_id: &str, alias: &str) -> bool {
    linked_project_key(project_id) == linked_project_key(alias)
}

fn linked_project_key(value: &str) -> String {
    tm_drive::slug(value).replace('-', "")
}

fn fs_mode_label(mode: tm_host::FsMode) -> &'static str {
    match mode {
        tm_host::FsMode::Ro => "ro",
        tm_host::FsMode::Rw => "rw",
    }
}

fn linked_to_project_uri(project_id: &str, linked_uri: &str) -> String {
    linked_uri
        .strip_prefix("linked://")
        .map(|path| format!("project://{project_id}/linked-folders/{path}"))
        .unwrap_or_else(|| linked_uri.to_string())
}

fn parse_project_uri(uri: &str) -> Result<(String, Option<String>)> {
    let rest = uri
        .strip_prefix("project://")
        .ok_or_else(|| ServerError::Policy(format!("unsupported project uri {uri}")))?;
    let (project_id, view) = rest.split_once('/').map_or((rest, None), |(id, view)| {
        (id, (!view.is_empty()).then(|| view.to_string()))
    });
    if project_id.is_empty() {
        return Err(ServerError::Policy("missing project id".to_string()));
    }
    Ok((project_id.to_string(), view))
}

fn select_text(content: &str, selector: Option<&str>) -> Result<(String, bool)> {
    let Some(selector) = selector else {
        return Ok((content.to_string(), false));
    };
    let (start, end) = selector
        .split_once('-')
        .ok_or_else(|| ServerError::InvalidRequest(format!("invalid selector {selector}")))?;
    let start: usize = start
        .parse()
        .map_err(|_| ServerError::InvalidRequest(format!("invalid selector {selector}")))?;
    let end: usize = end
        .parse()
        .map_err(|_| ServerError::InvalidRequest(format!("invalid selector {selector}")))?;
    if start == 0 || end < start {
        return Err(ServerError::InvalidRequest(format!(
            "invalid selector {selector}"
        )));
    }
    let lines = content.lines().collect::<Vec<_>>();
    let selected = lines
        .iter()
        .skip(start - 1)
        .take(end - start + 1)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Ok((selected, end < lines.len()))
}

fn map_host_error(err: tm_host::HostError) -> ServerError {
    match err {
        tm_host::HostError::UnknownScheme { .. }
        | tm_host::HostError::CapabilityDenied(_)
        | tm_host::HostError::InvalidPath(_) => ServerError::Policy(err.to_string()),
        tm_host::HostError::NotFound(target) => ServerError::NotFound(target),
        other => ServerError::InvalidRequest(other.to_string()),
    }
}

fn map_artifact_error(err: tm_artifacts::ArtifactError) -> ServerError {
    match err {
        tm_artifacts::ArtifactError::NotFound { uri, .. } => ServerError::NotFound(uri),
        tm_artifacts::ArtifactError::InvalidUri(uri) => ServerError::InvalidRequest(uri),
        tm_artifacts::ArtifactError::InvalidSelector(selector) => {
            ServerError::InvalidRequest(selector)
        }
        tm_artifacts::ArtifactError::Io(err) => ServerError::Store(err.to_string()),
    }
}

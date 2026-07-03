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
        Some("project") => read_project_resource(state, session_id, uri).await,
        Some("memory") => read_memory_resource(state, session_id, uri, selector).await,
        Some(scheme) => Err(ServerError::Policy(format!(
            "unknown resource scheme {scheme}; registered: artifact, linked, workspace, project, memory"
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
        return Ok(["artifact", "linked", "workspace", "project", "memory"]
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
        Some(scheme) => Err(ServerError::Policy(format!(
            "unknown resource scheme {scheme}; registered: artifact, linked, workspace, project, memory"
        ))),
        None => Err(ServerError::InvalidRequest(format!(
            "invalid resource uri {uri}"
        ))),
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
        session.mode_state.mode.default_scope(),
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
        session.mode_state.mode.default_scope(),
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
        session.mode_state.mode.default_scope(),
    )));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:memory"));
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
) -> Result<ResourceContent>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let (project_id, view) = parse_project_uri(uri)?;
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
    Ok(["open-loops", "decisions", "next-actions", "resources"]
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

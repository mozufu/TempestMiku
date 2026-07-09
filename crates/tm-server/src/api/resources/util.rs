use super::schemes::{list_linked_resources, read_linked_resource};
use super::*;

pub(crate) fn validate_relative_path(path: &str) -> Result<PathBuf> {
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

pub(crate) fn workspace_uri(relative: &FsPath) -> String {
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

pub(crate) async fn read_project_resource<S, M, C>(
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

pub(crate) async fn list_project_resources<S, M, C>(
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

pub(super) fn select_text(content: &str, selector: Option<&str>) -> Result<(String, bool)> {
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

pub(crate) fn map_host_error(err: tm_host::HostError) -> ServerError {
    match err {
        tm_host::HostError::UnknownScheme { .. }
        | tm_host::HostError::CapabilityDenied(_)
        | tm_host::HostError::InvalidPath(_) => ServerError::Policy(err.to_string()),
        tm_host::HostError::NotFound(target) => ServerError::NotFound(target),
        other => ServerError::InvalidRequest(other.to_string()),
    }
}

pub(crate) fn map_artifact_error(err: tm_artifacts::ArtifactError) -> ServerError {
    match err {
        tm_artifacts::ArtifactError::NotFound { uri, .. } => ServerError::NotFound(uri),
        tm_artifacts::ArtifactError::InvalidUri(uri) => ServerError::InvalidRequest(uri),
        tm_artifacts::ArtifactError::InvalidSelector(selector) => {
            ServerError::InvalidRequest(selector)
        }
        tm_artifacts::ArtifactError::Io(err) => ServerError::Store(err.to_string()),
    }
}

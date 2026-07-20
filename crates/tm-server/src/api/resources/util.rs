use super::schemes::{list_linked_resources, read_linked_resource};
use super::*;
use std::{
    fs::File,
    io::{BufRead, BufReader},
};

pub(crate) async fn validate_authorized_memory_scope<S>(store: &S, scope: &str) -> Result<()>
where
    S: Store,
{
    if scope == "global" {
        return Ok(());
    }
    let project = scope
        .strip_prefix("project:")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ServerError::InvalidRequest("scope must be global or project:<id>".to_string())
        })?;
    // §30: scope authority is the server-owned project entity, not a live linked-folder alias. A
    // folderless project is a valid scope; an archived project fails closed.
    match store.project(project).await? {
        Some(record) if record.status == crate::ProjectStatus::Active => Ok(()),
        _ => Err(ServerError::NotFound(format!(
            "active project scope {scope}"
        ))),
    }
}

/// Enter a memory scope as the owner (session create / scope switch). `global` is always valid.
/// A `project:<id>` scope lazily creates the entity (owner action, §30.2) but never resurrects an
/// archived one — re-entering an archived project fails closed.
pub(crate) async fn ensure_active_project_scope<S>(store: &S, scope: &str) -> Result<()>
where
    S: Store,
{
    if scope == "global" {
        return Ok(());
    }
    let project = scope
        .strip_prefix("project:")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ServerError::InvalidRequest("scope must be global or project:<id>".to_string())
        })?;
    let record = store.ensure_project(project, project).await?;
    if record.status != crate::ProjectStatus::Active {
        return Err(ServerError::NotFound(format!(
            "active project scope {scope}"
        )));
    }
    Ok(())
}

pub(crate) fn authorized_project_id(
    session: &crate::SessionRecord,
    requested: Option<&str>,
) -> Result<String> {
    let project_id = session
        .memory_scope
        .strip_prefix("project:")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ServerError::NotFound("active project scope".to_string()))?;
    if requested.is_some_and(|requested| requested != project_id) {
        return Err(ServerError::NotFound(format!(
            "project {}",
            requested.unwrap_or_default()
        )));
    }
    Ok(project_id.to_string())
}

pub(crate) fn authorized_linked_alias(session: &crate::SessionRecord, uri: &str) -> Result<String> {
    let alias = uri
        .strip_prefix("linked://")
        .and_then(|path| path.split('/').next())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ServerError::NotFound("linked resource".to_string()))?;
    let project_id = authorized_project_id(session, None)?;
    if alias != project_id {
        return Err(ServerError::NotFound(format!("linked resource {uri}")));
    }
    Ok(alias.to_string())
}

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
    let session = state.store.get_session(session_id).await?;
    validate_authorized_memory_scope(state.store.as_ref(), &session.memory_scope).await?;
    authorized_project_id(&session, Some(&project_id))?;
    if let Some(view) = view.as_deref()
        && let Some(linked_uri) = project_linked_uri(view)
    {
        ensure_project_linked_alias_active(&project_id, view, &state.linked_folders)?;
        let mut content = read_linked_resource(
            &state.linked_folders,
            state.linked_resource_handler.as_ref(),
            &linked_uri,
            selector,
            session_id,
            &session.memory_scope,
        )
        .await?;
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
            let policy = active_project_link_policy(&project_id, &state.linked_folders);
            project_memory_view(&project_id, policy.as_ref())
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
    if uri == "project://" {
        state.store.get_session(session_id).await?;
        return crate::api::projects::project_resource_entries(state).await;
    }
    let (project_id, view) = parse_project_uri(uri)?;
    let session = state.store.get_session(session_id).await?;
    validate_authorized_memory_scope(state.store.as_ref(), &session.memory_scope).await?;
    authorized_project_id(&session, Some(&project_id))?;
    if let Some(view) = view.as_deref() {
        if view == "memory" {
            let policy = active_project_link_policy(&project_id, &state.linked_folders);
            return Ok(project_memory_entries(&project_id, policy.as_ref()));
        }
        if view == "linked-folders" {
            return Ok(project_linked_folder_entries(
                &project_id,
                &state.linked_folders,
            ));
        }
        if let Some(linked_uri) = project_linked_uri(view) {
            ensure_project_linked_alias_active(&project_id, view, &state.linked_folders)?;
            let entries = list_linked_resources(
                &state.linked_folders,
                state.linked_resource_handler.as_ref(),
                Some(&linked_uri),
                session_id,
                &session.memory_scope,
            )
            .await?;
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
    // §30: project memory exists on the entity, not the folder, so the memory view is always listed.
    views.insert(4, "memory");
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
        .aliases()
        .into_iter()
        .filter(|alias| linked_alias_matches_project(project_id, alias))
        .map(|alias| ResourceEntry {
            uri: format!("project://{project_id}/linked-folders/{alias}/"),
            name: alias.clone(),
            kind: "linked_folder".to_string(),
            title: Some(format!("linked://{alias}/")),
            size_bytes: None,
            modified_at: None,
        })
        .collect()
}

fn project_memory_view(project_id: &str, policy: Option<&tm_host::FsPolicy>) -> Value {
    let scope = project_memory_scope(project_id);
    let encoded = crate::memory::encode_memory_segment(&scope);
    json!({
        "scope": scope,
        "chunksUri": format!("memory://scopes/{encoded}/chunks"),
        "mode": policy.map(|policy| fs_mode_label(policy.mode)),
        "linkedUri": policy.map(|policy| format!("linked://{}/", policy.alias)),
        "activeRootUri": "memory://root",
        "note": "memory://root and memory://summaries resolve using the active session scope; chunksUri is the explicit project-scope recall surface. Project memory is owned by the project entity and exists with or without a linked folder (§30)."
    })
}

fn project_memory_entries(
    project_id: &str,
    policy: Option<&tm_host::FsPolicy>,
) -> Vec<ResourceEntry> {
    let scope = project_memory_scope(project_id);
    let encoded = crate::memory::encode_memory_segment(&scope);
    let title = match policy {
        Some(policy) => format!("{} ({})", scope, fs_mode_label(policy.mode)),
        None => scope.clone(),
    };
    vec![ResourceEntry {
        uri: format!("memory://scopes/{encoded}/chunks"),
        name: "chunks".to_string(),
        kind: "memory_scope".to_string(),
        title: Some(title),
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
    if !linked_folders.contains_alias(alias) {
        return Err(ServerError::Policy(format!(
            "linked folder {alias} is not active"
        )));
    }
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

const DEFAULT_READ_LINES: usize = 200;
const DEFAULT_READ_BYTES: usize = 64 * 1024;
const MAX_READ_LINES: usize = 1_000;
const MAX_READ_BYTES: usize = 256 * 1024;

pub(super) fn read_text_page(path: &FsPath, selector: Option<&str>) -> Result<(String, bool)> {
    let (start, end, byte_limit) = parse_text_selector(selector)?;
    let normalize_selected_lines = selector.is_some();
    let file = File::open(path).map_err(|err| ServerError::Store(err.to_string()))?;
    let mut reader = BufReader::new(file);
    for _ in 1..start {
        if reader
            .skip_until(b'\n')
            .map_err(|err| ServerError::Store(err.to_string()))?
            == 0
        {
            return Ok((String::new(), false));
        }
    }

    let mut selected = Vec::new();
    let mut truncated = false;
    for (selected_lines, _) in (start..=end).enumerate() {
        let separator = usize::from(normalize_selected_lines && selected_lines > 0);
        let remaining = byte_limit.saturating_sub(selected.len().saturating_add(separator));
        let Some((mut line, line_truncated)) = read_bounded_line(&mut reader, remaining)? else {
            break;
        };
        if normalize_selected_lines {
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if separator == 1 {
                selected.push(b'\n');
            }
        }
        selected.extend_from_slice(&line);
        if line_truncated {
            truncated = true;
            break;
        }
    }
    let has_more = truncated
        || !reader
            .fill_buf()
            .map_err(|err| ServerError::Store(err.to_string()))?
            .is_empty();
    if let Err(error) = std::str::from_utf8(&selected) {
        if error.error_len().is_some() {
            return Err(ServerError::InvalidRequest(
                "workspace reads support UTF-8 text only".to_string(),
            ));
        }
        selected.truncate(error.valid_up_to());
    }
    let selected = String::from_utf8(selected).map_err(|_| {
        ServerError::InvalidRequest("workspace reads support UTF-8 text only".to_string())
    })?;
    Ok((selected, has_more))
}

fn parse_text_selector(selector: Option<&str>) -> Result<(usize, usize, usize)> {
    let Some(selector) = selector else {
        return Ok((1, DEFAULT_READ_LINES, DEFAULT_READ_BYTES));
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
    let line_count = end
        .checked_sub(start)
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| ServerError::InvalidRequest(format!("invalid selector {selector}")))?;
    if start == 0 || end < start || line_count > MAX_READ_LINES {
        return Err(ServerError::InvalidRequest(format!(
            "invalid selector {selector}"
        )));
    }
    Ok((start, end, MAX_READ_BYTES))
}

fn read_bounded_line<R: BufRead>(reader: &mut R, limit: usize) -> Result<Option<(Vec<u8>, bool)>> {
    let mut line = Vec::new();
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|err| ServerError::Store(err.to_string()))?;
        if buffer.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some((line, false)))
            };
        }
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let content = &buffer[..=newline];
            let available = limit.saturating_sub(line.len());
            let copied = content.len().min(available);
            line.extend_from_slice(&content[..copied]);
            if copied < content.len() {
                reader.consume(copied);
                return Ok(Some((line, true)));
            }
            reader.consume(newline + 1);
            return Ok(Some((line, false)));
        }

        let available = limit.saturating_sub(line.len());
        let buffer_len = buffer.len();
        let copied = buffer_len.min(available);
        line.extend_from_slice(&buffer[..copied]);
        reader.consume(copied);
        if copied < buffer_len || available == 0 {
            return Ok(Some((line, true)));
        }
    }
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
        other @ tm_artifacts::ArtifactError::QuotaExceeded { .. }
        | other @ tm_artifacts::ArtifactError::InvalidLimits(_) => {
            ServerError::InvalidRequest(other.to_string())
        }
        tm_artifacts::ArtifactError::Integrity(target) => {
            ServerError::Store(format!("artifact integrity check failed for {target}"))
        }
        tm_artifacts::ArtifactError::Io(err) => ServerError::Store(err.to_string()),
    }
}

#[cfg(test)]
mod paging_tests {
    use super::*;

    #[test]
    fn workspace_text_pages_are_bounded_before_full_file_loading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let content = (0..400)
            .map(|line| format!("{line:04} {}\n", "x".repeat(512)))
            .collect::<String>();
        std::fs::write(&path, content).unwrap();

        let (page, has_more) = read_text_page(&path, None).unwrap();
        assert!(page.len() <= DEFAULT_READ_BYTES);
        assert!(has_more);
        assert!(matches!(
            read_text_page(&path, Some("1-1001")),
            Err(ServerError::InvalidRequest(_))
        ));
    }
}

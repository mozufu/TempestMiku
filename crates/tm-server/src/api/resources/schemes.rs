use super::dispatch::registered_resource_schemes;
use super::util::{map_host_error, read_text_page};
use super::*;

pub(super) async fn read_skill_resource<S, M, C>(
    state: &AppState<S, M, C>,
    uri: &str,
    selector: Option<&str>,
) -> Result<ResourceContent>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    if selector.is_some() {
        return Err(ServerError::InvalidRequest(
            "skill resources do not support selectors".to_string(),
        ));
    }
    let view = parse_skill_uri(uri)?;
    let (kind, title, content) = match view {
        SkillUri::Root => (
            "skill_catalog",
            Some("Managed skills".to_string()),
            serde_json::to_string_pretty(
                &state
                    .persona
                    .managed_skills()
                    .map_err(managed_skill_resource_error)?,
            )?,
        ),
        SkillUri::Active { name } => {
            let (version, body) = state
                .persona
                .managed_skill_body(&name)
                .map_err(managed_skill_resource_error)?;
            (
                "managed_skill",
                Some(format!("{} ({})", version.name, version.content_digest)),
                body,
            )
        }
        SkillUri::Versions { name } => (
            "skill_versions",
            Some(format!("{name} versions")),
            serde_json::to_string_pretty(
                &state
                    .persona
                    .managed_skill(&name)
                    .map_err(managed_skill_resource_error)?,
            )?,
        ),
        SkillUri::Version { name, digest } => {
            let (version, body) = state
                .persona
                .managed_skill_version_body(&name, &digest)
                .map_err(managed_skill_resource_error)?;
            (
                "managed_skill_version",
                Some(format!("{} ({})", version.name, version.content_digest)),
                body,
            )
        }
    };
    Ok(ResourceContent {
        uri: uri.to_string(),
        kind: kind.to_string(),
        mime: if kind.contains("catalog") || kind.contains("versions") {
            "application/json".to_string()
        } else {
            "text/markdown; charset=utf-8".to_string()
        },
        title,
        size_bytes: content.len(),
        selector: None,
        has_more: false,
        preview: preview(&content, 1024),
        content,
    })
}

pub(super) async fn list_skill_resources<S, M, C>(
    state: &AppState<S, M, C>,
    uri: &str,
) -> Result<Vec<ResourceEntry>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    match parse_skill_uri(uri)? {
        SkillUri::Root => Ok(state
            .persona
            .managed_skills()
            .map_err(managed_skill_resource_error)?
            .into_iter()
            .map(|skill| ResourceEntry {
                uri: format!("skill://{}", skill.active.name),
                name: skill.active.name.clone(),
                kind: "managed_skill".to_string(),
                title: Some(skill.active.description),
                size_bytes: None,
                modified_at: None,
            })
            .collect()),
        SkillUri::Active { name } | SkillUri::Versions { name } => Ok(state
            .persona
            .managed_skill(&name)
            .map_err(managed_skill_resource_error)?
            .versions
            .into_iter()
            .map(|version| ResourceEntry {
                uri: format!(
                    "skill://{}/versions/{}",
                    version.name,
                    version.content_digest.trim_start_matches("sha256:")
                ),
                name: version.content_digest.clone(),
                kind: "managed_skill_version".to_string(),
                title: Some(version.description),
                size_bytes: None,
                modified_at: None,
            })
            .collect()),
        SkillUri::Version { .. } => Err(ServerError::Policy(
            "skill version listing requires skill://<name>/versions".to_string(),
        )),
    }
}

enum SkillUri {
    Root,
    Active { name: String },
    Versions { name: String },
    Version { name: String, digest: String },
}

fn parse_skill_uri(uri: &str) -> Result<SkillUri> {
    let path = uri
        .strip_prefix("skill://")
        .ok_or_else(|| ServerError::Policy(format!("unsupported skill uri {uri}")))?;
    if path.is_empty() || path == "root" {
        return Ok(SkillUri::Root);
    }
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        [name] if valid_skill_resource_name(name) => Ok(SkillUri::Active {
            name: (*name).to_string(),
        }),
        [name, "versions"] if valid_skill_resource_name(name) => Ok(SkillUri::Versions {
            name: (*name).to_string(),
        }),
        [name, "versions", digest]
            if valid_skill_resource_name(name)
                && digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) =>
        {
            Ok(SkillUri::Version {
                name: (*name).to_string(),
                digest: format!("sha256:{digest}"),
            })
        }
        _ => Err(ServerError::Policy(format!("unsupported skill uri {uri}"))),
    }
}

fn valid_skill_resource_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn managed_skill_resource_error(error: tm_modes::ManagedSkillError) -> ServerError {
    ServerError::NotFound(error.to_string())
}

pub(super) async fn read_cron_resource<S, M, C>(
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

pub(super) async fn list_cron_resources<S, M, C>(
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

pub(super) fn resource_scheme(uri: &str) -> Option<String> {
    uri.split_once("://").map(|(scheme, _)| scheme.to_string())
}

pub(super) async fn read_linked_resource(
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

pub(super) async fn list_linked_resources(
    linked_folders: &LinkedFolders,
    uri: Option<&str>,
) -> Result<Vec<ResourceEntry>> {
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(LinkedResourceHandler::new(linked_folders.clone())));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:linked"));
    registry.list(uri, &ctx).await.map_err(map_host_error)
}

pub(super) async fn read_drive_resource<S, M, C>(
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
    let drive_store = state.drive_store.clone().ok_or_else(|| {
        ServerError::Policy(format!(
            "unknown resource scheme drive; registered: {}",
            registered_resource_schemes(state).join(", ")
        ))
    })?;
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(tm_drive::DriveResourceHandler::new(drive_store)));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:drive"))
        .with_session_id(session_id.to_string())
        .with_session_scope(session.memory_scope);
    registry
        .read(uri, selector, &ctx)
        .await
        .map_err(map_host_error)
}

pub(super) async fn list_drive_resources<S, M, C>(
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
    let drive_store = state.drive_store.clone().ok_or_else(|| {
        ServerError::Policy(format!(
            "unknown resource scheme drive; registered: {}",
            registered_resource_schemes(state).join(", ")
        ))
    })?;
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(tm_drive::DriveResourceHandler::new(drive_store)));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:drive"))
        .with_session_id(session_id.to_string())
        .with_session_scope(session.memory_scope);
    registry.list(Some(uri), &ctx).await.map_err(map_host_error)
}

pub(super) async fn read_memory_resource<S, M, C>(
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
    super::util::validate_authorized_memory_scope(&state.linked_folders, &session.memory_scope)?;
    let subject = session.owner_subject;
    let scope = session.memory_scope;
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(crate::memory::MemoryResourceHandler::new(
        Arc::clone(&state.store),
        &subject,
        &scope,
    )));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:memory"))
        .with_session_id(session_id.to_string())
        .with_memory_authority(subject, scope);
    registry
        .read(uri, selector, &ctx)
        .await
        .map_err(map_host_error)
}

pub(super) async fn preview_memory_resource<S, M, C>(
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
    super::util::validate_authorized_memory_scope(&state.linked_folders, &session.memory_scope)?;
    let subject = session.owner_subject;
    let scope = session.memory_scope;
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(crate::memory::MemoryResourceHandler::new(
        Arc::clone(&state.store),
        &subject,
        &scope,
    )));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:memory"))
        .with_session_id(session_id.to_string())
        .with_memory_authority(subject, scope);
    registry.preview(uri, &ctx).await.map_err(map_host_error)
}

pub(super) async fn list_memory_resources<S, M, C>(
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
    super::util::validate_authorized_memory_scope(&state.linked_folders, &session.memory_scope)?;
    let subject = session.owner_subject;
    let scope = session.memory_scope;
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(crate::memory::MemoryResourceHandler::new(
        Arc::clone(&state.store),
        &subject,
        &scope,
    )));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:memory"))
        .with_session_id(session_id.to_string())
        .with_memory_authority(subject, scope);
    registry.list(Some(uri), &ctx).await.map_err(map_host_error)
}

pub(super) async fn read_agent_resource<S, M, C>(
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

pub(super) async fn read_history_resource<S, M, C>(
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

pub(super) async fn list_agent_resources<S, M, C>(
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

pub(super) fn compact_resource_preview(mut content: ResourceContent) -> ResourceContent {
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

pub(super) fn read_workspace_resource(
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
        .map_err(|_| ServerError::NotFound("workspace://session/".to_string()))?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| ServerError::NotFound(uri.to_string()))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(ServerError::Policy(format!(
            "workspace path escapes session root: {uri}"
        )));
    }
    let size_bytes = fs::metadata(&canonical_path)
        .map_err(|err| ServerError::Store(err.to_string()))?
        .len()
        .try_into()
        .map_err(|_| ServerError::Store("workspace file is too large".to_string()))?;
    let (selected, has_more) = read_text_page(&canonical_path, selector)?;
    Ok(ResourceContent {
        uri: super::util::workspace_uri(&relative),
        kind: "text".to_string(),
        mime: "text/plain".to_string(),
        title: canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string),
        size_bytes,
        selector: selector.map(str::to_string),
        has_more,
        preview: preview(&selected, 1024),
        content: selected,
    })
}

pub(super) fn list_workspace_resources(
    artifact_root: &FsPath,
    session_id: Uuid,
    uri: &str,
) -> Result<Vec<ResourceEntry>> {
    let relative = workspace_relative(uri)?;
    let root = workspace_root(artifact_root, session_id);
    let dir = root.join(&relative);
    let canonical_root = root
        .canonicalize()
        .map_err(|_| ServerError::NotFound("workspace://session/".to_string()))?;
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
            uri: super::util::workspace_uri(rel),
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
    super::util::validate_relative_path(path)
}

use super::schemes::{
    compact_resource_preview, list_agent_resources, list_cron_resources, list_drive_resources,
    list_linked_resources, list_memory_resources, list_skill_resources, list_workspace_resources,
    preview_memory_resource, read_agent_resource, read_cron_resource, read_drive_resource,
    read_history_resource, read_linked_resource, read_mcp_resource, read_memory_resource,
    read_skill_resource, read_workspace_resource, resource_scheme,
};
use super::util::map_artifact_error;
use super::*;

pub(crate) async fn read_resource_content<S, M, C>(
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
        Some("linked") => {
            let session = state.store.get_session(session_id).await?;
            super::util::authorized_linked_alias(&session, uri)?;
            read_linked_resource(
                &state.linked_folders,
                state.linked_resource_handler.as_ref(),
                uri,
                selector,
                session_id,
                &session.memory_scope,
            )
            .await
        }
        Some("project") => {
            super::util::read_project_resource(state, session_id, uri, selector).await
        }
        Some("memory") => read_memory_resource(state, session_id, uri, selector).await,
        Some("skill") if state.persona.managed_skills_path().is_some() => {
            read_skill_resource(state, uri, selector).await
        }
        Some("agent") => read_agent_resource(state, session_id, uri, selector).await,
        Some("history") => read_history_resource(state, session_id, uri, selector).await,
        Some("cron") => read_cron_resource(state, uri).await,
        Some("drive") => read_drive_resource(state, session_id, uri, selector).await,
        Some("mcp") if state.mcp_resource_handler.is_some() => {
            read_mcp_resource(state, session_id, uri, selector).await
        }
        Some(scheme) => Err(ServerError::Policy(format!(
            "unknown resource scheme {scheme}; registered: {}",
            registered_resource_schemes(state).join(", ")
        ))),
        None => Err(ServerError::InvalidRequest(format!(
            "invalid resource uri {uri}"
        ))),
    }
}

pub(crate) async fn preview_resource_content<S, M, C>(
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
        Some("mcp") if state.mcp_resource_handler.is_some() => {
            super::schemes::preview_mcp_resource(state, session_id, uri).await?
        }
        _ => read_resource_content(state, session_id, uri, None).await?,
    };
    Ok(compact_resource_preview(content))
}

pub(crate) async fn list_resource_entries<S, M, C>(
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
        Some("linked") => {
            let session = state.store.get_session(session_id).await?;
            super::util::authorized_linked_alias(&session, uri)?;
            list_linked_resources(
                &state.linked_folders,
                state.linked_resource_handler.as_ref(),
                Some(uri),
                session_id,
                &session.memory_scope,
            )
            .await
        }
        Some("project") => super::util::list_project_resources(state, session_id, uri).await,
        Some("memory") => list_memory_resources(state, session_id, uri).await,
        Some("skill") if state.persona.managed_skills_path().is_some() => {
            list_skill_resources(state, uri).await
        }
        Some("agent") => list_agent_resources(state, session_id, uri).await,
        Some("history") => Err(ServerError::Policy(
            "history:// listing not supported — read a specific history://<id>".to_string(),
        )),
        Some("cron") => list_cron_resources(state, uri).await,
        Some("drive") => list_drive_resources(state, session_id, uri).await,
        Some("mcp") if state.mcp_resource_handler.is_some() => {
            super::schemes::list_mcp_resources(state, session_id, uri).await
        }
        Some(scheme) => Err(ServerError::Policy(format!(
            "unknown resource scheme {scheme}; registered: {}",
            registered_resource_schemes(state).join(", ")
        ))),
        None => Err(ServerError::InvalidRequest(format!(
            "invalid resource uri {uri}"
        ))),
    }
}

pub(super) fn registered_resource_schemes<S, M, C>(state: &AppState<S, M, C>) -> Vec<&'static str> {
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
    if state.persona.managed_skills_path().is_some() {
        schemes.push("skill");
    }
    if state.mcp_resource_handler.is_some() {
        schemes.push("mcp");
    }
    schemes
}

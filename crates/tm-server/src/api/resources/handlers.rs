use super::dispatch::{list_resource_entries, preview_resource_content, read_resource_content};
use super::util::map_artifact_error;
use super::*;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ArtifactReadQuery {
    selector: Option<String>,
}

pub(crate) async fn list_artifacts<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.store.get_session(session_id).await?;
    let store = tm_artifacts::ArtifactStore::open(&state.artifact_root, session_id.to_string())
        .map_err(|err| ServerError::Store(err.to_string()))?;
    Ok(Json(serde_json::to_value(store.list())?))
}

pub(crate) async fn read_artifact<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, artifact_id)): Path<(Uuid, String)>,
    Query(query): Query<ArtifactReadQuery>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
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
pub(crate) struct ResourceQuery {
    uri: Option<String>,
    selector: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DriveFeedQuery {
    limit: Option<usize>,
    project: Option<String>,
}

pub(crate) async fn drive_feed<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<DriveFeedQuery>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    super::util::validate_authorized_memory_scope(&state.linked_folders, &session.memory_scope)?;
    let project = super::util::authorized_project_id(&session, query.project.as_deref())?;
    let store = state
        .drive_store
        .as_ref()
        .ok_or_else(|| ServerError::Policy("drive store is not configured".to_string()))?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let recent = store
        .search(tm_drive::DriveSearchOptions {
            project: Some(project.clone()),
            limit,
            return_snippets: true,
            ..tm_drive::DriveSearchOptions::default()
        })
        .await
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
        .collect::<Vec<_>>();
    let project_path_prefix = format!("projects/{}/", tm_drive::slug(&project));
    let proposals = store
        .proposals()
        .await
        .map_err(|err| ServerError::Store(err.to_string()))?
        .into_iter()
        .filter(|proposal| {
            proposal.status == tm_drive::ProposalStatus::Pending
                && (proposal.source_path.starts_with(&project_path_prefix)
                    || proposal
                        .proposed_path
                        .as_deref()
                        .is_some_and(|path| path.starts_with(&project_path_prefix)))
        })
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

pub(crate) async fn resolve_resource<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<ResourceQuery>,
) -> Result<Json<ResourceContent>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.store.get_session(session_id).await?;
    let uri = query.uri.ok_or_else(|| {
        ServerError::InvalidRequest("uri query parameter is required".to_string())
    })?;
    read_resource_content(&state, session_id, &uri, query.selector.as_deref())
        .await
        .map(Json)
}

pub(crate) async fn preview_resource<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<ResourceQuery>,
) -> Result<Json<ResourceContent>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.store.get_session(session_id).await?;
    let uri = query.uri.ok_or_else(|| {
        ServerError::InvalidRequest("uri query parameter is required".to_string())
    })?;
    preview_resource_content(&state, session_id, &uri)
        .await
        .map(Json)
}

pub(crate) async fn list_resources<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<ResourceQuery>,
) -> Result<Json<Vec<ResourceEntry>>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.store.get_session(session_id).await?;
    list_resource_entries(&state, session_id, query.uri.as_deref())
        .await
        .map(Json)
}

use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromoteSessionRequest {
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    open_loops: Vec<String>,
    #[serde(default)]
    decisions: Vec<String>,
    #[serde(default)]
    resources: Vec<String>,
    #[serde(default)]
    import_resources_to_drive: bool,
    #[serde(default = "default_user_initiator")]
    initiated_by: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

fn default_user_initiator() -> String {
    "user".to_string()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromoteSessionResponse {
    project_uri: String,
    project_id: String,
    promoted: Vec<ProjectItemRecord>,
}

pub(crate) async fn promote_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Json(payload): Json<PromoteSessionRequest>,
) -> Result<Json<PromoteSessionResponse>>
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
    let project_id =
        resources::util::authorized_project_id(&session, payload.project_id.as_deref())?;
    if payload.initiated_by == "miku" {
        let timeout = Duration::from_millis(payload.timeout_ms.unwrap_or(60_000));
        let sink = Arc::new(StoreCodingEventSink::new(
            session_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        ));
        let outcome = state
            .approval_broker
            .request_permission_for_backend(
                session_id,
                "project-promotion",
                crate::ApprovalPrompt {
                    action: format!("Promote session {session_id} into project://{project_id}"),
                    scope: json!({
                        "projectId": project_id,
                        "sessionId": session_id,
                    }),
                    options: vec![
                        crate::ApprovalOption {
                            option_id: "allow".to_string(),
                            name: "Promote".to_string(),
                            kind: "allow_once".to_string(),
                        },
                        crate::ApprovalOption {
                            option_id: "reject".to_string(),
                            name: "Reject".to_string(),
                            kind: "reject_once".to_string(),
                        },
                    ],
                },
                timeout,
                sink,
            )
            .await?;
        if !matches!(
            outcome,
            crate::ApprovalOutcome::Selected { ref option_id } if option_id == "allow"
        ) {
            return Err(ServerError::Policy(
                "project promotion denied by approval policy".to_string(),
            ));
        }
    }
    let promoted = apply_promotion(&state, session_id, project_id.clone(), payload).await?;
    Ok(Json(PromoteSessionResponse {
        project_uri: format!("project://{project_id}"),
        project_id,
        promoted,
    }))
}

async fn apply_promotion<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    project_id: String,
    payload: PromoteSessionRequest,
) -> Result<Vec<ProjectItemRecord>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let mut promoted = Vec::new();
    let selected_at = Utc::now();
    let latest_final = latest_event_payload(state, session_id, "final").await?;
    let summary = payload.summary.or_else(|| {
        latest_final
            .as_ref()
            .and_then(|event| event.payload_json.get("text"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    if let Some(summary) = summary.filter(|summary| !summary.trim().is_empty()) {
        promoted.push(
            state
                .store
                .upsert_project_item(NewProjectItem {
                    project_id: project_id.clone(),
                    kind: ProjectItemKind::Summary,
                    text: summary.trim().to_string(),
                    target_uri: format!("project://{project_id}/summary/{session_id}"),
                    source_session_id: session_id,
                    source_event_seq: latest_final.as_ref().map(|event| event.seq),
                    source_uri: None,
                    dedupe_key: format!("promote:summary:{session_id}"),
                    provenance_json: promotion_provenance(
                        session_id,
                        latest_final.as_ref().map(|event| event.seq),
                        None,
                        &payload.initiated_by,
                        selected_at,
                    ),
                })
                .await?,
        );
    }
    for (idx, text) in payload.open_loops.iter().enumerate() {
        promoted.push(
            state
                .store
                .upsert_project_item(NewProjectItem {
                    project_id: project_id.clone(),
                    kind: ProjectItemKind::OpenLoop,
                    text: text.clone(),
                    target_uri: format!("project://{project_id}/open-loops/{session_id}-{idx}"),
                    source_session_id: session_id,
                    source_event_seq: latest_final.as_ref().map(|event| event.seq),
                    source_uri: None,
                    dedupe_key: format!("promote:open-loop:{session_id}:{idx}:{text}"),
                    provenance_json: promotion_provenance(
                        session_id,
                        latest_final.as_ref().map(|event| event.seq),
                        None,
                        &payload.initiated_by,
                        selected_at,
                    ),
                })
                .await?,
        );
    }
    for (idx, text) in payload.decisions.iter().enumerate() {
        promoted.push(
            state
                .store
                .upsert_project_item(NewProjectItem {
                    project_id: project_id.clone(),
                    kind: ProjectItemKind::Decision,
                    text: text.clone(),
                    target_uri: format!("project://{project_id}/decisions/{session_id}-{idx}"),
                    source_session_id: session_id,
                    source_event_seq: latest_final.as_ref().map(|event| event.seq),
                    source_uri: None,
                    dedupe_key: format!("promote:decision:{session_id}:{idx}:{text}"),
                    provenance_json: promotion_provenance(
                        session_id,
                        latest_final.as_ref().map(|event| event.seq),
                        None,
                        &payload.initiated_by,
                        selected_at,
                    ),
                })
                .await?,
        );
    }
    let import_resources_to_drive = payload.import_resources_to_drive;
    for source_uri in payload.resources {
        let (kind, target_uri) = promoted_resource_target(
            state,
            session_id,
            &project_id,
            &source_uri,
            import_resources_to_drive,
            latest_final.as_ref().map(|event| event.seq),
        )
        .await?;
        promoted.push(
            state
                .store
                .upsert_project_item(NewProjectItem {
                    project_id: project_id.clone(),
                    kind,
                    text: source_uri.clone(),
                    target_uri,
                    source_session_id: session_id,
                    source_event_seq: latest_final.as_ref().map(|event| event.seq),
                    source_uri: Some(source_uri.clone()),
                    dedupe_key: format!("promote:resource:{session_id}:{source_uri}"),
                    provenance_json: promotion_provenance(
                        session_id,
                        latest_final.as_ref().map(|event| event.seq),
                        Some(&source_uri),
                        &payload.initiated_by,
                        selected_at,
                    ),
                })
                .await?,
        );
    }
    Ok(promoted)
}

async fn latest_event_payload<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    event_type: &str,
) -> Result<Option<SessionEvent>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    Ok(state
        .store
        .events_after(session_id, None)
        .await?
        .into_iter()
        .rev()
        .find(|event| event.event_type == event_type))
}

fn promotion_provenance(
    session_id: Uuid,
    source_event_seq: Option<i64>,
    source_uri: Option<&str>,
    actor: &str,
    selected_at: chrono::DateTime<Utc>,
) -> Value {
    json!({
        "sourceSession": session_id,
        "sourceEventId": source_event_seq,
        "sourceUri": source_uri,
        "selectedAt": selected_at,
        "actor": actor,
    })
}

async fn promoted_resource_target<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    project_id: &str,
    source_uri: &str,
    import_resources_to_drive: bool,
    source_event_seq: Option<i64>,
) -> Result<(ProjectItemKind, String)>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    if import_resources_to_drive
        && let Some((relative, relative_slash)) = workspace_source_relative(project_id, source_uri)?
    {
        let target_uri = import_workspace_attachment_to_drive(
            state,
            session_id,
            project_id,
            source_uri,
            &relative,
            &relative_slash,
            source_event_seq,
        )
        .await?;
        return Ok((ProjectItemKind::Workspace, target_uri));
    }
    promoted_resource_pointer_target(project_id, source_uri)
}

fn promoted_resource_pointer_target(
    project_id: &str,
    source_uri: &str,
) -> Result<(ProjectItemKind, String)> {
    if let Some(id) = source_uri.strip_prefix("artifact://") {
        return Ok((
            ProjectItemKind::Artifact,
            format!("project://{project_id}/artifacts/{id}"),
        ));
    }
    if let Some(path) = source_uri.strip_prefix("workspace://session/") {
        validate_relative_path(path)?;
        return Ok((
            ProjectItemKind::Workspace,
            format!("project://{project_id}/workspace/{path}"),
        ));
    }
    if let Some((source_project_id, path)) = project_workspace_source(source_uri) {
        if source_project_id != project_id {
            return Err(ServerError::Policy(format!(
                "cannot promote workspace resource from project://{source_project_id} into project://{project_id}"
            )));
        }
        validate_relative_path(path)?;
        return Ok((ProjectItemKind::Workspace, source_uri.to_string()));
    }
    if let Some(path) = source_uri.strip_prefix("linked://") {
        return Ok((
            ProjectItemKind::Linked,
            format!("project://{project_id}/linked-folders/{path}"),
        ));
    }
    Err(ServerError::Policy(format!(
        "unsupported promotion resource scheme for {source_uri}"
    )))
}

async fn import_workspace_attachment_to_drive<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    project_id: &str,
    source_uri: &str,
    relative: &FsPath,
    relative_slash: &str,
    source_event_seq: Option<i64>,
) -> Result<String>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let drive_store = state.drive_store.as_ref().ok_or_else(|| {
        ServerError::Policy(
            "drive store is not configured for promoted resource import".to_string(),
        )
    })?;
    let bytes = read_session_workspace_attachment(&state.artifact_root, session_id, relative)?;
    let suggested_path = format!(
        "projects/{}/attachments/{}",
        tm_drive::slug(project_id),
        relative_slash
    );
    let filed = drive_store
        .put_bytes(
            &bytes,
            tm_drive::DrivePutOptions {
                auto: true,
                suggested_path: Some(suggested_path),
                project: Some(project_id.to_string()),
                doc_kind: Some("project_attachment".to_string()),
                tags: vec!["project-attachment".to_string()],
                source_uri: Some(source_uri.to_string()),
                approval_mode: tm_drive::DriveApprovalMode::Auto,
                session_id: Some(session_id.to_string()),
                event_seq: source_event_seq,
                ..tm_drive::DrivePutOptions::default()
            },
        )
        .await
        .map_err(|err| ServerError::Store(err.to_string()))?;
    Ok(filed.uri)
}

fn read_session_workspace_attachment(
    artifact_root: &FsPath,
    session_id: Uuid,
    relative: &FsPath,
) -> Result<Vec<u8>> {
    let root = artifact_root
        .join("sessions")
        .join(session_id.to_string())
        .join("workspace");
    let canonical_root = root
        .canonicalize()
        .map_err(|_| ServerError::NotFound("workspace://session/".to_string()))?;
    let path = root.join(relative);
    let canonical_path = path.canonicalize().map_err(|_| {
        ServerError::NotFound(format!("workspace://session/{}", path_slash(relative)))
    })?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(ServerError::Policy(format!(
            "workspace path escapes session root: {}",
            path_slash(relative)
        )));
    }
    if !canonical_path.is_file() {
        return Err(ServerError::Policy(format!(
            "workspace promotion import requires a file: {}",
            path_slash(relative)
        )));
    }
    fs::read(&canonical_path).map_err(|err| ServerError::Store(err.to_string()))
}

fn workspace_source_relative(
    project_id: &str,
    source_uri: &str,
) -> Result<Option<(PathBuf, String)>> {
    if let Some(path) = source_uri.strip_prefix("workspace://session/") {
        let relative = validate_relative_path(path)?;
        let relative_slash = path_slash(&relative);
        return Ok(Some((relative, relative_slash)));
    }
    if let Some((source_project_id, path)) = project_workspace_source(source_uri) {
        if source_project_id != project_id {
            return Err(ServerError::Policy(format!(
                "cannot import workspace resource from project://{source_project_id} into project://{project_id}"
            )));
        }
        let relative = validate_relative_path(path)?;
        let relative_slash = path_slash(&relative);
        return Ok(Some((relative, relative_slash)));
    }
    Ok(None)
}

fn project_workspace_source(source_uri: &str) -> Option<(&str, &str)> {
    let rest = source_uri.strip_prefix("project://")?;
    let (project_id, view) = rest.split_once('/')?;
    let path = view.strip_prefix("workspace/")?;
    Some((project_id, path))
}

fn path_slash(path: &FsPath) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

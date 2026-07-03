use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectQuery {
    project_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectOverview {
    project_id: String,
    project_uri: String,
    session_id: Uuid,
    status: String,
    open_loops: Vec<ProjectItemRecord>,
    decisions: Vec<ProjectItemRecord>,
    next_actions: Vec<ProjectItemRecord>,
    pub(super) resources: Vec<ProjectItemRecord>,
    provenance: Value,
}

pub(super) async fn project_overview<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<ProjectOverview>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let project_id = query
        .project_id
        .unwrap_or_else(|| project_id_from_scope(session.mode_state.mode.default_scope()));
    Ok(Json(
        build_project_overview(&state, session_id, project_id).await?,
    ))
}

pub(super) async fn project_open_loops<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<ProjectItemRecord>>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let project_id = query
        .project_id
        .unwrap_or_else(|| project_id_from_scope(session.mode_state.mode.default_scope()));
    Ok(Json(
        state
            .store
            .project_items(&project_id, Some(ProjectItemKind::OpenLoop))
            .await?,
    ))
}

pub(super) async fn project_decisions<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<ProjectItemRecord>>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let project_id = query
        .project_id
        .unwrap_or_else(|| project_id_from_scope(session.mode_state.mode.default_scope()));
    Ok(Json(
        state
            .store
            .project_items(&project_id, Some(ProjectItemKind::Decision))
            .await?,
    ))
}

pub(super) async fn project_next_actions<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<ProjectItemRecord>>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let project_id = query
        .project_id
        .unwrap_or_else(|| project_id_from_scope(session.mode_state.mode.default_scope()));
    Ok(Json(
        state
            .store
            .project_items(&project_id, Some(ProjectItemKind::NextAction))
            .await?,
    ))
}

pub(super) async fn build_project_overview<S, M, C>(
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

pub(super) async fn record_project_observations<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    project_id: String,
    user_content: &str,
    response: &str,
) -> Result<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let final_event_seq = latest_event_seq(state, session_id, "final").await?;
    let selected_at = Utc::now();
    let summary_text = response.trim().to_string();
    let summary_key = format!("summary:{session_id}:{final_event_seq:?}");
    let summary_uri = format!("project://{project_id}/summary/{session_id}");
    state
        .store
        .upsert_project_item(NewProjectItem {
            project_id: project_id.clone(),
            kind: ProjectItemKind::Summary,
            text: summary_text.clone(),
            target_uri: summary_uri,
            source_session_id: session_id,
            source_event_seq: final_event_seq,
            source_uri: None,
            dedupe_key: summary_key,
            provenance_json: json!({
                "sourceSession": session_id,
                "sourceEventId": final_event_seq,
                "selectedAt": selected_at,
                "actor": "router",
            }),
        })
        .await?;
    state
        .store
        .upsert_project_item(NewProjectItem {
            project_id: project_id.clone(),
            kind: ProjectItemKind::NextAction,
            text: next_action_text(response),
            target_uri: format!("project://{project_id}/next-actions/{session_id}"),
            source_session_id: session_id,
            source_event_seq: final_event_seq,
            source_uri: None,
            dedupe_key: format!("next-action:{session_id}:{final_event_seq:?}"),
            provenance_json: json!({
                "sourceSession": session_id,
                "sourceEventId": final_event_seq,
                "selectedAt": selected_at,
                "actor": "router",
            }),
        })
        .await?;
    let combined = format!("{user_content}\n{response}").to_lowercase();
    if combined.contains("open loop") || combined.contains("todo") || combined.contains("follow up")
    {
        state
            .store
            .upsert_project_item(NewProjectItem {
                project_id: project_id.clone(),
                kind: ProjectItemKind::OpenLoop,
                text: extract_line_with_markers(response, &["open loop", "todo", "follow up"])
                    .unwrap_or_else(|| summary_text.clone()),
                target_uri: format!("project://{project_id}/open-loops/{session_id}"),
                source_session_id: session_id,
                source_event_seq: final_event_seq,
                source_uri: None,
                dedupe_key: format!("open-loop:{session_id}:{final_event_seq:?}"),
                provenance_json: json!({
                    "sourceSession": session_id,
                    "sourceEventId": final_event_seq,
                    "selectedAt": selected_at,
                    "actor": "router",
                }),
            })
            .await?;
    }
    if combined.contains("decision") || combined.contains("decided") {
        state
            .store
            .upsert_project_item(NewProjectItem {
                project_id: project_id.clone(),
                kind: ProjectItemKind::Decision,
                text: extract_line_with_markers(response, &["decision", "decided"])
                    .unwrap_or(summary_text),
                target_uri: format!("project://{project_id}/decisions/{session_id}"),
                source_session_id: session_id,
                source_event_seq: final_event_seq,
                source_uri: None,
                dedupe_key: format!("decision:{session_id}:{final_event_seq:?}"),
                provenance_json: json!({
                    "sourceSession": session_id,
                    "sourceEventId": final_event_seq,
                    "selectedAt": selected_at,
                    "actor": "router",
                }),
            })
            .await?;
    }
    Ok(())
}

async fn latest_event_seq<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    event_type: &str,
) -> Result<Option<i64>>
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
        .find(|event| event.event_type == event_type)
        .map(|event| event.seq))
}

fn next_action_text(response: &str) -> String {
    let first = response
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(response)
        .trim();
    format!("Continue from latest session result: {first}")
}

fn extract_line_with_markers(text: &str, markers: &[&str]) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| {
            let lower = line.to_lowercase();
            markers.iter().any(|marker| lower.contains(marker))
        })
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

pub(super) fn project_id_from_scope(scope: &str) -> String {
    scope
        .strip_prefix("project:")
        .filter(|id| !id.is_empty())
        .unwrap_or("tempestmiku")
        .to_string()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PromoteSessionRequest {
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
pub(super) struct PromoteSessionResponse {
    project_uri: String,
    project_id: String,
    promoted: Vec<ProjectItemRecord>,
}

pub(super) async fn promote_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<PromoteSessionRequest>,
) -> Result<Json<PromoteSessionResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let project_id = payload
        .project_id
        .clone()
        .unwrap_or_else(|| project_id_from_scope(session.mode_state.mode.default_scope()));
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
    for source_uri in payload.resources {
        let (kind, target_uri) = promoted_resource_target(&project_id, &source_uri)?;
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

fn promoted_resource_target(
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

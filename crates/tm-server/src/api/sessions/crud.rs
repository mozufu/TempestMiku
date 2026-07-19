use super::*;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub mode: Option<ModeId>,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionResponse {
    pub id: Uuid,
    pub status: String,
    pub mode: ModeId,
    pub mode_state: ModeState,
    pub persona_status: crate::AssetStatus,
    pub label: String,
    pub voice_cap: String,
    pub default_scope: String,
    pub owner_subject: String,
    pub memory_scope: String,
    #[serde(rename = "activeSkills")]
    pub active_skills: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionSummaryResponse>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummaryResponse {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: String,
    pub mode: ModeId,
    pub label: String,
    pub voice_cap: String,
    #[serde(rename = "activeSkills")]
    pub active_skills: Vec<String>,
    pub title: String,
    pub preview: String,
    pub message_count: i64,
    pub last_event_id: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessagesResponse {
    pub id: Uuid,
    pub status: String,
    pub mode: ModeId,
    pub mode_state: ModeState,
    pub persona_status: crate::AssetStatus,
    pub label: String,
    pub voice_cap: String,
    pub default_scope: String,
    pub owner_subject: String,
    pub memory_scope: String,
    #[serde(rename = "activeSkills")]
    pub active_skills: Vec<String>,
    pub messages: Vec<SessionMessageResponse>,
    pub last_event_id: Option<i64>,
    pub pending_events: Vec<SessionPendingEventResponse>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageResponse {
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub turn_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPendingEventResponse {
    pub id: String,
    pub seq: i64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndSessionRequest {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetSessionScopeRequest {
    pub scope: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionScopeResponse {
    pub id: Uuid,
    pub owner_subject: String,
    pub memory_scope: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndSessionResponse {
    pub id: Uuid,
    pub status: String,
    pub dream: DreamQueueRecord,
}

pub(crate) async fn list_sessions<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Query(query): Query<ListSessionsQuery>,
) -> Result<Json<ListSessionsResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let limit = query.limit.unwrap_or(30).clamp(1, 100);
    let sessions = state
        .store
        .list_sessions(limit)
        .await?
        .into_iter()
        .map(|summary| session_summary_response(&state.persona, summary))
        .collect();
    Ok(Json(ListSessionsResponse { sessions }))
}

pub(crate) async fn create_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    payload: Option<Json<CreateSessionRequest>>,
) -> Result<Json<CreateSessionResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let assets = state.persona.load_assets();
    let payload = payload.map(|Json(payload)| payload).unwrap_or_default();
    let mode = match payload.mode {
        Some(mode) => modes::validate_mode(&state.persona, mode)?,
        None => assets.modes.default_mode(),
    };
    let scope = payload.scope.unwrap_or_else(|| "global".to_string());
    validate_memory_scope(&state.linked_folders, &scope)?;
    let persona_status = assets.status.clone();
    let mut session = state
        .store
        .create_session(NewSession {
            mode: mode.clone(),
            persona_status: persona_status.clone(),
        })
        .await?;
    if session.memory_scope != scope {
        session = state
            .store
            .set_session_memory_scope(session.id, &scope)
            .await?;
    }
    let profile = mode_profile(&state.persona, &session.mode_state.mode);
    let mode_event = state
        .store
        .append_event(
            session.id,
            "mode",
            mode_changed_payload(None, &session.mode_state, persona_status.clone(), &profile)?,
        )
        .await?;
    let _ = state.sender(session.id).send(mode_event);
    Ok(Json(session_response(
        &state.persona,
        session,
        persona_status,
    )))
}

pub(crate) async fn get_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<CreateSessionResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    let persona_status = session.persona_status.clone();
    Ok(Json(session_response(
        &state.persona,
        session,
        persona_status,
    )))
}

pub(crate) async fn get_session_messages<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<SessionMessagesResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    let persona_status = session.persona_status.clone();
    let messages = state
        .store
        .session_messages(session_id)
        .await?
        .into_iter()
        .map(|message| SessionMessageResponse {
            seq: message.seq,
            role: message.role,
            content: message.content,
            turn_id: message.turn_id,
            created_at: message.created_at,
        })
        .collect();
    let events = state.store.events_after(session_id, None).await?;
    let last_event_id = events.last().map(|event| event.seq);
    let pending_events = pending_events(&events);
    let profile = mode_profile(&state.persona, &session.mode);
    Ok(Json(SessionMessagesResponse {
        id: session.id,
        status: session.status,
        mode: session.mode.clone(),
        mode_state: session.mode_state.clone(),
        persona_status,
        label: profile.label,
        voice_cap: profile.voice_cap,
        default_scope: session.memory_scope.clone(),
        owner_subject: session.owner_subject.clone(),
        memory_scope: session.memory_scope,
        active_skills: active_skills(&state.persona, &session.mode),
        messages,
        last_event_id,
        pending_events,
    }))
}

pub(crate) async fn end_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    _payload: Option<Json<EndSessionRequest>>,
) -> Result<Json<EndSessionResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let existing = state.store.get_session(session_id).await?;
    let result = state
        .store
        .end_session_and_enqueue_dream(session_id, existing.owner_subject, existing.memory_scope)
        .await?;
    for event in result.events {
        let _ = state.sender(session_id).send(event);
    }
    if let Some(admin) = &state.egress_admin {
        admin
            .clear_session(&session_id.to_string())
            .await
            .map_err(|_| {
                ServerError::Store("egress session cleanup failed (durability_failed)".into())
            })?;
    }
    Ok(Json(EndSessionResponse {
        id: result.session.id,
        status: result.session.status,
        dream: result.dream,
    }))
}

pub(crate) async fn set_session_scope<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Json(payload): Json<SetSessionScopeRequest>,
) -> Result<Json<SetSessionScopeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    validate_memory_scope(&state.linked_folders, &payload.scope)?;
    let existing = state.store.get_session(session_id).await?;
    if existing.status == "ended" {
        return Err(ServerError::Conflict(format!(
            "session {session_id} has ended"
        )));
    }
    let session = state
        .store
        .set_session_memory_scope(session_id, &payload.scope)
        .await?;
    Ok(Json(SetSessionScopeResponse {
        id: session.id,
        owner_subject: session.owner_subject,
        memory_scope: session.memory_scope,
    }))
}

fn session_response(
    persona: &ModesConfig,
    session: crate::SessionRecord,
    persona_status: crate::AssetStatus,
) -> CreateSessionResponse {
    let profile = mode_profile(persona, &session.mode);
    CreateSessionResponse {
        id: session.id,
        status: session.status,
        mode: session.mode.clone(),
        mode_state: session.mode_state.clone(),
        persona_status,
        label: profile.label,
        voice_cap: profile.voice_cap,
        default_scope: session.memory_scope.clone(),
        owner_subject: session.owner_subject.clone(),
        memory_scope: session.memory_scope,
        active_skills: profile.active_skills,
    }
}

fn validate_memory_scope(linked_folders: &tm_host::LinkedFolders, scope: &str) -> Result<()> {
    resources::util::validate_authorized_memory_scope(linked_folders, scope)
}

fn session_summary_response(
    persona: &ModesConfig,
    summary: crate::SessionSummaryRecord,
) -> SessionSummaryResponse {
    let title = compact_session_text(summary.summary.as_deref())
        .or_else(|| compact_session_text(summary.title.as_deref()))
        .unwrap_or_else(|| "New session".to_string());
    let preview = compact_session_text(summary.preview.as_deref()).unwrap_or_default();
    let profile = mode_profile(persona, &summary.session.mode);
    SessionSummaryResponse {
        id: summary.session.id,
        created_at: summary.session.created_at,
        updated_at: summary.session.updated_at,
        status: summary.session.status,
        mode: summary.session.mode,
        label: profile.label,
        voice_cap: profile.voice_cap,
        active_skills: profile.active_skills,
        title,
        preview,
        message_count: summary.message_count,
        last_event_id: summary.last_event_id,
    }
}

fn compact_session_text(value: Option<&str>) -> Option<String> {
    let text = value?.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    const MAX_CHARS: usize = 140;
    let mut compact = text.chars().take(MAX_CHARS).collect::<String>();
    if text.chars().count() > MAX_CHARS {
        compact.push_str("...");
    }
    Some(compact)
}

fn pending_events(events: &[SessionEvent]) -> Vec<SessionPendingEventResponse> {
    let mut approvals = BTreeMap::<String, SessionEvent>::new();
    let mut proposals = BTreeMap::<String, SessionEvent>::new();

    for event in events {
        match event.event_type.as_str() {
            "approval" => {
                if let Some(id) = event_id_value(&event.payload_json, "approvalId") {
                    approvals.insert(id, event.clone());
                }
            }
            "approval_resolved" => {
                if let Some(id) = event_id_value(&event.payload_json, "approvalId") {
                    approvals.remove(&id);
                }
            }
            "write_proposal" => {
                if let Some(id) = event_id_value(&event.payload_json, "proposalId") {
                    let status = event_id_value(&event.payload_json, "status")
                        .unwrap_or_else(|| "pending".to_string());
                    if status == "pending" {
                        proposals.insert(id, event.clone());
                    } else {
                        proposals.remove(&id);
                    }
                }
            }
            _ => {}
        }
    }

    let mut pending = approvals
        .into_values()
        .chain(proposals.into_values())
        .collect::<Vec<_>>();
    pending.sort_by_key(|event| event.seq);
    pending
        .into_iter()
        .map(|event| SessionPendingEventResponse {
            id: event.seq.to_string(),
            seq: event.seq,
            event_type: event.event_type,
            data: event.payload_json,
            created_at: event.created_at,
        })
        .collect()
}

fn event_id_value(payload: &Value, camel_key: &str) -> Option<String> {
    let snake_key = camel_to_snake(camel_key);
    payload
        .get(camel_key)
        .or_else(|| payload.get(&snake_key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn camel_to_snake(value: &str) -> String {
    let mut snake = String::new();
    for ch in value.chars() {
        if ch.is_ascii_uppercase() {
            snake.push('_');
            snake.push(ch.to_ascii_lowercase());
        } else {
            snake.push(ch);
        }
    }
    snake
}

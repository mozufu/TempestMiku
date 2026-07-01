use std::{
    collections::BTreeMap,
    convert::Infallible,
    fs,
    path::{Component, Path as FsPath, PathBuf},
    sync::Arc,
    time::Duration,
};

use chrono::Utc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Sse, sse::Event},
    routing::{get, post},
};
use futures::{StreamExt, stream::BoxStream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use tm_artifacts::{ResourceContent, preview};
use tm_core::DEFAULT_SYSTEM_PROMPT;
use tm_host::{
    CapabilityGrants, InvocationCtx, LinkedFolders, LinkedResourceHandler, ResourceEntry,
    ResourceRegistry,
};

use crate::{
    ApprovalBroker, AuthConfig, ChatRunner, ChatTurn, CodingBackend, CodingTurn, MemoryProvider,
    Mode, NewProjectItem, NewSession, PersistingEventSink, PersonaConfig, ProjectItemKind,
    ProjectItemRecord, ResolveApprovalRequest, Result, ServerError, SessionEvent, Store,
    StoreCodingEventSink, StoreEvent, store::ModeState, store::RecallChunkRecord,
};

pub struct AppState<S, M, C> {
    pub store: Arc<S>,
    pub memory: Arc<M>,
    pub chat: Arc<C>,
    pub persona: PersonaConfig,
    pub auth: AuthConfig,
    pub coding_backend: Option<Arc<dyn CodingBackend>>,
    pub approval_broker: Arc<ApprovalBroker>,
    pub artifact_root: PathBuf,
    pub linked_folders: LinkedFolders,
    live_events:
        Arc<parking_lot::Mutex<std::collections::BTreeMap<Uuid, broadcast::Sender<SessionEvent>>>>,
}

impl<S, M, C> Clone for AppState<S, M, C> {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
            memory: Arc::clone(&self.memory),
            chat: Arc::clone(&self.chat),
            persona: self.persona.clone(),
            auth: self.auth.clone(),
            coding_backend: self.coding_backend.clone(),
            approval_broker: Arc::clone(&self.approval_broker),
            artifact_root: self.artifact_root.clone(),
            linked_folders: self.linked_folders.clone(),
            live_events: Arc::clone(&self.live_events),
        }
    }
}

impl<S, M, C> AppState<S, M, C> {
    pub fn new(
        store: Arc<S>,
        memory: Arc<M>,
        chat: Arc<C>,
        persona: PersonaConfig,
        auth: AuthConfig,
    ) -> Self {
        Self {
            store,
            memory,
            chat,
            persona,
            auth,
            coding_backend: None,
            approval_broker: Arc::new(ApprovalBroker::default()),
            artifact_root: tm_artifacts::default_root(),
            linked_folders: LinkedFolders::default(),
            live_events: Arc::new(parking_lot::Mutex::new(std::collections::BTreeMap::new())),
        }
    }

    pub fn with_coding_backend(mut self, backend: Arc<dyn CodingBackend>) -> Self {
        self.coding_backend = Some(backend);
        self
    }

    pub fn with_artifact_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.artifact_root = root.into();
        self
    }

    pub fn with_linked_folders(mut self, linked_folders: LinkedFolders) -> Self {
        self.linked_folders = linked_folders;
        self
    }

    fn sender(&self, session_id: Uuid) -> broadcast::Sender<SessionEvent> {
        self.live_events
            .lock()
            .entry(session_id)
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }
}

pub fn app<S, M, C>(state: AppState<S, M, C>) -> Router
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    Router::<AppState<S, M, C>>::new()
        .route("/health", get(health))
        .route("/sessions", post(create_session::<S, M, C>))
        .route("/sessions/:id", get(get_session::<S, M, C>))
        .route("/sessions/:id/events", get(session_events::<S, M, C>))
        .route("/sessions/:id/messages", post(post_message::<S, M, C>))
        .route("/sessions/:id/mode", get(get_mode::<S, M, C>))
        .route("/sessions/:id/mode/suggest", post(suggest_mode::<S, M, C>))
        .route("/sessions/:id/mode/apply", post(apply_mode::<S, M, C>))
        .route("/sessions/:id/mode/lock", post(lock_mode::<S, M, C>))
        .route("/sessions/:id/mode/unlock", post(unlock_mode::<S, M, C>))
        .route(
            "/sessions/:id/mode/override",
            post(override_mode::<S, M, C>),
        )
        .route(
            "/sessions/:id/approvals/:approval_id",
            post(resolve_approval::<S, M, C>),
        )
        .route("/sessions/:id/project", get(project_overview::<S, M, C>))
        .route(
            "/sessions/:id/project/open-loops",
            get(project_open_loops::<S, M, C>),
        )
        .route(
            "/sessions/:id/project/decisions",
            get(project_decisions::<S, M, C>),
        )
        .route(
            "/sessions/:id/project/next-actions",
            get(project_next_actions::<S, M, C>),
        )
        .route("/sessions/:id/promote", post(promote_session::<S, M, C>))
        .route(
            "/sessions/:id/resources/resolve",
            get(resolve_resource::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/read",
            get(resolve_resource::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/preview",
            get(preview_resource::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/list",
            get(list_resources::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/artifacts",
            get(list_artifacts::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/artifacts/:artifact_id",
            get(read_artifact::<S, M, C>),
        )
        .fallback_service(crate::webui::service())
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

#[derive(Debug, Default, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub mode: Mode,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub id: Uuid,
    pub mode: Mode,
    pub mode_state: ModeState,
    pub persona_status: crate::PersonaStatus,
    pub label: String,
    pub voice_cap: String,
    pub default_scope: String,
    #[serde(rename = "activeSkills")]
    pub active_skills: Vec<String>,
}

async fn create_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    headers: HeaderMap,
    payload: Option<Json<CreateSessionRequest>>,
) -> Result<Json<CreateSessionResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let mode = payload
        .map(|Json(payload)| payload.mode)
        .unwrap_or_default();
    let persona_status = state.persona.load_status();
    let session = state
        .store
        .create_session(NewSession {
            mode,
            persona_status: persona_status.clone(),
        })
        .await?;
    let mode_event = state
        .store
        .append_event(
            session.id,
            "mode",
            mode_changed_payload(None, &session.mode_state, persona_status.clone())?,
        )
        .await?;
    let _ = state.sender(session.id).send(mode_event);
    Ok(Json(session_response(session, persona_status)))
}

async fn get_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<CreateSessionResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let persona_status = session.persona_status.clone();
    Ok(Json(session_response(session, persona_status)))
}

fn session_response(
    session: crate::SessionRecord,
    persona_status: crate::PersonaStatus,
) -> CreateSessionResponse {
    CreateSessionResponse {
        id: session.id,
        mode: session.mode,
        mode_state: session.mode_state.clone(),
        persona_status,
        label: session.mode.label().to_string(),
        voice_cap: session.mode.voice_cap().to_string(),
        default_scope: session.mode.default_scope().to_string(),
        active_skills: active_skills(session.mode),
    }
}

#[derive(Debug, Deserialize)]
pub struct PostMessageRequest {
    pub content: String,
    #[serde(default = "default_subject")]
    pub subject: String,
    #[serde(default)]
    pub scope: Option<String>,
}

fn default_subject() -> String {
    "brian".to_string()
}

async fn post_message<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<PostMessageRequest>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let mut session = state.store.get_session(session_id).await?;
    if let Some((suggested, reason)) = route_mode_for_prompt(&payload.content)
        && session.mode_state.lock_source.is_none()
        && suggested != session.mode_state.mode
    {
        let mut next = session.mode_state.clone();
        next.mode = suggested;
        next.router_reason = Some(reason);
        next.override_source = None;
        next.updated_at = Utc::now();
        let (updated, _) = commit_mode_state(&state, session.clone(), next).await?;
        session = updated;
    }
    let scope = payload
        .scope
        .clone()
        .unwrap_or_else(|| session.mode_state.mode.default_scope().to_string());
    let persona_prompt = build_turn_prompt(&state, session.mode_state.mode);
    state
        .store
        .append_message(session_id, "user", &payload.content)
        .await?;
    let memory = state
        .memory
        .context_for_turn(&payload.subject, &scope, &payload.content)
        .await?;
    let user_prompt = if memory.is_empty() {
        payload.content.clone()
    } else {
        format!(
            "{}\n\n[recall]\n{}",
            payload.content,
            memory.render_prompt_block()
        )
    };
    let response = if matches!(
        session.mode_state.mode,
        Mode::SeriousEngineer | Mode::Handoff
    ) {
        if let Some(backend) = &state.coding_backend {
            let sink = Arc::new(StoreCodingEventSink::new(
                session_id,
                Arc::clone(&state.store),
                state.sender(session_id),
            ));
            backend
                .run_turn(
                    CodingTurn {
                        session_id,
                        user_prompt,
                        system_prompt: persona_prompt.system_prompt.clone(),
                        mode: session.mode_state.mode,
                        scope: scope.clone(),
                    },
                    sink,
                )
                .await?
                .final_text
        } else {
            let sink = Arc::new(PersistingEventSink::new(
                session_id,
                Arc::clone(&state.store),
                state.sender(session_id),
            ));
            let response = state
                .chat
                .run_turn(
                    ChatTurn {
                        session_id,
                        user_prompt,
                        system_prompt: persona_prompt.system_prompt.clone(),
                        mode: session.mode_state.mode,
                        scope: scope.clone(),
                    },
                    sink.clone(),
                )
                .await?;
            sink.flush().await?;
            response
        }
    } else {
        let sink = Arc::new(PersistingEventSink::new(
            session_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        ));
        let response = state
            .chat
            .run_turn(
                ChatTurn {
                    session_id,
                    user_prompt,
                    system_prompt: persona_prompt.system_prompt,
                    mode: session.mode_state.mode,
                    scope: scope.clone(),
                },
                sink.clone(),
            )
            .await?;
        sink.flush().await?;
        response
    };

    state
        .store
        .append_message(session_id, "assistant", &response)
        .await?;
    if matches!(
        session.mode_state.mode,
        Mode::SeriousEngineer | Mode::Handoff
    ) && !response.trim().is_empty()
    {
        state
            .store
            .add_recall_chunk(RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: scope.clone(),
                text: format!(
                    "Project summary/open loop from session {session_id}: {}",
                    response.trim()
                ),
                source: format!("session:{session_id}:assistant"),
                created_at: Utc::now(),
            })
            .await?;
        record_project_observations(
            &state,
            session_id,
            project_id_from_scope(&scope),
            &payload.content,
            &response,
        )
        .await?;
    }
    Ok(Json(json!({ "status": "accepted" })))
}

#[derive(Debug, Deserialize)]
pub struct ModeRequest {
    #[serde(default)]
    pub mode: Option<Mode>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeResponse {
    pub mode_state: ModeState,
    pub label: String,
    pub voice_cap: String,
    pub default_scope: String,
    pub active_skills: Vec<String>,
    pub changed: bool,
    pub ignored_reason: Option<String>,
}

async fn get_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    Ok(Json(mode_response(session.mode_state, false, None)))
}

async fn suggest_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ModeRequest>,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    if session.mode_state.lock_source.is_some() {
        return Ok(Json(mode_response(
            session.mode_state,
            false,
            Some("mode is locked by user".to_string()),
        )));
    }
    let mode = payload.mode.unwrap_or(session.mode_state.mode);
    let mut next = session.mode_state.clone();
    next.mode = mode;
    next.router_reason = payload.reason;
    next.override_source = None;
    next.updated_at = Utc::now();
    let (session, changed) = commit_mode_state(&state, session, next).await?;
    Ok(Json(mode_response(session.mode_state, changed, None)))
}

async fn apply_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ModeRequest>,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    if session.mode_state.lock_source.is_some() {
        return Ok(Json(mode_response(
            session.mode_state,
            false,
            Some("mode is locked by user".to_string()),
        )));
    }
    let mode = payload
        .mode
        .ok_or_else(|| ServerError::InvalidRequest("mode is required".to_string()))?;
    let mut next = session.mode_state.clone();
    next.mode = mode;
    next.router_reason = payload.reason;
    next.override_source = payload.source.or_else(|| Some("api".to_string()));
    next.updated_at = Utc::now();
    let (session, changed) = commit_mode_state(&state, session, next).await?;
    Ok(Json(mode_response(session.mode_state, changed, None)))
}

async fn lock_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ModeRequest>,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let mut next = session.mode_state.clone();
    if let Some(mode) = payload.mode {
        next.mode = mode;
    }
    next.router_reason = payload.reason;
    next.lock_source = Some(payload.source.unwrap_or_else(|| "user".to_string()));
    next.override_source = None;
    next.updated_at = Utc::now();
    let (session, changed) = commit_mode_state(&state, session, next).await?;
    Ok(Json(mode_response(session.mode_state, changed, None)))
}

async fn unlock_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ModeRequest>,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let mut next = session.mode_state.clone();
    next.lock_source = None;
    next.override_source = None;
    next.router_reason = payload.reason;
    next.updated_at = Utc::now();
    let (session, changed) = commit_mode_state(&state, session, next).await?;
    Ok(Json(mode_response(session.mode_state, changed, None)))
}

async fn override_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ModeRequest>,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let mode = payload
        .mode
        .ok_or_else(|| ServerError::InvalidRequest("mode is required".to_string()))?;
    let mut next = session.mode_state.clone();
    next.mode = mode;
    next.router_reason = payload.reason;
    next.lock_source = None;
    next.override_source = Some(payload.source.unwrap_or_else(|| "user".to_string()));
    next.updated_at = Utc::now();
    let (session, changed) = commit_mode_state(&state, session, next).await?;
    Ok(Json(mode_response(session.mode_state, changed, None)))
}

fn mode_response(
    mode_state: ModeState,
    changed: bool,
    ignored_reason: Option<String>,
) -> ModeResponse {
    ModeResponse {
        label: mode_state.label().to_string(),
        voice_cap: mode_state.voice_cap().to_string(),
        default_scope: mode_state.mode.default_scope().to_string(),
        active_skills: active_skills(mode_state.mode),
        mode_state,
        changed,
        ignored_reason,
    }
}

async fn commit_mode_state<S, M, C>(
    state: &AppState<S, M, C>,
    session: crate::SessionRecord,
    next: ModeState,
) -> Result<(crate::SessionRecord, bool)>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let changed = session.mode_state != next;
    if !changed {
        return Ok((session, false));
    }
    let from = Some(session.mode_state.mode);
    let updated = state.store.set_mode_state(session.id, next).await?;
    let event = state
        .store
        .append_event(
            session.id,
            "mode",
            mode_changed_payload(from, &updated.mode_state, updated.persona_status.clone())?,
        )
        .await?;
    let _ = state.sender(session.id).send(event);
    Ok((updated, true))
}

fn mode_changed_payload(
    from: Option<Mode>,
    mode_state: &ModeState,
    persona_status: crate::PersonaStatus,
) -> Result<Value> {
    Ok(serde_json::to_value(StoreEvent::ModeChanged {
        from,
        mode: mode_state.mode,
        label: mode_state.label().to_string(),
        voice_cap: mode_state.voice_cap().to_string(),
        active_skills: active_skills(mode_state.mode),
        router_reason: mode_state.router_reason.clone(),
        lock_source: mode_state.lock_source.clone(),
        override_source: mode_state.override_source.clone(),
        updated_at: mode_state.updated_at,
        persona_status,
    })?)
}

fn active_skills(mode: Mode) -> Vec<String> {
    mode.active_skill_names()
        .iter()
        .map(|skill| (*skill).to_string())
        .collect()
}

fn build_turn_prompt<S, M, C>(state: &AppState<S, M, C>, mode: Mode) -> tm_persona::PersonaPrompt {
    state.persona.build_system_prompt(
        mode,
        DEFAULT_SYSTEM_PROMPT,
        &linked_folder_capability_notes(&state.linked_folders),
    )
}

fn linked_folder_capability_notes(linked_folders: &LinkedFolders) -> String {
    if linked_folders.is_empty() {
        return "No linked folders configured; fs.*, code.*, and proc.* will fail closed."
            .to_string();
    }

    let mut notes = String::new();
    for policy in linked_folders.policies() {
        let mode = match policy.mode {
            tm_host::FsMode::Ro => "ro",
            tm_host::FsMode::Rw => "rw",
        };
        notes.push_str(&format!(
            "Linked folders: {} ({mode}) at linked://{}/\n",
            policy.alias, policy.alias
        ));
    }
    notes
}

fn route_mode_for_prompt(content: &str) -> Option<(Mode, String)> {
    let lower = content.to_lowercase();
    if lower.contains("handoff") || lower.contains("delegate") {
        return Some((Mode::Handoff, "handoff/delegation prompt".to_string()));
    }
    if lower.contains("燒烤") || lower.contains("grill me") || lower.contains("ambiguous") {
        return Some((Mode::AmbiguityGrill, "ambiguity-grill trigger".to_string()));
    }
    if lower.contains("overwhelmed")
        || lower.contains("spiraling")
        || lower.contains("exhausted")
        || lower.contains("i can't")
    {
        return Some((
            Mode::NegativeStateGrounding,
            "negative-state grounding trigger".to_string(),
        ));
    }
    let engineering_markers = [
        "code",
        "rust",
        "cargo",
        "compile",
        "test",
        "bug",
        "fix",
        "repo",
        "patch",
        "crates/",
        "apps/",
        "src/",
        ".rs",
        "implement",
        "refactor",
        "migration",
        "production",
    ];
    engineering_markers
        .iter()
        .any(|marker| lower.contains(marker))
        .then(|| {
            (
                Mode::SeriousEngineer,
                "coding/production prompt requires Serious Engineer".to_string(),
            )
        })
}

async fn resolve_approval<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, approval_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<ResolveApprovalRequest>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    state.store.get_session(session_id).await?;
    state
        .approval_broker
        .resolve(session_id, approval_id, payload)?;
    Ok(Json(json!({ "status": "resolved" })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectQuery {
    project_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectOverview {
    project_id: String,
    project_uri: String,
    session_id: Uuid,
    status: String,
    open_loops: Vec<ProjectItemRecord>,
    decisions: Vec<ProjectItemRecord>,
    next_actions: Vec<ProjectItemRecord>,
    resources: Vec<ProjectItemRecord>,
    provenance: Value,
}

async fn project_overview<S, M, C>(
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

async fn project_open_loops<S, M, C>(
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

async fn project_decisions<S, M, C>(
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

async fn project_next_actions<S, M, C>(
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

async fn build_project_overview<S, M, C>(
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

async fn record_project_observations<S, M, C>(
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

fn project_id_from_scope(scope: &str) -> String {
    scope
        .strip_prefix("project:")
        .filter(|id| !id.is_empty())
        .unwrap_or("tempestmiku")
        .to_string()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromoteSessionRequest {
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
struct PromoteSessionResponse {
    project_uri: String,
    project_id: String,
    promoted: Vec<ProjectItemRecord>,
}

async fn promote_session<S, M, C>(
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

#[derive(Debug, Default, Deserialize)]
struct ArtifactReadQuery {
    selector: Option<String>,
}

async fn list_artifacts<S, M, C>(
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

async fn read_artifact<S, M, C>(
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
struct ResourceQuery {
    uri: Option<String>,
    selector: Option<String>,
}

async fn resolve_resource<S, M, C>(
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

async fn preview_resource<S, M, C>(
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
    let mut content = read_resource_content(&state, session_id, &uri, None).await?;
    content.content.clear();
    Ok(Json(content))
}

async fn list_resources<S, M, C>(
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
        Some(scheme) => Err(ServerError::Policy(format!(
            "unknown resource scheme {scheme}; registered: artifact, linked, workspace, project"
        ))),
        None => Err(ServerError::InvalidRequest(format!(
            "invalid resource uri {uri}"
        ))),
    }
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
        return Ok(["artifact", "linked", "workspace", "project"]
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
        Some(scheme) => Err(ServerError::Policy(format!(
            "unknown resource scheme {scheme}; registered: artifact, linked, workspace, project"
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

fn validate_relative_path(path: &str) -> Result<PathBuf> {
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
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsQuery {
    last_event_id: Option<i64>,
    last_event_id_legacy: Option<i64>,
}

async fn session_events<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
) -> Result<impl IntoResponse>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok());
    let last_event_id = last_event_id
        .or(query.last_event_id)
        .or(query.last_event_id_legacy);
    let receiver = state.sender(session_id).subscribe();
    let replay = state.store.events_after(session_id, last_event_id).await?;
    let replay_finished = replay.iter().any(|event| event.event_type == "final");
    let live = BroadcastStream::new(receiver).filter_map(|event| async move { event.ok() });
    let replay_stream = futures::stream::iter(replay);
    let event_stream: BoxStream<'static, SessionEvent> = if replay_finished {
        replay_stream.boxed()
    } else {
        replay_stream.chain(live).boxed()
    };
    let stream = event_stream.map(|event| Ok::<Event, Infallible>(sse_event(event)));
    Ok(Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15))))
}

fn sse_event(event: SessionEvent) -> Event {
    Event::default()
        .id(event.seq.to_string())
        .event(event.event_type)
        .json_data(event.payload_json)
        .expect("serializing stored JSON payload cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use chrono::Utc;
    use std::{collections::VecDeque, path::PathBuf, sync::Arc};

    use async_trait::async_trait;
    use futures::stream::{self, BoxStream};
    use parking_lot::Mutex;
    use tm_core::{
        AgentConfig, CellBudget, ChatRequest, Error as CoreError, LlmClient, Message,
        Result as CoreResult, Role, StreamEvent,
    };
    use tm_host::{FsMode, LinkedFolderConfig, LinkedFolders};
    use tm_sandbox::DenoSandboxOptions;
    use tower::ServiceExt;

    use crate::{
        AgentChatRunner, ApprovalOption, ApprovalPrompt, ChatRunner, ChatTurn, CodingEventSink,
        CodingTurn, CodingTurnResult, EchoChatRunner, InMemoryStore, NativeApprovalMode,
        NativeDenoBackend, StoreMemoryProvider,
        auth::ForwardedAuthConfig,
        store::{ProfileFactRecord, RecallChunkRecord},
    };
    use tm_persona::KNOWN_SKILLS;

    fn test_app(persona: PersonaConfig, auth: AuthConfig) -> (Router, Arc<InMemoryStore>) {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let state = AppState::new(store.clone(), memory, chat, persona, auth);
        (app(state), store)
    }

    fn test_app_with_state(
        state: AppState<InMemoryStore, StoreMemoryProvider<InMemoryStore>, EchoChatRunner>,
    ) -> (Router, Arc<InMemoryStore>) {
        let store = Arc::clone(&state.store);
        (app(state), store)
    }

    struct ScriptedBackend {
        kind: ScriptedBackendKind,
    }

    enum ScriptedBackendKind {
        Events,
        Approval { broker: Arc<ApprovalBroker> },
        Artifact { root: PathBuf },
    }

    struct ScriptedLlm {
        scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
        requests: Mutex<Vec<Vec<Message>>>,
    }

    impl ScriptedLlm {
        fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
            Self {
                scripts: Mutex::new(scripts.into()),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl LlmClient for ScriptedLlm {
        async fn chat_stream(
            &self,
            req: &ChatRequest,
        ) -> CoreResult<BoxStream<'static, CoreResult<StreamEvent>>> {
            self.requests.lock().push(req.messages.clone());
            let script = self.scripts.lock().pop_front().unwrap_or_default();
            Ok(Box::pin(stream::iter(
                script.into_iter().map(Ok::<StreamEvent, CoreError>),
            )))
        }
    }

    impl ScriptedBackend {
        fn events() -> Self {
            Self {
                kind: ScriptedBackendKind::Events,
            }
        }

        fn approval(broker: Arc<ApprovalBroker>) -> Self {
            Self {
                kind: ScriptedBackendKind::Approval { broker },
            }
        }

        fn artifact(root: PathBuf) -> Self {
            Self {
                kind: ScriptedBackendKind::Artifact { root },
            }
        }
    }

    #[async_trait]
    impl CodingBackend for ScriptedBackend {
        async fn run_turn(
            &self,
            turn: CodingTurn,
            sink: Arc<dyn CodingEventSink>,
        ) -> Result<CodingTurnResult> {
            match &self.kind {
                ScriptedBackendKind::Events => {
                    assert_eq!(turn.mode, Mode::SeriousEngineer);
                    assert_eq!(turn.scope, "project:tempestmiku");
                    sink.emit("text", json!({ "event": "text", "delta": "working" }))
                        .await?;
                    sink.emit(
                        "diff",
                        json!({
                            "backend": "test",
                            "path": "crates/tm-persona/src/lib.rs",
                            "oldText": null,
                            "newText": "patched",
                        }),
                    )
                    .await?;
                    let artifact = tm_artifacts::ArtifactRef {
                        uri: "artifact://0".to_string(),
                        id: "0".to_string(),
                        kind: "text".to_string(),
                        mime: "application/jsonl".to_string(),
                        title: Some("test transcript".to_string()),
                        size_bytes: 2,
                        preview: "{}".to_string(),
                    };
                    sink.emit(
                        "artifact",
                        json!({ "backend": "test", "artifact": artifact }),
                    )
                    .await?;
                    let final_text = "Completed via scripted backend with artifact://0".to_string();
                    sink.emit(
                        "final",
                        serde_json::to_value(StoreEvent::Final {
                            text: final_text.clone(),
                        })?,
                    )
                    .await?;
                    Ok(CodingTurnResult {
                        final_text,
                        transcript_artifact: None,
                    })
                }
                ScriptedBackendKind::Approval { broker } => {
                    let outcome = broker
                        .request_permission(
                            turn.session_id,
                            ApprovalPrompt {
                                action: "write file".to_string(),
                                scope: json!({ "path": "crates/tm-persona/src/lib.rs" }),
                                options: vec![
                                    ApprovalOption {
                                        option_id: "allow".to_string(),
                                        name: "Allow once".to_string(),
                                        kind: "allow_once".to_string(),
                                    },
                                    ApprovalOption {
                                        option_id: "reject".to_string(),
                                        name: "Reject once".to_string(),
                                        kind: "reject_once".to_string(),
                                    },
                                ],
                            },
                            Duration::from_secs(5),
                            sink.clone(),
                        )
                        .await?;
                    let final_text = format!("approval outcome: {outcome:?} artifact://approval");
                    sink.emit(
                        "final",
                        serde_json::to_value(StoreEvent::Final {
                            text: final_text.clone(),
                        })?,
                    )
                    .await?;
                    Ok(CodingTurnResult {
                        final_text,
                        transcript_artifact: None,
                    })
                }
                ScriptedBackendKind::Artifact { root } => {
                    let store =
                        tm_artifacts::ArtifactStore::open(root, turn.session_id.to_string())
                            .map_err(|err| ServerError::Store(err.to_string()))?;
                    let artifact = store
                        .put_text(
                            "{\"line\":\"scripted transcript\"}\n",
                            Some("scripted transcript".to_string()),
                            "application/jsonl",
                        )
                        .map_err(|err| ServerError::Store(err.to_string()))?;
                    sink.emit(
                        "artifact",
                        json!({ "backend": "test", "artifact": artifact }),
                    )
                    .await?;
                    let final_text = "Completed via scripted backend with artifact://0".to_string();
                    sink.emit(
                        "final",
                        serde_json::to_value(StoreEvent::Final {
                            text: final_text.clone(),
                        })?,
                    )
                    .await?;
                    Ok(CodingTurnResult {
                        final_text,
                        transcript_artifact: None,
                    })
                }
            }
        }
    }

    #[derive(Default)]
    struct RecordingChatRunner {
        turns: Arc<Mutex<Vec<ChatTurn>>>,
    }

    #[async_trait]
    impl ChatRunner for RecordingChatRunner {
        async fn run_turn(
            &self,
            turn: ChatTurn,
            sink: Arc<dyn tm_core::EventSink + Send + Sync>,
        ) -> Result<String> {
            self.turns.lock().push(turn);
            let text = "recorded chat turn".to_string();
            sink.on_text(&text);
            sink.on_final(&text);
            Ok(text)
        }
    }

    #[derive(Default)]
    struct RecordingBackend {
        turns: Arc<Mutex<Vec<CodingTurn>>>,
    }

    #[async_trait]
    impl CodingBackend for RecordingBackend {
        async fn run_turn(
            &self,
            turn: CodingTurn,
            sink: Arc<dyn CodingEventSink>,
        ) -> Result<CodingTurnResult> {
            self.turns.lock().push(turn);
            let final_text = "recorded coding turn".to_string();
            sink.emit(
                "final",
                serde_json::to_value(StoreEvent::Final {
                    text: final_text.clone(),
                })?,
            )
            .await?;
            Ok(CodingTurnResult {
                final_text,
                transcript_artifact: None,
            })
        }
    }

    async fn response_json(res: axum::response::Response) -> Value {
        let body = axum::body::to_bytes(res.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn create(app: &Router) -> CreateSessionResponse {
        create_with_body(app, Body::empty()).await
    }

    async fn create_with_body(app: &Router, body: Body) -> CreateSessionResponse {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn write_persona_fixture(root: &std::path::Path) {
        std::fs::write(
            root.join("SOUL.md"),
            "# Fixture SOUL\nIdentity constant and Mode Router fixture.",
        )
        .unwrap();
        for skill in KNOWN_SKILLS {
            let dir = root.join("skills").join(skill);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("# {skill}\n{skill} fixture body"),
            )
            .unwrap();
        }
    }

    #[tokio::test]
    async fn session_creation_message_append_event_append_and_replay_work() {
        let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
        let session = create(&app).await;
        assert_eq!(
            session.active_skills,
            vec!["miku-voice", "personal-assistant-state-capture"]
        );
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sessions/{}", session.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let reused = response_json(res).await;
        assert_eq!(reused["id"], session.id.to_string());
        assert_eq!(reused["mode"], json!("personal_assistant"));

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"hello"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let all = store.events_after(session.id, None).await.unwrap();
        assert_eq!(
            all.iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["mode", "text", "final"]
        );
        assert_eq!(
            all[0].payload_json["voice_cap"],
            serde_json::json!("medium")
        );
        assert_eq!(
            all[0].payload_json["activeSkills"],
            json!(["miku-voice", "personal-assistant-state-capture"])
        );
        let replay = store.events_after(session.id, Some(1)).await.unwrap();
        assert_eq!(
            replay
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["text", "final"]
        );
    }

    #[tokio::test]
    async fn missing_persona_path_boots_degraded_with_warning() {
        let (app, _) = test_app(
            PersonaConfig::from_path("/definitely/missing/tempestmiku/persona"),
            AuthConfig::NoAuth,
        );
        let session = create(&app).await;
        match session.persona_status {
            crate::PersonaStatus::Degraded { warning } => assert!(warning.contains("missing")),
            crate::PersonaStatus::Loaded { .. } => panic!("missing path must degrade"),
        }
    }

    #[tokio::test]
    async fn chat_turn_prompt_uses_active_persona_bundle() {
        let temp = tempfile::tempdir().unwrap();
        write_persona_fixture(temp.path());
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(RecordingChatRunner::default());
        let turns = Arc::clone(&chat.turns);
        let state = AppState::new(
            store,
            memory,
            chat,
            PersonaConfig::from_path(temp.path()),
            AuthConfig::NoAuth,
        );
        let app = app(state);
        let session = create(&app).await;

        for content in [
            "hello",
            "燒烤我 about this fuzzy plan",
            "i am overwhelmed and exhausted",
        ] {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/sessions/{}/messages", session.id))
                        .header("content-type", "application/json")
                        .body(Body::from(json!({ "content": content }).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
        }

        let turns = turns.lock();
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].mode, Mode::PersonalAssistant);
        assert!(turns[0].system_prompt.contains("Fixture SOUL"));
        assert!(turns[0].system_prompt.contains("skill://miku-voice"));
        assert!(
            turns[0]
                .system_prompt
                .contains("skill://personal-assistant-state-capture")
        );

        assert_eq!(turns[1].mode, Mode::AmbiguityGrill);
        assert!(turns[1].system_prompt.contains("skill://ambiguity-grill"));
        assert!(
            !turns[1]
                .system_prompt
                .contains("skill://negative-state-grounding")
        );

        assert_eq!(turns[2].mode, Mode::NegativeStateGrounding);
        assert!(
            turns[2]
                .system_prompt
                .contains("skill://negative-state-grounding")
        );
        assert_eq!(turns[2].scope, "global");
    }

    #[tokio::test]
    async fn coding_turn_prompt_uses_active_persona_bundle() {
        let temp = tempfile::tempdir().unwrap();
        write_persona_fixture(temp.path());
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let backend = Arc::new(RecordingBackend::default());
        let turns = Arc::clone(&backend.turns);
        let state = AppState::new(
            store,
            memory,
            chat,
            PersonaConfig::from_path(temp.path()),
            AuthConfig::NoAuth,
        )
        .with_coding_backend(backend);
        let app = app(state);
        let serious = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
        let handoff = create_with_body(&app, Body::from(r#"{"mode":"handoff"}"#)).await;

        for (session_id, content) in [
            (serious.id, "fix the Rust code"),
            (handoff.id, "delegate this implementation handoff"),
        ] {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/sessions/{session_id}/messages"))
                        .header("content-type", "application/json")
                        .body(Body::from(json!({ "content": content }).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
        }

        let turns = turns.lock();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].mode, Mode::SeriousEngineer);
        assert!(turns[0].system_prompt.contains("Fixture SOUL"));
        assert!(
            turns[0]
                .system_prompt
                .contains("Active mode: Serious Engineer")
        );
        assert!(!turns[0].system_prompt.contains("skill://miku-voice"));

        assert_eq!(turns[1].mode, Mode::Handoff);
        assert!(turns[1].system_prompt.contains("skill://oh-my-pi-handoff"));
        assert_eq!(turns[1].scope, "project:tempestmiku");
    }

    #[tokio::test]
    async fn memory_context_injects_profile_facts_and_recall_chunks() {
        let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
        store
            .add_profile_fact(ProfileFactRecord {
                id: Uuid::new_v4(),
                subject: "brian".to_string(),
                predicate: "prefers".to_string(),
                object: "boring Rust".to_string(),
                confidence: 0.9,
                provenance: "test".to_string(),
                valid_from: Utc::now(),
                valid_to: None,
            })
            .await
            .unwrap();
        store
            .add_recall_chunk(RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: "global".to_string(),
                text: "hello project open loop".to_string(),
                source: "test".to_string(),
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        let session = create(&app).await;
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"hello"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let events = store.events_after(session.id, None).await.unwrap();
        let final_event = events
            .iter()
            .find(|event| event.event_type == "final")
            .unwrap();
        assert!(final_event.payload_json.to_string().contains("boring Rust"));
        assert!(
            final_event
                .payload_json
                .to_string()
                .contains("hello project open loop")
        );
    }

    #[tokio::test]
    async fn serious_engineer_session_uses_project_scope_and_recalls_next_session() {
        let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
        let session_a = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
        assert_eq!(session_a.voice_cap, "off");
        assert_eq!(session_a.default_scope, "project:tempestmiku");
        assert!(session_a.active_skills.is_empty());
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session_a.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"tempestmiku open loop"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let chunks = store
            .recall_chunks("project:tempestmiku", "tempestmiku", 5)
            .await
            .unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(
            chunks[0]
                .text
                .contains("Project summary/open loop from session")
        );
        let session_b = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session_b.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"tempestmiku open loop"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let events = store.events_after(session_b.id, None).await.unwrap();
        let final_event = events
            .iter()
            .find(|event| event.event_type == "final")
            .unwrap();
        assert!(
            final_event
                .payload_json
                .to_string()
                .contains("Project summary/open loop from session")
        );
    }

    #[tokio::test]
    async fn serious_engineer_uses_coding_backend_and_replays_events() {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let state = AppState::new(
            store.clone(),
            memory,
            chat,
            PersonaConfig::default(),
            AuthConfig::NoAuth,
        )
        .with_coding_backend(Arc::new(ScriptedBackend::events()));
        let (app, store) = test_app_with_state(state);
        let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"ship p0a"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let events = store.events_after(session.id, None).await.unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["mode", "text", "diff", "artifact", "final"]
        );
        let replay = store.events_after(session.id, Some(1)).await.unwrap();
        assert_eq!(
            replay
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["text", "diff", "artifact", "final"]
        );
        let final_event = events
            .iter()
            .find(|event| event.event_type == "final")
            .unwrap();
        let final_text = final_event.payload_json.to_string();
        assert!(final_text.contains("artifact://"));
        assert!(!final_text.contains("喵"));
    }

    #[tokio::test]
    async fn approval_route_resolves_pending_backend_permission() {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let mut state = AppState::new(
            store.clone(),
            memory,
            chat,
            PersonaConfig::default(),
            AuthConfig::NoAuth,
        );
        let broker = Arc::clone(&state.approval_broker);
        state = state.with_coding_backend(Arc::new(ScriptedBackend::approval(Arc::clone(&broker))));
        let (app, store) = test_app_with_state(state);
        let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

        let post_app = app.clone();
        let message = tokio::spawn(async move {
            post_app
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/sessions/{}/messages", session.id))
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"content":"needs approval"}"#))
                        .unwrap(),
                )
                .await
                .unwrap()
        });

        let approval_id =
            wait_for_event_payload(&store, session.id, "approval").await["approvalId"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap();
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/sessions/{}/approvals/{}",
                        session.id, approval_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"decision":"approve"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(message.await.unwrap().status(), StatusCode::OK);

        let resolved = store
            .events_after(session.id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|event| event.event_type == "approval_resolved")
            .unwrap();
        assert_eq!(resolved.payload_json["outcome"], json!("selected"));
        assert_eq!(resolved.payload_json["optionId"], json!("allow"));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn native_deno_backend_approval_route_approves_proc_run() {
        let (app, store, llm, session, _temp) =
            native_deno_approval_app(Duration::from_secs(5), native_proc_script()).await;
        let post_app = app.clone();
        let session_id = session.id;
        let message = tokio::spawn(async move {
            post_app
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/sessions/{session_id}/messages"))
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"content":"run an unsafe proc command"}"#))
                        .unwrap(),
                )
                .await
                .unwrap()
        });

        let approval = wait_for_event_payload(&store, session_id, "approval").await;
        assert_eq!(approval["backend"], json!("native-deno"));
        assert_eq!(approval["action"], json!("proc.run cargo clean"));
        let approval_id = approval["approvalId"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap();
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{session_id}/approvals/{approval_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"decision":"approve"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(message.await.unwrap().status(), StatusCode::OK);

        let resolved = store
            .events_after(session_id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|event| event.event_type == "approval_resolved")
            .unwrap();
        assert_eq!(resolved.payload_json["backend"], json!("native-deno"));
        assert_eq!(resolved.payload_json["optionId"], json!("allow"));
        assert!(native_tool_result(&llm).contains("\"ok\": true"));
        assert!(
            store
                .events_after(session_id, None)
                .await
                .unwrap()
                .iter()
                .any(|event| event.event_type == "cell_result")
        );
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn native_deno_backend_approval_route_denies_proc_run() {
        let (app, store, llm, session, _temp) =
            native_deno_approval_app(Duration::from_secs(5), native_proc_script()).await;
        let post_app = app.clone();
        let session_id = session.id;
        let message = tokio::spawn(async move {
            post_app
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/sessions/{session_id}/messages"))
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"content":"deny an unsafe proc command"}"#))
                        .unwrap(),
                )
                .await
                .unwrap()
        });

        let approval_id =
            wait_for_event_payload(&store, session_id, "approval").await["approvalId"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap();
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{session_id}/approvals/{approval_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"decision":"deny"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(message.await.unwrap().status(), StatusCode::OK);
        assert!(native_tool_result(&llm).contains("ApprovalDeniedError"));
        let resolved = store
            .events_after(session_id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|event| event.event_type == "approval_resolved")
            .unwrap();
        assert_eq!(resolved.payload_json["backend"], json!("native-deno"));
        assert_eq!(resolved.payload_json["optionId"], json!("reject"));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn native_deno_backend_approval_timeout_defaults_to_deny() {
        let (app, store, llm, session, _temp) =
            native_deno_approval_app(Duration::from_millis(1), native_proc_script()).await;
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"content":"timeout an unsafe proc command"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(native_tool_result(&llm).contains("ApprovalTimeoutError"));
        let events = store.events_after(session.id, None).await.unwrap();
        assert!(events.iter().any(|event| {
            event.event_type == "approval" && event.payload_json["backend"] == json!("native-deno")
        }));
        assert!(events.iter().any(|event| {
            event.event_type == "approval_resolved"
                && event.payload_json["backend"] == json!("native-deno")
                && event.payload_json["optionId"] == json!("reject")
        }));
    }

    #[tokio::test]
    async fn artifact_resource_route_reads_session_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let state = AppState::new(
            store,
            memory,
            chat,
            PersonaConfig::default(),
            AuthConfig::NoAuth,
        )
        .with_artifact_root(root.clone())
        .with_coding_backend(Arc::new(ScriptedBackend::artifact(root)));
        let (app, _) = test_app_with_state(state);
        let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"write transcript"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sessions/{}/resources/artifacts", session.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_json = response_json(list).await;
        assert_eq!(list_json[0]["uri"], json!("artifact://0"));

        let read = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sessions/{}/resources/artifacts/0", session.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);
        let read_json = response_json(read).await;
        assert_eq!(read_json["uri"], json!("artifact://0"));
        assert!(
            read_json["content"]
                .as_str()
                .unwrap()
                .contains("scripted transcript")
        );
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn server_agent_route_uses_deno_sdk_and_preserves_denials() {
        let temp = tempfile::tempdir().unwrap();
        let artifact_root = temp.path().join("artifacts");
        let linked_root = temp.path().join("repo");
        std::fs::create_dir_all(&linked_root).unwrap();
        std::fs::write(
            linked_root.join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )
        .unwrap();
        let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
            name: "repo".to_string(),
            path: linked_root,
            mode: FsMode::Rw,
            commands: vec!["cargo".to_string()],
            safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
        }])
        .unwrap();

        let code = r#"
const doc = await fs.read("linked://repo/Cargo.toml");
const artifact = artifacts.put(`server sdk artifact\n${doc.content}`, {
  title: "server-sdk",
  mime: "text/plain"
});
const unsafeProc = await proc.run("cargo", ["clean"], { cwd: "repo:" })
  .catch(err => ({ name: err.name, retryable: err.retryable }));
const deniedHttp = await http.get("https://evil.test/")
  .catch(err => ({ name: err.name, capability: err.capability }));
display({
  ok: doc.content.includes("[workspace]"),
  artifact: artifact.uri,
  unsafeProc,
  deniedHttp
});
"#;
        let tool_args = json!({ "code": code }).to_string();
        let llm = Arc::new(ScriptedLlm::new(vec![
            vec![
                StreamEvent::ToolCall {
                    index: 0,
                    id: Some("call_server_sdk".to_string()),
                    name: Some("execute".to_string()),
                    arguments: Some(tool_args),
                },
                StreamEvent::Finish {
                    reason: Some("tool_calls".to_string()),
                },
            ],
            vec![
                StreamEvent::Text("done".to_string()),
                StreamEvent::Finish {
                    reason: Some("stop".to_string()),
                },
            ],
        ]));
        let cfg = AgentConfig {
            model: "fake".to_string(),
            max_turns: 3,
            cell_budget: CellBudget {
                wall_ms: 240_000,
                output_bytes: 50_000,
            },
            ..AgentConfig::default()
        };
        let chat = Arc::new(AgentChatRunner::deno(
            llm.clone(),
            cfg,
            DenoSandboxOptions {
                artifact_root: artifact_root.clone(),
                linked_folders: Some(linked.clone()),
                approval_timeout: Duration::from_millis(1),
                ..DenoSandboxOptions::default()
            },
        ));
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let state = AppState::new(
            store.clone(),
            memory,
            chat,
            PersonaConfig::default(),
            AuthConfig::NoAuth,
        )
        .with_artifact_root(artifact_root.clone())
        .with_linked_folders(linked);
        let router = app(state);
        let session = create(&router).await;

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"exercise the server sdk"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let requests = llm.requests.lock();
        assert_eq!(requests.len(), 2);
        let tool_result = requests[1]
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("tool result is fed back before final turn");
        assert!(
            tool_result.content.contains("\"ok\": true"),
            "tool result: {}",
            tool_result.content
        );
        assert!(
            tool_result.content.contains("artifact://0"),
            "tool result: {}",
            tool_result.content
        );
        assert!(
            tool_result.content.contains("ApprovalTimeoutError"),
            "tool result: {}",
            tool_result.content
        );
        assert!(
            tool_result.content.contains("CapabilityDeniedError"),
            "tool result: {}",
            tool_result.content
        );
        drop(requests);

        let events = store.events_after(session.id, None).await.unwrap();
        assert!(events.iter().any(|event| event.event_type == "cell_start"));
        assert!(events.iter().any(|event| event.event_type == "cell_result"));
        assert!(
            events.iter().any(|event| event.event_type == "final"
                && event.payload_json.to_string().contains("done"))
        );

        let list = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sessions/{}/resources/artifacts", session.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_json = response_json(list).await;
        assert_eq!(list_json[0]["uri"], json!("artifact://0"));

        let read = router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sessions/{}/resources/artifacts/0", session.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);
        let read_json = response_json(read).await;
        assert!(
            read_json["content"]
                .as_str()
                .unwrap()
                .contains("server sdk artifact")
        );
    }

    #[tokio::test]
    async fn router_lock_unlock_and_replay_mode_events() {
        let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
        let session = create(&app).await;

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"please fix this Rust code bug"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let events = store.events_after(session.id, None).await.unwrap();
        let mode_events = events
            .iter()
            .filter(|event| event.event_type == "mode")
            .collect::<Vec<_>>();
        assert_eq!(mode_events.len(), 2);
        assert_eq!(mode_events[1].payload_json["event"], json!("mode_changed"));
        assert_eq!(
            mode_events[1].payload_json["mode"],
            json!("serious_engineer")
        );
        assert_eq!(mode_events[1].payload_json["voice_cap"], json!("off"));
        assert_eq!(mode_events[1].payload_json["activeSkills"], json!([]));
        assert!(
            mode_events[1].payload_json["router_reason"]
                .as_str()
                .unwrap()
                .contains("coding")
        );

        let replay = store.events_after(session.id, Some(1)).await.unwrap();
        assert_eq!(replay[0].event_type, "mode");
        assert_eq!(replay[0].payload_json["mode"], json!("serious_engineer"));

        let lock = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/mode/lock", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"mode":"personal_assistant","reason":"stay light"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(lock.status(), StatusCode::OK);
        let lock_json = response_json(lock).await;
        assert_eq!(lock_json["modeState"]["lockSource"], json!("user"));
        assert_eq!(lock_json["voiceCap"], json!("medium"));
        assert_eq!(
            lock_json["activeSkills"],
            json!(["miku-voice", "personal-assistant-state-capture"])
        );

        let locked_message = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"fix another code bug"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(locked_message.status(), StatusCode::OK);
        let latest = store.get_session(session.id).await.unwrap();
        assert_eq!(latest.mode_state.mode, Mode::PersonalAssistant);

        let unlock = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/mode/unlock", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"reason":"router may resume"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unlock.status(), StatusCode::OK);

        let reroute = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"fix the Rust code now"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reroute.status(), StatusCode::OK);
        let latest = store.get_session(session.id).await.unwrap();
        assert_eq!(latest.mode_state.mode, Mode::SeriousEngineer);
        assert_eq!(latest.mode_state.mode.voice_cap(), "off");
    }

    #[tokio::test]
    async fn project_views_and_promotion_are_idempotent() {
        let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
        let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"content":"capture an open loop and decision for the code TODO"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let overview = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sessions/{}/project", session.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(overview.status(), StatusCode::OK);
        let overview_json = response_json(overview).await;
        assert_eq!(overview_json["projectUri"], json!("project://tempestmiku"));
        assert!(!overview_json["nextActions"].as_array().unwrap().is_empty());
        assert!(!overview_json["openLoops"].as_array().unwrap().is_empty());
        assert!(!overview_json["decisions"].as_array().unwrap().is_empty());

        let body = Body::from(
            r#"{"summary":"ship P1 slice","openLoops":["wire mobile resume"],"decisions":["keep SSE"],"resources":["artifact://0","workspace://session/notes.md"]}"#,
        );
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/promote", session.id))
                    .header("content-type", "application/json")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_json = response_json(first).await;
        assert_eq!(first_json["projectUri"], json!("project://tempestmiku"));
        let first_promoted = first_json["promoted"].as_array().unwrap().clone();
        assert_eq!(first_promoted.len(), 5);
        assert_eq!(first_promoted[0]["provenanceJson"]["actor"], json!("user"));

        let second = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/promote", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"summary":"ship P1 slice","openLoops":["wire mobile resume"],"decisions":["keep SSE"],"resources":["artifact://0","workspace://session/notes.md"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second_json = response_json(second).await;
        assert_eq!(
            first_promoted[0]["id"],
            second_json["promoted"].as_array().unwrap()[0]["id"]
        );
        let project_resources = store
            .project_items("tempestmiku", Some(ProjectItemKind::Artifact))
            .await
            .unwrap();
        assert_eq!(project_resources.len(), 1);
    }

    #[tokio::test]
    async fn resource_gateway_reads_supported_schemes_and_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let artifact_root = temp.path().join("artifacts");
        let linked_root = temp.path().join("linked");
        std::fs::create_dir_all(&linked_root).unwrap();
        std::fs::write(linked_root.join("README.md"), "linked readme").unwrap();
        let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
            name: "tempestmiku".to_string(),
            path: linked_root,
            mode: FsMode::Ro,
            commands: Vec::new(),
            safe_args: Vec::new(),
        }])
        .unwrap();
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let state = AppState::new(
            store,
            memory,
            chat,
            PersonaConfig::default(),
            AuthConfig::NoAuth,
        )
        .with_artifact_root(artifact_root.clone())
        .with_linked_folders(linked);
        let (app, store) = test_app_with_state(state);
        let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
        let artifact_store =
            tm_artifacts::ArtifactStore::open(&artifact_root, session.id.to_string()).unwrap();
        artifact_store
            .put_text("artifact line", Some("artifact".to_string()), "text/plain")
            .unwrap();
        let workspace = artifact_root
            .join("sessions")
            .join(session.id.to_string())
            .join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("notes.md"), "one\ntwo").unwrap();
        store
            .upsert_project_item(NewProjectItem {
                project_id: "tempestmiku".to_string(),
                kind: ProjectItemKind::Artifact,
                text: "artifact://0".to_string(),
                target_uri: "project://tempestmiku/artifacts/0".to_string(),
                source_session_id: session.id,
                source_event_seq: None,
                source_uri: Some("artifact://0".to_string()),
                dedupe_key: "test-artifact".to_string(),
                provenance_json: json!({"sourceSession": session.id}),
            })
            .await
            .unwrap();

        for (uri, expected) in [
            ("artifact://0", "artifact line"),
            ("workspace://session/notes.md", "two"),
            ("linked://tempestmiku/README.md", "linked readme"),
            ("project://tempestmiku/resources", "artifact://0"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(format!(
                            "/sessions/{}/resources/resolve?uri={}",
                            session.id, uri
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let json = response_json(response).await;
            assert!(
                json["content"].as_str().unwrap().contains(expected),
                "content for {uri}: {json}"
            );
        }

        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/sessions/{}/resources/resolve?uri=workspace://session/../secret",
                        session.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);

        let unknown = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/sessions/{}/resources/resolve?uri=drive://later",
                        session.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn miku_initiated_promotion_denies_by_default_on_timeout() {
        let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
        let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/promote", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"summary":"proposed write","initiatedBy":"miku","timeoutMs":1}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let events = store.events_after(session.id, None).await.unwrap();
        assert!(events.iter().any(|event| event.event_type == "approval"));
        let resolved = events
            .iter()
            .find(|event| event.event_type == "approval_resolved")
            .unwrap();
        assert_eq!(resolved.payload_json["backend"], json!("project-promotion"));
        assert_eq!(resolved.payload_json["optionId"], json!("reject"));
    }

    async fn native_deno_approval_app(
        timeout: Duration,
        code: &str,
    ) -> (
        Router,
        Arc<InMemoryStore>,
        Arc<ScriptedLlm>,
        CreateSessionResponse,
        tempfile::TempDir,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let artifact_root = temp.path().join("artifacts");
        let linked_root = temp.path().join("repo");
        std::fs::create_dir_all(linked_root.join("src")).unwrap();
        std::fs::write(
            linked_root.join("Cargo.toml"),
            "[package]\nname = \"native-deno-approval-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(linked_root.join("src/lib.rs"), "pub fn x() -> i32 { 1 }\n").unwrap();
        let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
            name: "repo".to_string(),
            path: linked_root,
            mode: FsMode::Rw,
            commands: vec!["cargo".to_string()],
            safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
        }])
        .unwrap();
        let tool_args = json!({ "code": code }).to_string();
        let llm = Arc::new(ScriptedLlm::new(vec![
            vec![
                StreamEvent::ToolCall {
                    index: 0,
                    id: Some("call_native_deno".to_string()),
                    name: Some("execute".to_string()),
                    arguments: Some(tool_args),
                },
                StreamEvent::Finish {
                    reason: Some("tool_calls".to_string()),
                },
            ],
            vec![
                StreamEvent::Text("done".to_string()),
                StreamEvent::Finish {
                    reason: Some("stop".to_string()),
                },
            ],
        ]));
        let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
        let cfg = AgentConfig {
            model: "fake".to_string(),
            max_turns: 3,
            cell_budget: CellBudget {
                wall_ms: 240_000,
                output_bytes: 50_000,
            },
            ..AgentConfig::default()
        };
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let mut state = AppState::new(
            store.clone(),
            memory,
            chat,
            PersonaConfig::default(),
            AuthConfig::NoAuth,
        )
        .with_artifact_root(artifact_root.clone())
        .with_linked_folders(linked.clone());
        let backend = NativeDenoBackend::new(
            llm_for_backend,
            cfg,
            DenoSandboxOptions {
                artifact_root,
                linked_folders: Some(linked),
                approval_timeout: timeout,
                ..DenoSandboxOptions::default()
            },
            NativeApprovalMode::Manual,
            Arc::clone(&state.approval_broker),
        );
        state = state.with_coding_backend(Arc::new(backend));
        let router = app(state);
        let session = create_with_body(&router, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
        (router, store, llm, session, temp)
    }

    fn native_proc_script() -> &'static str {
        r#"
const result = await proc.run("cargo", ["clean"], { cwd: "repo:" })
  .then(ok => ({ ok: true, exitCode: ok.exitCode }))
  .catch(err => ({ ok: false, name: err.name, retryable: err.retryable }));
display(result);
"#
    }

    fn native_tool_result(llm: &ScriptedLlm) -> String {
        let requests = llm.requests.lock();
        requests[1]
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("tool result is fed back before final turn")
            .content
            .clone()
    }

    async fn wait_for_event_payload(
        store: &InMemoryStore,
        session_id: Uuid,
        event_type: &str,
    ) -> Value {
        for _ in 0..100 {
            if let Some(event) = store
                .events_after(session_id, None)
                .await
                .unwrap()
                .into_iter()
                .find(|event| event.event_type == event_type)
            {
                return event.payload_json;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("event {event_type} was not persisted")
    }

    #[tokio::test]
    async fn bearer_and_forwarded_auth_are_enforced() {
        let (app, _) = test_app(
            PersonaConfig::default(),
            AuthConfig::BearerToken("secret".to_string()),
        );
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let allowed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sessions")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);

        let (forwarded, _) = test_app(
            PersonaConfig::default(),
            AuthConfig::Forwarded(ForwardedAuthConfig {
                user_header: "x-forwarded-user".to_string(),
                expected_user: Some("brian".to_string()),
            }),
        );
        let wrong = forwarded
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sessions")
                    .header("x-forwarded-user", "not-brian")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
        let ok = forwarded
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sessions")
                    .header("x-forwarded-user", "brian")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
    }
}

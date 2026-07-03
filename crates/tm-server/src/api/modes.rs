use super::*;

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

pub(super) async fn get_mode<S, M, C>(
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

pub(super) async fn suggest_mode<S, M, C>(
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

pub(super) async fn apply_mode<S, M, C>(
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

pub(super) async fn lock_mode<S, M, C>(
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

pub(super) async fn unlock_mode<S, M, C>(
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

pub(super) async fn override_mode<S, M, C>(
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

pub(super) async fn commit_mode_state<S, M, C>(
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

pub(super) fn mode_changed_payload(
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

pub(super) fn active_skills(mode: Mode) -> Vec<String> {
    mode.active_skill_names()
        .iter()
        .map(|skill| (*skill).to_string())
        .collect()
}

pub(super) fn build_turn_prompt<S, M, C>(
    state: &AppState<S, M, C>,
    mode: Mode,
) -> tm_persona::PersonaPrompt {
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

pub(super) fn route_mode_for_prompt(content: &str) -> (Mode, String) {
    let lower = content.to_lowercase();
    if lower.contains("handoff") || lower.contains("delegate") {
        return (Mode::Handoff, "handoff/delegation prompt".to_string());
    }
    if lower.contains("燒烤") || lower.contains("grill me") || lower.contains("ambiguous") {
        return (Mode::AmbiguityGrill, "ambiguity-grill trigger".to_string());
    }
    if let Some(trigger) = negative_state_grounding_trigger(&lower) {
        return (
            Mode::NegativeStateGrounding,
            format!("negative-state grounding trigger: {trigger}"),
        );
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
        "proc",
        "crates/",
        "apps/",
        "src/",
        ".rs",
        "implement",
        "refactor",
        "migration",
        "production",
    ];
    if engineering_markers
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return (
            Mode::SeriousEngineer,
            "coding/production prompt requires Serious Engineer".to_string(),
        );
    }
    (
        Mode::PersonalAssistant,
        "default personal-assistant route for non-coding prompt".to_string(),
    )
}

const NEGATIVE_STATE_GROUNDING_TRIGGERS: &[(&str, &[&str])] = &[
    (
        "overwhelmed language",
        &[
            "overwhelmed",
            "overload",
            "overloaded",
            "too much",
            "drowning",
            "can't handle",
            "cant handle",
        ],
    ),
    (
        "exhausted language",
        &[
            "exhausted",
            "burned out",
            "burnt out",
            "drained",
            "fried",
            "wiped out",
            "no energy",
            "too tired",
            "i'm tired",
            "i am tired",
            "can't keep going",
            "cant keep going",
        ],
    ),
    (
        "self-deprecating language",
        &[
            "self-deprecating",
            "self deprecating",
            "useless",
            "worthless",
            "i'm a failure",
            "i am a failure",
            "i'm failing",
            "i am failing",
            "i suck",
            "i'm stupid",
            "i am stupid",
            "nothing i do counts",
            "no output",
        ],
    ),
    (
        "spiraling language",
        &[
            "spiral",
            "spiraling",
            "spiralling",
            "spinning out",
            "doom loop",
            "can't stop thinking",
            "cant stop thinking",
        ],
    ),
    (
        "stuck language",
        &[
            "stuck",
            "blocked",
            "can't start",
            "cant start",
            "can't move",
            "cant move",
            "can't make progress",
            "cant make progress",
            "i can't",
            "i cant",
            "i cannot",
        ],
    ),
];

fn negative_state_grounding_trigger(lower: &str) -> Option<&'static str> {
    NEGATIVE_STATE_GROUNDING_TRIGGERS
        .iter()
        .find(|(_, markers)| markers.iter().any(|marker| lower.contains(marker)))
        .map(|(trigger, _)| *trigger)
}

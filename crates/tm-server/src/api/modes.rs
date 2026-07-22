use super::*;

#[derive(Debug, Deserialize)]
pub struct ModeRequest {
    #[serde(default)]
    pub mode: Option<ModeId>,
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
    pub capabilities: Vec<String>,
    pub active_skills: Vec<String>,
    pub profile: ModeProfile,
    pub changed: bool,
    pub ignored_reason: Option<String>,
}

pub(super) async fn list_modes<S, M, C>(
    State(state): State<AppState<S, M, C>>,
) -> Result<Json<ModeCatalog>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    Ok(Json(state.persona.load_assets().modes))
}

pub(super) async fn get_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    Ok(Json(mode_response(
        &state.persona,
        session.mode_state,
        false,
        None,
    )))
}

pub(super) async fn suggest_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Json(payload): Json<ModeRequest>,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    if let Some(mode) = payload.mode {
        validate_mode(&state.persona, mode)?;
    }
    if session.mode_state.lock_source.is_some() {
        return Ok(Json(mode_response(
            &state.persona,
            session.mode_state,
            false,
            Some("mode is locked by user".to_string()),
        )));
    }
    Ok(Json(mode_response(
        &state.persona,
        session.mode_state,
        false,
        Some(
            "mode suggestions are approval-backed through the modes.suggest SDK capability; use apply or override for an explicit user switch"
                .to_string(),
        ),
    )))
}

pub(super) async fn apply_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Json(payload): Json<ModeRequest>,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    if session.mode_state.lock_source.is_some() {
        return Ok(Json(mode_response(
            &state.persona,
            session.mode_state,
            false,
            Some("mode is locked by user".to_string()),
        )));
    }
    let mode = payload
        .mode
        .ok_or_else(|| ServerError::InvalidRequest("mode is required".to_string()))
        .and_then(|mode| validate_mode(&state.persona, mode))?;
    let mut next = session.mode_state.clone();
    next.mode = mode;
    next.router_reason = payload.reason;
    next.override_source = payload.source.or_else(|| Some("api".to_string()));
    next.updated_at = Utc::now();
    let (session, changed) = commit_mode_state(&state, session, next).await?;
    Ok(Json(mode_response(
        &state.persona,
        session.mode_state,
        changed,
        None,
    )))
}

pub(super) async fn lock_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Json(payload): Json<ModeRequest>,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    let mut next = session.mode_state.clone();
    if let Some(mode) = payload.mode {
        next.mode = validate_mode(&state.persona, mode)?;
    }
    next.router_reason = payload.reason;
    next.lock_source = Some(payload.source.unwrap_or_else(|| "user".to_string()));
    next.override_source = None;
    next.updated_at = Utc::now();
    let (session, changed) = commit_mode_state(&state, session, next).await?;
    Ok(Json(mode_response(
        &state.persona,
        session.mode_state,
        changed,
        None,
    )))
}

pub(super) async fn unlock_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Json(payload): Json<ModeRequest>,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    let mut next = session.mode_state.clone();
    next.lock_source = None;
    next.override_source = None;
    next.router_reason = payload.reason;
    next.updated_at = Utc::now();
    let (session, changed) = commit_mode_state(&state, session, next).await?;
    Ok(Json(mode_response(
        &state.persona,
        session.mode_state,
        changed,
        None,
    )))
}

pub(super) async fn override_mode<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Json(payload): Json<ModeRequest>,
) -> Result<Json<ModeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    let mode = payload
        .mode
        .ok_or_else(|| ServerError::InvalidRequest("mode is required".to_string()))
        .and_then(|mode| validate_mode(&state.persona, mode))?;
    let mut next = session.mode_state.clone();
    next.mode = mode;
    next.router_reason = payload.reason;
    next.lock_source = None;
    next.override_source = Some(payload.source.unwrap_or_else(|| "user".to_string()));
    next.updated_at = Utc::now();
    let (session, changed) = commit_mode_state(&state, session, next).await?;
    Ok(Json(mode_response(
        &state.persona,
        session.mode_state,
        changed,
        None,
    )))
}

pub(super) fn mode_response(
    persona: &ModesConfig,
    mode_state: ModeState,
    changed: bool,
    ignored_reason: Option<String>,
) -> ModeResponse {
    let profile = mode_profile(persona, &mode_state.mode);
    ModeResponse {
        label: profile.label.clone(),
        capabilities: profile.capabilities.clone(),
        active_skills: profile.active_skills.clone(),
        profile,
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
    commit_mode_state_parts(
        &state.store,
        &state.persona,
        state.sender(session.id),
        session,
        next,
    )
    .await
}

pub(super) async fn commit_mode_state_parts<S>(
    store: &Arc<S>,
    persona: &ModesConfig,
    sender: broadcast::Sender<SessionEvent>,
    session: crate::SessionRecord,
    mut next: ModeState,
) -> Result<(crate::SessionRecord, bool)>
where
    S: Store,
{
    for value in [
        &mut next.router_reason,
        &mut next.lock_source,
        &mut next.override_source,
    ]
    .into_iter()
    .flatten()
    {
        *value = tm_memory::redact_dream_text(value).text;
    }
    let changed = session.mode_state != next;
    if !changed {
        return Ok((session, false));
    }
    let from = Some(session.mode_state.mode.clone());
    let updated = store.set_mode_state(session.id, next).await?;
    let profile = mode_profile(persona, &updated.mode_state.mode);
    let event = store
        .append_event(
            session.id,
            "mode",
            mode_changed_payload(
                from,
                &updated.mode_state,
                updated.persona_status.clone(),
                &profile,
            )?,
        )
        .await?;
    let _ = sender.send(event);
    Ok((updated, true))
}

pub(crate) fn mode_changed_payload(
    from: Option<ModeId>,
    mode_state: &ModeState,
    persona_status: crate::AssetStatus,
    profile: &ModeProfile,
) -> Result<Value> {
    Ok(serde_json::to_value(StoreEvent::ModeChanged(Box::new(
        crate::store::ModeChangedStoreEvent {
            from,
            mode: mode_state.mode.clone(),
            label: profile.label.clone(),
            capabilities: profile.capabilities.clone(),
            active_skills: profile.active_skills.clone(),
            router_reason: mode_state.router_reason.clone(),
            lock_source: mode_state.lock_source.clone(),
            override_source: mode_state.override_source.clone(),
            updated_at: mode_state.updated_at,
            persona_status,
        },
    )))?)
}

pub(crate) fn mode_profile(persona: &ModesConfig, mode: &ModeId) -> ModeProfile {
    persona.load_assets().profile_or_unknown(mode)
}

pub(super) fn active_skills(persona: &ModesConfig, mode: &ModeId) -> Vec<String> {
    mode_profile(persona, mode).active_skills
}

pub(super) fn validate_mode(persona: &ModesConfig, mode: ModeId) -> Result<ModeId> {
    let assets = persona.load_assets();
    if assets.mode_profile(&mode).is_some() {
        Ok(mode)
    } else {
        Err(ServerError::InvalidRequest(format!(
            "unknown mode {mode}; available modes are {}",
            assets
                .modes
                .modes
                .iter()
                .map(|profile| profile.mode.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}

pub(crate) fn build_turn_prompt<S, M, C>(
    state: &AppState<S, M, C>,
    mode: &ModeId,
    message: &str,
) -> tm_modes::ComposedPrompt {
    state.persona.build_system_prompt(
        mode,
        DEFAULT_SYSTEM_PROMPT,
        &linked_folder_capability_notes(&state.linked_folders),
        message,
    )
}

fn linked_folder_capability_notes(linked_folders: &LinkedFolders) -> String {
    if linked_folders.is_empty() {
        return "No linked folders configured; fs.*, code.*, and proc.* will fail closed."
            .to_string();
    }

    let policies = linked_folders
        .policies()
        .into_iter()
        .map(|policy| (policy.alias.clone(), policy))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut notes = String::new();
    for alias in linked_folders.aliases() {
        let mode = policies
            .get(&alias)
            .map(|policy| match policy.mode {
                tm_host::FsMode::Ro => "ro",
                tm_host::FsMode::Rw => "rw",
            })
            .unwrap_or("remote policy");
        notes.push_str(&format!(
            "Linked folders: {alias} ({mode}) at linked://{alias}/\n"
        ));
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::validate_mode;
    use crate::ServerError;
    use tm_modes::{ModeId, ModesConfig};

    #[test]
    fn validate_mode_accepts_known_modes() {
        let persona = ModesConfig::default();
        for id in ["general", "serious_engineer"] {
            assert!(validate_mode(&persona, ModeId::from(id)).is_ok(), "{id}");
        }
        assert!(matches!(
            validate_mode(&persona, ModeId::from("handoff")),
            Err(ServerError::InvalidRequest(_))
        ));
    }

    #[test]
    fn validate_mode_rejects_unknown_id() {
        let persona = ModesConfig::default();
        let err = validate_mode(&persona, ModeId::from("nonexistent_mode")).unwrap_err();
        assert!(
            matches!(err, ServerError::InvalidRequest(ref msg) if msg.contains("unknown mode")),
            "expected InvalidRequest with 'unknown mode', got: {err:?}"
        );
    }
}

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
    pub voice_cap: String,
    pub default_scope: String,
    pub capabilities: Vec<String>,
    pub active_skills: Vec<String>,
    pub profile: ModeProfile,
    pub changed: bool,
    pub ignored_reason: Option<String>,
}

pub(super) async fn list_modes<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    headers: HeaderMap,
) -> Result<Json<ModeCatalog>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    Ok(Json(state.persona.load_assets().modes))
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
            &state.persona,
            session.mode_state,
            false,
            Some("mode is locked by user".to_string()),
        )));
    }
    let mode = match payload.mode {
        Some(mode) => validate_mode(&state.persona, mode)?,
        None => session.mode_state.mode.clone(),
    };
    let mut next = session.mode_state.clone();
    next.mode = mode;
    next.router_reason = payload.reason;
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
    persona: &PersonaConfig,
    mode_state: ModeState,
    changed: bool,
    ignored_reason: Option<String>,
) -> ModeResponse {
    let profile = mode_profile(persona, &mode_state.mode);
    ModeResponse {
        label: profile.label.clone(),
        voice_cap: profile.voice_cap.clone(),
        default_scope: profile.default_scope.clone(),
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
    let changed = session.mode_state != next;
    if !changed {
        return Ok((session, false));
    }
    let from = Some(session.mode_state.mode.clone());
    let updated = state.store.set_mode_state(session.id, next).await?;
    let profile = mode_profile(&state.persona, &updated.mode_state.mode);
    let event = state
        .store
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
    let _ = state.sender(session.id).send(event);
    Ok((updated, true))
}

pub(super) fn mode_changed_payload(
    from: Option<ModeId>,
    mode_state: &ModeState,
    persona_status: crate::PersonaStatus,
    profile: &ModeProfile,
) -> Result<Value> {
    Ok(serde_json::to_value(StoreEvent::ModeChanged {
        from,
        mode: mode_state.mode.clone(),
        label: profile.label.clone(),
        voice_cap: profile.voice_cap.clone(),
        capabilities: profile.capabilities.clone(),
        active_skills: profile.active_skills.clone(),
        router_reason: mode_state.router_reason.clone(),
        lock_source: mode_state.lock_source.clone(),
        override_source: mode_state.override_source.clone(),
        updated_at: mode_state.updated_at,
        persona_status,
    })?)
}

pub(super) fn mode_profile(persona: &PersonaConfig, mode: &ModeId) -> ModeProfile {
    persona.load_assets().profile_or_unknown(mode)
}

pub(super) fn active_skills(persona: &PersonaConfig, mode: &ModeId) -> Vec<String> {
    mode_profile(persona, mode).active_skills
}

pub(super) fn validate_mode(persona: &PersonaConfig, mode: ModeId) -> Result<ModeId> {
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

pub(super) fn build_turn_prompt<S, M, C>(
    state: &AppState<S, M, C>,
    mode: &ModeId,
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

#[cfg(test)]
mod tests {
    use super::validate_mode;
    use crate::ServerError;
    use tm_persona::{ModeId, PersonaConfig};

    #[test]
    fn validate_mode_accepts_known_modes() {
        let persona = PersonaConfig::default();
        for id in ["personal_assistant", "serious_engineer", "handoff"] {
            assert!(validate_mode(&persona, ModeId::from(id)).is_ok(), "{id}");
        }
    }

    #[test]
    fn validate_mode_rejects_unknown_id() {
        let persona = PersonaConfig::default();
        let err = validate_mode(&persona, ModeId::from("nonexistent_mode")).unwrap_err();
        assert!(
            matches!(err, ServerError::InvalidRequest(ref msg) if msg.contains("unknown mode")),
            "expected InvalidRequest with 'unknown mode', got: {err:?}"
        );
    }
}

pub(super) fn route_mode_for_prompt(catalog: &ModeCatalog, content: &str) -> (ModeId, String) {
    let lower = content.to_lowercase();
    let mut profiles = catalog.modes.iter().collect::<Vec<_>>();
    profiles.sort_by_key(|profile| std::cmp::Reverse(profile.route.priority));

    for profile in profiles {
        if profile.route.is_default {
            continue;
        }
        if let Some(trigger) = profile
            .route
            .triggers
            .iter()
            .find(|trigger| lower.contains(&trigger.to_lowercase()))
        {
            return (
                profile.mode.clone(),
                format!("runtime mode trigger for {}: {trigger}", profile.mode),
            );
        }
    }

    (
        catalog.default_mode(),
        "default runtime mode route for unmatched prompt".to_string(),
    )
}

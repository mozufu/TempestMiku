use axum::{
    Extension, Json,
    extract::{Path, State},
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, AuthPrincipal, ChatRunner, MemoryProvider, Result, ServerError, Store};

const MAX_PROVIDER_CHARS: usize = 64;
const MAX_REGISTRATION_CHARS: usize = 4096;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RegisterPushRequest {
    provider: String,
    registration: String,
}

pub(super) async fn register<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(payload): Json<RegisterPushRequest>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let AuthPrincipal::Device { device, .. } = principal else {
        return Err(ServerError::Policy(
            "push registration requires device authentication".to_string(),
        ));
    };
    let push = state
        .push
        .as_ref()
        .ok_or_else(|| ServerError::Policy("push notifications are not configured".to_string()))?;
    let provider = bounded("provider", payload.provider, MAX_PROVIDER_CHARS)?;
    let registration = bounded("registration", payload.registration, MAX_REGISTRATION_CHARS)?;
    let metadata = push.register(device.id, &provider, &registration).await?;
    Ok(Json(json!({ "registration": metadata })))
}

pub(super) async fn unregister<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let AuthPrincipal::Device { device, .. } = principal else {
        return Err(ServerError::Policy(
            "push registration requires device authentication".to_string(),
        ));
    };
    if let Some(push) = &state.push {
        push.unregister(device.id).await?;
    }
    Ok(Json(json!({ "status": "unregistered" })))
}

pub(super) async fn approval<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, approval_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.store.get_session(session_id).await?;
    let approval = state
        .store
        .approval_request(session_id, approval_id)
        .await?;
    Ok(Json(json!({
        "approvalId": approval.id,
        "sessionId": approval.session_id,
        "backend": approval.origin,
        "action": approval.action,
        "scope": approval.scope_json,
        "options": approval.options_json,
        "status": approval.status,
        "createdAt": approval.created_at,
        "expiresAt": approval.expires_at,
        "resolvedAt": approval.resolved_at,
        "serverTime": Utc::now(),
    })))
}

fn bounded(field: &str, value: String, max_chars: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max_chars || value.chars().any(char::is_control)
    {
        return Err(ServerError::InvalidRequest(format!(
            "{field} must contain 1-{max_chars} non-control characters"
        )));
    }
    Ok(value.to_string())
}

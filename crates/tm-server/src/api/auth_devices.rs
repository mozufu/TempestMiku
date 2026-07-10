use std::{net::SocketAddr, time::Duration};

use axum::{
    Extension, Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::IntoResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    AppState, AuthDeviceRecord, AuthPrincipal, ChatRunner, MemoryProvider, NewAuthDevice,
    NewPairingCode, Result, ServerError, Store,
    auth::{hash_secret, new_device_token, new_pairing_code},
};

const PAIRING_CODE_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PairDeviceRequest {
    code: String,
    device_name: String,
    platform: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PairDeviceResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    device: AuthDeviceRecord,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PairingCodeResponse {
    pub code: String,
    pub pairing_link: String,
    pub expires_at: chrono::DateTime<Utc>,
}

pub(super) async fn create_pairing_code<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
) -> Result<Json<PairingCodeResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.validate_cookie_mutation(&headers)?;
    let base_url = super::pairing::public_base_url(&headers, connect.map(|value| value.0), &state);
    issue_pairing_code(&state, &headers, connect.map(|value| value.0), &base_url)
        .await
        .map(Json)
}

pub(super) async fn issue_pairing_code<S, M, C>(
    state: &AppState<S, M, C>,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    base_url: &str,
) -> Result<PairingCodeResponse>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    if state.auth.device_config().is_none() {
        return Err(ServerError::Policy(
            "device pairing requires TM_AUTH_MODE=device".to_string(),
        ));
    }
    let created_by_device_id = state.auth.pairing_code_creator(headers, peer).await?;
    let now = Utc::now();
    let expires_at = now
        + chrono::Duration::from_std(PAIRING_CODE_TTL)
            .expect("pairing code TTL fits chrono duration");
    let code = new_pairing_code()?;
    state
        .auth
        .devices()
        .create_pairing_code(NewPairingCode {
            id: Uuid::new_v4(),
            code_hash: hash_secret(&code),
            created_at: now,
            expires_at,
            created_by_device_id,
        })
        .await?;
    Ok(PairingCodeResponse {
        pairing_link: super::pairing::pairing_link(base_url, &code),
        code,
        expires_at,
    })
}

pub(super) async fn pair_device<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Json(payload): Json<PairDeviceRequest>,
) -> Result<impl IntoResponse>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let config = state.auth.device_config().ok_or_else(|| {
        ServerError::Policy("device pairing requires TM_AUTH_MODE=device".to_string())
    })?;
    let code = required_bounded("code", payload.code, 128)?;
    let name = required_bounded("deviceName", payload.device_name, 120)?;
    let platform = required_bounded("platform", payload.platform, 64)?;
    let cookie_only = platform.eq_ignore_ascii_case("web");
    let token = new_device_token()?;
    let now = Utc::now();
    let device = state
        .auth
        .devices()
        .consume_pairing_code(
            &hash_secret(&code),
            NewAuthDevice {
                id: Uuid::new_v4(),
                owner_subject: config.owner_subject.clone(),
                name,
                platform,
                token_hash: hash_secret(&token),
                created_at: now,
            },
            now,
        )
        .await?;

    let cookie = device_cookie(&config.cookie_name, &token, config.secure_cookie);
    let mut response = Json(PairDeviceResponse {
        token: (!cookie_only).then_some(token),
        device,
    })
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie)
            .map_err(|err| ServerError::InvalidRequest(err.to_string()))?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub(super) async fn logout<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<impl IntoResponse>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let AuthPrincipal::Device { device, .. } = principal else {
        return Err(ServerError::Policy(
            "logout is available only for device authentication".to_string(),
        ));
    };
    let config = state
        .auth
        .device_config()
        .ok_or_else(|| ServerError::Policy("logout requires TM_AUTH_MODE=device".to_string()))?;
    let device = state
        .auth
        .devices()
        .revoke_auth_device(device.id, Utc::now())
        .await?;
    let mut response = Json(json!({ "device": device })).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&expired_device_cookie(
            &config.cookie_name,
            config.secure_cookie,
        ))
        .map_err(|err| ServerError::InvalidRequest(err.to_string()))?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub(super) async fn list_devices<S, M, C>(
    State(state): State<AppState<S, M, C>>,
) -> Result<Json<serde_json::Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    Ok(Json(
        json!({ "devices": state.auth.devices().auth_devices().await? }),
    ))
}

pub(super) async fn revoke_device<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(device_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let device = state
        .auth
        .devices()
        .revoke_auth_device(device_id, Utc::now())
        .await?;
    Ok(Json(json!({ "device": device })))
}

fn device_cookie(name: &str, token: &str, secure: bool) -> String {
    format!(
        "{name}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000{}",
        if secure { "; Secure" } else { "" }
    )
}

fn expired_device_cookie(name: &str, secure: bool) -> String {
    format!(
        "{name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT{}",
        if secure { "; Secure" } else { "" }
    )
}

fn required_bounded(field: &str, value: String, max_chars: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ServerError::InvalidRequest(format!(
            "{field} must not be empty"
        )));
    }
    if value.chars().count() > max_chars {
        return Err(ServerError::InvalidRequest(format!(
            "{field} exceeds {max_chars} characters"
        )));
    }
    Ok(value.to_string())
}

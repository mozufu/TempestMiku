use std::{fmt, time::Duration};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use serde::Deserialize;
use url::Url;

use crate::{Result, ServerError};

use super::{PushMessage, PushProvider, PushProviderOutcome, PushProviderResult};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UnifiedPushRegistration {
    endpoint: String,
    p256dh: String,
    auth: String,
}

#[derive(Clone)]
pub struct UnifiedPushProvider {
    client: reqwest::Client,
    allowed_origin: Url,
}

impl fmt::Debug for UnifiedPushProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UnifiedPushProvider")
            .field("allowed_origin", &self.allowed_origin.as_str())
            .finish_non_exhaustive()
    }
}

impl UnifiedPushProvider {
    pub fn new(allowed_origin: &str) -> Result<Self> {
        Self::build(allowed_origin, false)
    }

    pub(super) fn build(allowed_origin: &str, allow_http: bool) -> Result<Self> {
        let mut allowed_origin = Url::parse(allowed_origin).map_err(|_| {
            ServerError::InvalidRequest(
                "TM_UNIFIED_PUSH_ENDPOINT_ORIGIN must be an absolute URL".to_string(),
            )
        })?;
        if (!allow_http && allowed_origin.scheme() != "https")
            || allowed_origin.cannot_be_a_base()
            || allowed_origin.host_str().is_none()
            || !allowed_origin.username().is_empty()
            || allowed_origin.password().is_some()
            || allowed_origin.query().is_some()
            || allowed_origin.fragment().is_some()
        {
            return Err(ServerError::InvalidRequest(
                "TM_UNIFIED_PUSH_ENDPOINT_ORIGIN must be an HTTPS origin without credentials, path, query, or fragment"
                    .to_string(),
            ));
        }
        if allowed_origin.path() != "/" {
            return Err(ServerError::InvalidRequest(
                "TM_UNIFIED_PUSH_ENDPOINT_ORIGIN must not contain a path".to_string(),
            ));
        }
        allowed_origin.set_path("");
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        Ok(Self {
            client,
            allowed_origin,
        })
    }

    pub(super) fn parse_registration(&self, raw: &str) -> Result<(Url, Vec<u8>, Vec<u8>)> {
        let registration: UnifiedPushRegistration = serde_json::from_str(raw).map_err(|_| {
            ServerError::InvalidRequest("invalid UnifiedPush registration envelope".to_string())
        })?;
        let endpoint = Url::parse(&registration.endpoint).map_err(|_| {
            ServerError::InvalidRequest("invalid UnifiedPush endpoint URL".to_string())
        })?;
        if endpoint.origin() != self.allowed_origin.origin()
            || endpoint.scheme() != self.allowed_origin.scheme()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(ServerError::Policy(
                "UnifiedPush endpoint is outside the configured origin".to_string(),
            ));
        }
        let public_key = URL_SAFE_NO_PAD.decode(registration.p256dh).map_err(|_| {
            ServerError::InvalidRequest("invalid UnifiedPush p256dh key".to_string())
        })?;
        let auth = URL_SAFE_NO_PAD.decode(registration.auth).map_err(|_| {
            ServerError::InvalidRequest("invalid UnifiedPush auth secret".to_string())
        })?;
        if public_key.len() != 65 || public_key.first() != Some(&4) || auth.len() != 16 {
            return Err(ServerError::InvalidRequest(
                "invalid UnifiedPush Web Push key material".to_string(),
            ));
        }
        Ok((endpoint, public_key, auth))
    }

    fn invalid_registration(error: impl fmt::Display) -> PushProviderResult {
        PushProviderResult {
            outcome: PushProviderOutcome::PermanentRegistrationFailure,
            error: Some(error.to_string()),
        }
    }

    fn transient(error: impl fmt::Display) -> PushProviderResult {
        PushProviderResult {
            outcome: PushProviderOutcome::TransientFailure,
            error: Some(error.to_string()),
        }
    }
}

#[async_trait]
impl PushProvider for UnifiedPushProvider {
    fn name(&self) -> &str {
        "unifiedpush"
    }

    async fn deliver(&self, registration: &str, message: &PushMessage) -> PushProviderResult {
        let (endpoint, public_key, auth) = match self.parse_registration(registration) {
            Ok(parsed) => parsed,
            Err(error) => return Self::invalid_registration(error),
        };
        let payload = match serde_json::to_vec(message) {
            Ok(payload) => payload,
            Err(_) => return Self::transient("UnifiedPush payload serialization failed"),
        };
        let encrypted = match ece::encrypt(&public_key, &auth, &payload) {
            Ok(encrypted) => encrypted,
            Err(_) => {
                return Self::invalid_registration("UnifiedPush registration encryption failed");
            }
        };
        let ttl = (message.expires_at - Utc::now())
            .num_seconds()
            .clamp(0, 3600);
        let response = match self
            .client
            .post(endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(reqwest::header::CONTENT_ENCODING, "aes128gcm")
            .header("TTL", ttl)
            .header("Urgency", "high")
            .body(encrypted)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => return Self::transient("UnifiedPush delivery transport failed"),
        };
        let status = response.status();
        if status.is_success() {
            PushProviderResult::delivered()
        } else if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            Self::invalid_registration(format!("UnifiedPush endpoint returned {status}"))
        } else {
            Self::transient(format!("UnifiedPush endpoint returned {status}"))
        }
    }
}

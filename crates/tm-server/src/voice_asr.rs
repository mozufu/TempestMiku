use std::{fmt, net::Ipv4Addr, sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    Json,
    body::Bytes,
    extract::{Extension, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Semaphore;
use url::{Host, Url};
use uuid::Uuid;

use crate::{AppState, AuthPrincipal, ChatRunner, MemoryProvider, Result, ServerError, Store};

pub const SELF_HOSTED_ASR_ENGINE_ID: &str = "self_hosted";
pub const MAX_ASR_PCM_BYTES: usize = 1_920_000;
pub const MAX_ASR_DURATION_SECONDS: u32 = 60;
const SAMPLE_RATE_HZ: u32 = 16_000;
const CHANNELS: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;
const MAX_UPSTREAM_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_TRANSCRIPT_BYTES: usize = 16 * 1024;
const TRANSCRIBE_PATH: &str = "/transcribe";

pub const ENGINE_ID_HEADER: &str = "x-tm-asr-engine-id";
pub const CAPTURE_ID_HEADER: &str = "x-tm-capture-id";
pub const SAMPLE_RATE_HEADER: &str = "x-tm-sample-rate";
pub const CHANNELS_HEADER: &str = "x-tm-channels";

#[derive(Clone)]
pub struct SelfHostedAsrConfig {
    endpoint: Url,
    label: String,
    model_id: String,
}

impl fmt::Debug for SelfHostedAsrConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SelfHostedAsrConfig")
            .field("endpoint", &"[REDACTED]")
            .field("label", &self.label)
            .field("model_id", &self.model_id)
            .finish()
    }
}

impl SelfHostedAsrConfig {
    pub fn new(endpoint: &str, label: &str, model_id: &str) -> Result<Self> {
        Self::build(endpoint, label, model_id, false)
    }

    fn build(
        endpoint: &str,
        label: &str,
        model_id: &str,
        allow_test_loopback_http: bool,
    ) -> Result<Self> {
        let endpoint = Url::parse(endpoint).map_err(|_| {
            ServerError::InvalidRequest(
                "TM_SELF_HOSTED_ASR_ENDPOINT must be an absolute URL".to_string(),
            )
        })?;
        validate_endpoint(&endpoint, allow_test_loopback_http)?;
        let label = validate_metadata("TM_SELF_HOSTED_ASR_LABEL", label, 80)?;
        let model_id = validate_metadata("TM_SELF_HOSTED_ASR_MODEL_ID", model_id, 160)?;
        Ok(Self {
            endpoint,
            label,
            model_id,
        })
    }
}

fn validate_endpoint(endpoint: &Url, allow_test_loopback_http: bool) -> Result<()> {
    let common_invalid = endpoint.cannot_be_a_base()
        || endpoint.host().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || endpoint.path() != TRANSCRIBE_PATH;
    if common_invalid {
        return Err(invalid_endpoint());
    }

    match endpoint.scheme() {
        "https" => Ok(()),
        "http" => match endpoint.host() {
            Some(Host::Ipv4(address))
                if is_tailscale_cgnat(address)
                    || (allow_test_loopback_http && address.is_loopback()) =>
            {
                Ok(())
            }
            _ => Err(invalid_endpoint()),
        },
        _ => Err(invalid_endpoint()),
    }
}

fn invalid_endpoint() -> ServerError {
    ServerError::InvalidRequest(
        "TM_SELF_HOSTED_ASR_ENDPOINT must pin /transcribe on HTTPS, or HTTP at a literal Tailscale CGNAT address, without credentials, query, or fragment"
            .to_string(),
    )
}

fn is_tailscale_cgnat(address: Ipv4Addr) -> bool {
    let address = u32::from(address);
    let network = u32::from(Ipv4Addr::new(100, 64, 0, 0));
    (address & 0xffc0_0000) == network
}

fn validate_metadata(name: &str, value: &str, max_bytes: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(ServerError::InvalidRequest(format!(
            "{name} must be 1-{max_bytes} non-control UTF-8 bytes"
        )));
    }
    Ok(value.to_string())
}

#[derive(Clone)]
pub struct SelfHostedAsr {
    config: SelfHostedAsrConfig,
    transport: Arc<dyn AsrTransport>,
    in_flight: Arc<Semaphore>,
}

impl fmt::Debug for SelfHostedAsr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SelfHostedAsr")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SelfHostedAsr {
    pub fn new(config: SelfHostedAsrConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(45))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| {
                ServerError::InvalidRequest(
                    "self-hosted ASR HTTP client could not be initialized".to_string(),
                )
            })?;
        Ok(Self {
            config,
            transport: Arc::new(HttpAsrTransport { client }),
            in_flight: Arc::new(Semaphore::new(1)),
        })
    }

    #[cfg(test)]
    fn for_test(
        endpoint: &str,
        label: &str,
        model_id: &str,
        transport: Arc<dyn AsrTransport>,
    ) -> Self {
        Self {
            config: SelfHostedAsrConfig::build(endpoint, label, model_id, true)
                .expect("valid test ASR endpoint"),
            transport,
            in_flight: Arc::new(Semaphore::new(1)),
        }
    }

    fn engine(&self) -> AsrEngine {
        AsrEngine {
            id: SELF_HOSTED_ASR_ENGINE_ID,
            kind: "remote",
            label: self.config.label.clone(),
            available: true,
            model_id: Some(self.config.model_id.clone()),
            max_duration_seconds: MAX_ASR_DURATION_SECONDS,
        }
    }

    async fn transcribe(&self, pcm: Bytes) -> Result<String> {
        let wav = pcm16_wav(&pcm)?;
        let boundary = format!("tm-asr-{}", Uuid::new_v4().simple());
        let body = multipart_body(&boundary, &wav);
        let bytes = self
            .transport
            .transcribe(&self.config.endpoint, &boundary, body)
            .await
            .map_err(|_| upstream_failed())?;
        if bytes.len() > MAX_UPSTREAM_RESPONSE_BYTES {
            return Err(upstream_failed());
        }
        let response: UpstreamResponse =
            serde_json::from_slice(&bytes).map_err(|_| upstream_failed())?;
        let text = response.text.trim();
        if text.is_empty()
            || text.len() > MAX_TRANSCRIPT_BYTES
            || text
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        {
            return Err(upstream_failed());
        }
        Ok(text.to_string())
    }
}

#[async_trait]
trait AsrTransport: Send + Sync {
    async fn transcribe(
        &self,
        endpoint: &Url,
        boundary: &str,
        body: Vec<u8>,
    ) -> std::result::Result<Vec<u8>, ()>;
}

struct HttpAsrTransport {
    client: reqwest::Client,
}

#[async_trait]
impl AsrTransport for HttpAsrTransport {
    async fn transcribe(
        &self,
        endpoint: &Url,
        boundary: &str,
        body: Vec<u8>,
    ) -> std::result::Result<Vec<u8>, ()> {
        let response = self
            .client
            .post(endpoint.clone())
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await
            .map_err(|_| ())?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|size| size > MAX_UPSTREAM_RESPONSE_BYTES as u64)
        {
            return Err(());
        }

        let mut bytes = Vec::with_capacity(MAX_UPSTREAM_RESPONSE_BYTES.min(8 * 1024));
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ())?;
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|size| size > MAX_UPSTREAM_RESPONSE_BYTES)
            {
                return Err(());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

fn upstream_failed() -> ServerError {
    ServerError::Backend("self-hosted ASR transcription failed".to_string())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpstreamResponse {
    text: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct AsrEngine {
    id: &'static str,
    kind: &'static str,
    label: String,
    available: bool,
    model_id: Option<String>,
    max_duration_seconds: u32,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct AsrEnginesResponse {
    engines: Vec<AsrEngine>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct TranscriptionResponse {
    text: String,
    engine_id: &'static str,
    model_id: String,
}

pub(crate) async fn engines<S, M, C>(State(state): State<AppState<S, M, C>>) -> impl IntoResponse
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let mut engines = vec![AsrEngine {
        id: "local",
        kind: "local",
        label: "在此裝置".to_string(),
        available: true,
        model_id: None,
        max_duration_seconds: MAX_ASR_DURATION_SECONDS,
    }];
    engines.push(match state.self_hosted_asr.as_deref() {
        Some(service) => service.engine(),
        None => AsrEngine {
            id: SELF_HOSTED_ASR_ENGINE_ID,
            kind: "remote",
            label: "家用自架遠端".to_string(),
            available: false,
            model_id: None,
            max_duration_seconds: MAX_ASR_DURATION_SECONDS,
        },
    });
    Json(AsrEnginesResponse { engines })
}

pub(crate) async fn transcriptions<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Extension(principal): Extension<AuthPrincipal>,
    headers: HeaderMap,
    pcm: Bytes,
) -> Result<Response>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    if !matches!(principal, AuthPrincipal::Device { .. }) {
        return Err(ServerError::Forbidden);
    }
    validate_request(&headers, &pcm)?;
    let service = state.self_hosted_asr.as_deref().ok_or_else(|| {
        ServerError::NotFound("self-hosted ASR engine is not configured".to_string())
    })?;
    let _permit = match Arc::clone(&service.in_flight).try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return Ok((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error": "self-hosted ASR is busy"})),
            )
                .into_response());
        }
    };
    let text = service.transcribe(pcm).await?;
    Ok(Json(TranscriptionResponse {
        text,
        engine_id: SELF_HOSTED_ASR_ENGINE_ID,
        model_id: service.config.model_id.clone(),
    })
    .into_response())
}

fn validate_request(headers: &HeaderMap, pcm: &Bytes) -> Result<()> {
    let content_type = required_header(headers, header::CONTENT_TYPE.as_str())?;
    if content_type != "application/octet-stream" {
        return Err(ServerError::InvalidRequest(
            "voice transcription content type must be application/octet-stream".to_string(),
        ));
    }
    if required_header(headers, ENGINE_ID_HEADER)? != SELF_HOSTED_ASR_ENGINE_ID {
        return Err(ServerError::InvalidRequest(
            "unsupported ASR engine id".to_string(),
        ));
    }
    required_header(headers, CAPTURE_ID_HEADER)?
        .parse::<Uuid>()
        .map_err(|_| ServerError::InvalidRequest("invalid capture id".to_string()))?;
    if parse_u32_header(headers, SAMPLE_RATE_HEADER)? != SAMPLE_RATE_HZ {
        return Err(ServerError::InvalidRequest(
            "ASR sample rate must be 16000 Hz".to_string(),
        ));
    }
    if parse_u16_header(headers, CHANNELS_HEADER)? != CHANNELS {
        return Err(ServerError::InvalidRequest(
            "ASR audio must have one channel".to_string(),
        ));
    }
    if pcm.is_empty() || !pcm.len().is_multiple_of(2) || pcm.len() > MAX_ASR_PCM_BYTES {
        return Err(ServerError::InvalidRequest(
            "ASR body must contain up to 60 seconds of complete PCM16 samples".to_string(),
        ));
    }
    Ok(())
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ServerError::InvalidRequest(format!("missing or invalid {name} header")))
}

fn parse_u32_header(headers: &HeaderMap, name: &str) -> Result<u32> {
    required_header(headers, name)?
        .parse()
        .map_err(|_| ServerError::InvalidRequest(format!("invalid {name} header")))
}

fn parse_u16_header(headers: &HeaderMap, name: &str) -> Result<u16> {
    required_header(headers, name)?
        .parse()
        .map_err(|_| ServerError::InvalidRequest(format!("invalid {name} header")))
}

fn pcm16_wav(pcm: &[u8]) -> Result<Vec<u8>> {
    if pcm.len() > MAX_ASR_PCM_BYTES || !pcm.len().is_multiple_of(2) || pcm.is_empty() {
        return Err(ServerError::InvalidRequest(
            "invalid PCM16 audio body".to_string(),
        ));
    }
    let data_size = u32::try_from(pcm.len())
        .map_err(|_| ServerError::InvalidRequest("PCM16 audio body is too large".to_string()))?;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&CHANNELS.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE_HZ.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE_HZ * u32::from(CHANNELS) * 2).to_le_bytes());
    wav.extend_from_slice(&(CHANNELS * 2).to_le_bytes());
    wav.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm);
    Ok(wav)
}

fn multipart_body(boundary: &str, wav: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(wav.len() + 512);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"capture.wav\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
    body.extend_from_slice(wav);
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"language\"\r\n\r\n");
    body.extend_from_slice(b"Chinese");
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use parking_lot::Mutex;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::{
        AuthConfig, AuthDeviceStore, DeviceAuthConfig, EchoChatRunner, InMemoryAuthDeviceStore,
        InMemoryStore, ModesConfig, NewAuthDevice, NewPairingCode, StoreMemoryProvider, app,
        auth::hash_secret,
    };

    use super::*;

    #[test]
    fn endpoint_policy_allows_only_https_or_literal_tailscale_http() {
        for endpoint in [
            "https://asr.example.test/transcribe",
            "http://100.64.0.1:19000/transcribe",
            "http://100.110.95.111:19000/transcribe",
            "http://100.127.255.254/transcribe",
        ] {
            SelfHostedAsrConfig::new(endpoint, "家用 ASR", "tea-asr-1.1-mini")
                .unwrap_or_else(|error| panic!("{endpoint}: {error}"));
        }
        for endpoint in [
            "http://100.63.255.255/transcribe",
            "http://100.128.0.1/transcribe",
            "http://127.0.0.1/transcribe",
            "http://asr.example.test/transcribe",
            "ftp://100.110.95.111/transcribe",
            "https://user@asr.example.test/transcribe",
            "https://asr.example.test/other",
            "https://asr.example.test/transcribe?language=Chinese",
            "https://asr.example.test/transcribe#fragment",
        ] {
            assert!(
                SelfHostedAsrConfig::new(endpoint, "家用 ASR", "tea-asr-1.1-mini").is_err(),
                "unexpectedly accepted {endpoint}"
            );
        }
    }

    #[test]
    fn wav_is_canonical_16khz_mono_pcm16() {
        let wav = pcm16_wav(&[1, 2, 3, 4]).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 40);
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        assert_eq!(&wav[44..], &[1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn endpoints_are_authenticated_and_catalog_never_exposes_the_endpoint() {
        let (service, captured) = fake_service(Ok(json!({"text":"幫我記得倒垃圾"})));
        let static_app = test_app(
            AuthConfig::BearerToken("secret".to_string()),
            Some(service.clone()),
        );

        let denied = static_app
            .clone()
            .oneshot(request("GET", "/voice/asr/engines", Body::empty(), None))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let catalog = static_app
            .clone()
            .oneshot(request(
                "GET",
                "/voice/asr/engines",
                Body::empty(),
                Some("secret"),
            ))
            .await
            .unwrap();
        assert_eq!(catalog.status(), StatusCode::OK);
        let catalog = response_json(catalog).await;
        assert_eq!(catalog["engines"][0]["id"], "local");
        assert_eq!(catalog["engines"][1]["id"], SELF_HOSTED_ASR_ENGINE_ID);
        assert_eq!(catalog["engines"][1]["available"], true);
        assert_eq!(catalog["engines"][1]["modelId"], "tea-asr-1.1-mini");
        assert!(!catalog.to_string().contains("127.0.0.1"));

        let denied = static_app
            .clone()
            .oneshot(transcription_request(vec![0, 0], None))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let non_device = static_app
            .oneshot(transcription_request(vec![0, 0], Some("secret")))
            .await
            .unwrap();
        assert_eq!(non_device.status(), StatusCode::FORBIDDEN);

        let (device_app, token) = device_test_app(Some(service)).await;
        let response = device_app
            .oneshot(transcription_request(
                vec![0, 0, 1, 0],
                Some(token.as_str()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            json!({
                "text": "幫我記得倒垃圾",
                "engineId": "self_hosted",
                "modelId": "tea-asr-1.1-mini"
            })
        );
        let captured = captured.lock();
        let captured = captured.as_ref().expect("upstream request");
        assert_eq!(captured.endpoint, "http://127.0.0.1:19000/transcribe");
        assert!(captured.body.windows(4).any(|window| window == b"RIFF"));
        assert!(captured.body.windows(7).any(|window| window == b"Chinese"));
        assert!(captured.boundary.starts_with("tm-asr-"));
    }

    #[tokio::test]
    async fn disabled_and_invalid_requests_fail_without_upstream_or_fallback() {
        let (disabled, token) = device_test_app(None).await;
        let catalog = disabled
            .clone()
            .oneshot(request(
                "GET",
                "/voice/asr/engines",
                Body::empty(),
                Some(token.as_str()),
            ))
            .await
            .unwrap();
        let catalog = response_json(catalog).await;
        assert_eq!(catalog["engines"][1]["available"], false);
        let response = disabled
            .oneshot(transcription_request(vec![0, 0], Some(token.as_str())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let (service, captured) = fake_service(Ok(json!({"text":"unused"})));
        let (app, token) = device_test_app(Some(service)).await;
        for body in [Vec::new(), vec![0], vec![0; MAX_ASR_PCM_BYTES + 2]] {
            let response = app
                .clone()
                .oneshot(transcription_request(body, Some(token.as_str())))
                .await
                .unwrap();
            assert!(
                matches!(
                    response.status(),
                    StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE
                ),
                "unexpected status {}",
                response.status()
            );
        }
        assert!(captured.lock().is_none());
    }

    #[tokio::test]
    async fn request_format_and_engine_id_are_exact() {
        let (service, captured) = fake_service(Ok(json!({"text":"unused"})));
        let (app, token) = device_test_app(Some(service)).await;
        let mut requests = Vec::new();

        let mut wrong_engine = transcription_request(vec![0, 0], Some(token.as_str()));
        wrong_engine.headers_mut().insert(
            ENGINE_ID_HEADER,
            axum::http::HeaderValue::from_static("local"),
        );
        requests.push(wrong_engine);

        let mut wrong_capture = transcription_request(vec![0, 0], Some(token.as_str()));
        wrong_capture.headers_mut().insert(
            CAPTURE_ID_HEADER,
            axum::http::HeaderValue::from_static("not-a-uuid"),
        );
        requests.push(wrong_capture);

        let mut wrong_rate = transcription_request(vec![0, 0], Some(token.as_str()));
        wrong_rate.headers_mut().insert(
            SAMPLE_RATE_HEADER,
            axum::http::HeaderValue::from_static("8000"),
        );
        requests.push(wrong_rate);

        let mut wrong_channels = transcription_request(vec![0, 0], Some(token.as_str()));
        wrong_channels
            .headers_mut()
            .insert(CHANNELS_HEADER, axum::http::HeaderValue::from_static("2"));
        requests.push(wrong_channels);

        let mut wrong_content_type = transcription_request(vec![0, 0], Some(token.as_str()));
        wrong_content_type.headers_mut().insert(
            header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("audio/wav"),
        );
        requests.push(wrong_content_type);

        for request in requests {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        assert!(captured.lock().is_none());
    }

    #[tokio::test]
    async fn upstream_error_body_and_transcript_are_not_reflected() {
        let (service, _) = fake_service(Err(
            "secret transcript and http://private.invalid".to_string()
        ));
        let (app, token) = device_test_app(Some(service)).await;
        let response = app
            .oneshot(transcription_request(vec![0, 0], Some(token.as_str())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = response_json(response).await.to_string();
        assert_eq!(
            body,
            json!({"error":"backend error: self-hosted ASR transcription failed"}).to_string()
        );
        assert!(!body.contains("secret transcript"));
        assert!(!body.contains("private.invalid"));
    }

    #[tokio::test]
    async fn empty_control_and_oversized_upstream_transcripts_fail_closed() {
        for transcript in [
            "   ".to_string(),
            "contains\0control".to_string(),
            "字".repeat(MAX_TRANSCRIPT_BYTES),
        ] {
            let (service, _) = fake_service(Ok(json!({"text": transcript})));
            let (app, token) = device_test_app(Some(service)).await;
            let response = app
                .oneshot(transcription_request(vec![0, 0], Some(token.as_str())))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            assert_eq!(
                response_json(response).await,
                json!({"error":"backend error: self-hosted ASR transcription failed"})
            );
        }
    }

    #[tokio::test]
    async fn only_one_remote_transcription_can_be_in_flight() {
        let (service, captured) = fake_service(Ok(json!({"text":"unused"})));
        let permit = Arc::clone(&service.in_flight)
            .try_acquire_owned()
            .expect("reserve the sole ASR slot");
        let (app, token) = device_test_app(Some(service)).await;
        let response = app
            .oneshot(transcription_request(vec![0, 0], Some(token.as_str())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response_json(response).await,
            json!({"error":"self-hosted ASR is busy"})
        );
        assert!(captured.lock().is_none());
        drop(permit);
    }

    fn test_app(auth: AuthConfig, service: Option<SelfHostedAsr>) -> Router {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let mut state = AppState::new(
            store,
            memory,
            Arc::new(EchoChatRunner),
            ModesConfig::default(),
            auth,
        );
        if let Some(service) = service {
            state = state.with_self_hosted_asr(Arc::new(service));
        }
        app(state)
    }

    async fn device_test_app(service: Option<SelfHostedAsr>) -> (Router, String) {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let auth_store = Arc::new(InMemoryAuthDeviceStore::default());
        let now = chrono::Utc::now();
        let pairing_code = format!("pairing-{}", Uuid::new_v4());
        let token = format!("device-{}", Uuid::new_v4());
        auth_store
            .create_pairing_code(NewPairingCode {
                id: Uuid::new_v4(),
                code_hash: hash_secret(&pairing_code),
                created_at: now,
                expires_at: now + chrono::Duration::minutes(5),
                created_by_device_id: None,
            })
            .await
            .unwrap();
        auth_store
            .consume_pairing_code(
                &hash_secret(&pairing_code),
                NewAuthDevice {
                    id: Uuid::new_v4(),
                    owner_subject: "brian".to_string(),
                    name: "ASR test device".to_string(),
                    platform: "android".to_string(),
                    token_hash: hash_secret(&token),
                    created_at: now,
                },
                now,
            )
            .await
            .unwrap();
        let mut state = AppState::new(
            store,
            memory,
            Arc::new(EchoChatRunner),
            ModesConfig::default(),
            AuthConfig::Device(DeviceAuthConfig {
                cookie_name: "tm_device".to_string(),
                secure_cookie: true,
                owner_subject: "brian".to_string(),
                bootstrap_token_hash: None,
                allow_loopback_pairing: false,
                allowed_origin: None,
            }),
        )
        .with_auth_store(auth_store);
        if let Some(service) = service {
            state = state.with_self_hosted_asr(Arc::new(service));
        }
        (app(state), token)
    }

    #[derive(Debug)]
    struct CapturedUpstreamRequest {
        endpoint: String,
        boundary: String,
        body: Vec<u8>,
    }

    struct FakeTransport {
        response: std::result::Result<Vec<u8>, String>,
        captured: Arc<Mutex<Option<CapturedUpstreamRequest>>>,
    }

    #[async_trait]
    impl AsrTransport for FakeTransport {
        async fn transcribe(
            &self,
            endpoint: &Url,
            boundary: &str,
            body: Vec<u8>,
        ) -> std::result::Result<Vec<u8>, ()> {
            *self.captured.lock() = Some(CapturedUpstreamRequest {
                endpoint: endpoint.as_str().to_string(),
                boundary: boundary.to_string(),
                body,
            });
            self.response.clone().map_err(|_| ())
        }
    }

    fn fake_service(
        response: std::result::Result<Value, String>,
    ) -> (SelfHostedAsr, Arc<Mutex<Option<CapturedUpstreamRequest>>>) {
        let captured = Arc::new(Mutex::new(None));
        let transport = Arc::new(FakeTransport {
            response: response.map(|response| serde_json::to_vec(&response).unwrap()),
            captured: Arc::clone(&captured),
        });
        (
            SelfHostedAsr::for_test(
                "http://127.0.0.1:19000/transcribe",
                "家用臺灣華語",
                "tea-asr-1.1-mini",
                transport,
            ),
            captured,
        )
    }

    fn request(method: &str, uri: &str, body: Body, authorization: Option<&str>) -> Request<Body> {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(authorization) = authorization {
            request = request.header(header::AUTHORIZATION, format!("Bearer {authorization}"));
        }
        request.body(body).unwrap()
    }

    fn transcription_request(body: Vec<u8>, authorization: Option<&str>) -> Request<Body> {
        let mut request = Request::builder()
            .method("POST")
            .uri("/voice/asr/transcriptions")
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(ENGINE_ID_HEADER, SELF_HOSTED_ASR_ENGINE_ID)
            .header(CAPTURE_ID_HEADER, Uuid::new_v4().to_string())
            .header(SAMPLE_RATE_HEADER, SAMPLE_RATE_HZ.to_string())
            .header(CHANNELS_HEADER, CHANNELS.to_string());
        if let Some(authorization) = authorization {
            request = request.header(header::AUTHORIZATION, format!("Bearer {authorization}"));
        }
        request.body(Body::from(body)).unwrap()
    }

    async fn response_json(response: axum::response::Response) -> Value {
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }
}

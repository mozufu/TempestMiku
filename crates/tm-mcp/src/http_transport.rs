use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tm_egress::{EgressRuntime, HttpRequest, HttpResponse};
use tm_host::InvocationCtx;
use tokio::sync::Mutex as AsyncMutex;
use url::Url;

use crate::{MCP_PROTOCOL_VERSION, McpError, McpTransport, Result};

const CONTENT_TYPE_JSON: &str = "application/json";
const CONTENT_TYPE_SSE: &str = "text/event-stream";
const HEADER_ACCEPT: &str = "Accept";
const HEADER_CONTENT_TYPE: &str = "Content-Type";
const HEADER_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";
const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
const HEADER_LAST_EVENT_ID: &str = "Last-Event-ID";
const EXPOSED_SESSION_ID: &str = "mcp-session-id";

/// Trusted mapping from a local MCP alias to one P9 destination and optional opaque secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpHttpServerConfig {
    pub alias: String,
    pub url: String,
    pub destination_id: String,
    #[serde(default)]
    pub secret_id: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Transport-local parsing bounds in addition to the P9 byte/time budgets and tm-mcp RPC bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpHttpTransportBounds {
    pub max_response_bytes: usize,
    pub max_session_id_bytes: usize,
    pub max_sse_events: usize,
    pub max_sse_line_bytes: usize,
    pub max_sse_resumes: usize,
    pub max_sse_retry_ms: u64,
}

impl Default for McpHttpTransportBounds {
    fn default() -> Self {
        Self {
            max_response_bytes: 512 * 1024,
            max_session_id_bytes: 1024,
            max_sse_events: 128,
            max_sse_line_bytes: 64 * 1024,
            max_sse_resumes: 4,
            max_sse_retry_ms: 2_000,
        }
    }
}

/// MCP Streamable HTTP carried exclusively through the P9 egress and opaque-secret runtime.
///
/// The remote MCP session is host-owned and shared by the catalog plus its generated bindings.
/// Local authority is not shared: every exchange receives the current [`InvocationCtx`], so P9
/// rechecks the destination and secret grants and emits audits into the current durable sink.
#[derive(Clone)]
pub struct EgressMcpTransport {
    egress: EgressRuntime,
    servers: Arc<BTreeMap<String, McpHttpServerConfig>>,
    sessions: Arc<Mutex<BTreeMap<String, String>>>,
    session_locks: Arc<BTreeMap<String, Arc<AsyncMutex<()>>>>,
    bounds: McpHttpTransportBounds,
}

impl std::fmt::Debug for EgressMcpTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EgressMcpTransport")
            .field("servers", &self.servers.keys().collect::<Vec<_>>())
            .field("bounds", &self.bounds)
            .finish_non_exhaustive()
    }
}

impl EgressMcpTransport {
    pub fn new(
        egress: EgressRuntime,
        servers: Vec<McpHttpServerConfig>,
        bounds: McpHttpTransportBounds,
    ) -> Result<Self> {
        validate_bounds(bounds)?;
        let mut configured = BTreeMap::new();
        for server in servers {
            validate_server(&server)?;
            let alias = server.alias.clone();
            if configured.insert(alias.clone(), server).is_some() {
                return Err(McpError::InvalidConfig(format!(
                    "duplicate MCP HTTP server alias {alias}"
                )));
            }
        }
        if configured.is_empty() {
            return Err(McpError::InvalidConfig(
                "MCP HTTP transport requires at least one server".to_string(),
            ));
        }
        let session_locks = configured
            .keys()
            .map(|alias| (alias.clone(), Arc::new(AsyncMutex::new(()))))
            .collect();
        Ok(Self {
            egress,
            servers: Arc::new(configured),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            session_locks: Arc::new(session_locks),
            bounds,
        })
    }

    pub fn session_id(&self, server: &str) -> Option<String> {
        self.sessions.lock().get(server).cloned()
    }

    /// Terminate a stateful remote MCP session when the selected server supports HTTP DELETE.
    /// The local session id is cleared even when the remote reports that it is already gone.
    pub async fn terminate(&self, ctx: &InvocationCtx, server: &str) -> Result<()> {
        let config = self.server(server)?;
        let Some(session_id) = self.session_id(server) else {
            return Ok(());
        };
        let headers = self.post_headers(Some(&session_id));
        let response = self
            .execute(ctx, server, config, "DELETE", headers, None)
            .await;
        self.sessions.lock().remove(server);
        let response = response?;
        if !matches!(response.status, 200 | 202 | 204 | 404) {
            return Err(McpError::transport(
                server,
                format!("session termination returned HTTP {}", response.status),
            ));
        }
        Ok(())
    }

    fn server(&self, alias: &str) -> Result<&McpHttpServerConfig> {
        self.servers
            .get(alias)
            .ok_or_else(|| McpError::transport(alias, "server alias is not configured"))
    }

    fn post_headers(&self, session_id: Option<&str>) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::from([
            (
                HEADER_ACCEPT.to_string(),
                format!("{CONTENT_TYPE_JSON}, {CONTENT_TYPE_SSE}"),
            ),
            (
                HEADER_CONTENT_TYPE.to_string(),
                CONTENT_TYPE_JSON.to_string(),
            ),
            (
                HEADER_PROTOCOL_VERSION.to_string(),
                MCP_PROTOCOL_VERSION.to_string(),
            ),
        ]);
        if let Some(session_id) = session_id {
            headers.insert(HEADER_SESSION_ID.to_string(), session_id.to_string());
        }
        headers
    }

    fn get_headers(
        &self,
        session_id: Option<&str>,
        last_event_id: &str,
    ) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::from([
            (HEADER_ACCEPT.to_string(), CONTENT_TYPE_SSE.to_string()),
            (
                HEADER_PROTOCOL_VERSION.to_string(),
                MCP_PROTOCOL_VERSION.to_string(),
            ),
            (HEADER_LAST_EVENT_ID.to_string(), last_event_id.to_string()),
        ]);
        if let Some(session_id) = session_id {
            headers.insert(HEADER_SESSION_ID.to_string(), session_id.to_string());
        }
        headers
    }

    async fn execute(
        &self,
        ctx: &InvocationCtx,
        server: &str,
        config: &McpHttpServerConfig,
        method: &str,
        headers: BTreeMap<String, String>,
        body: Option<String>,
    ) -> Result<HttpResponse> {
        let auth = match config.secret_id.as_deref() {
            Some(secret_id) => Some(
                self.egress
                    .issue_secret_handle(ctx, secret_id)
                    .await
                    .map_err(|error| {
                        McpError::transport(
                            server,
                            format!("secret broker denied MCP request ({})", error.code()),
                        )
                    })?,
            ),
            None => None,
        };
        self.egress
            .execute_for_destination(
                ctx,
                &config.destination_id,
                HttpRequest {
                    method: method.to_string(),
                    url: config.url.clone(),
                    headers,
                    body,
                    auth,
                    timeout_ms: config.timeout_ms,
                },
            )
            .await
            .map_err(|error| {
                McpError::transport(
                    server,
                    format!("P9 egress denied MCP request ({})", error.code()),
                )
            })
    }

    async fn post(
        &self,
        ctx: &InvocationCtx,
        server: &str,
        encoded: &[u8],
        expects_response: bool,
    ) -> Result<Option<Vec<u8>>> {
        if encoded.len() > self.bounds.max_response_bytes {
            return Err(McpError::Bounds {
                target: format!("{server} MCP HTTP request"),
                limit: format!(
                    "{} bytes exceeds {}",
                    encoded.len(),
                    self.bounds.max_response_bytes
                ),
            });
        }
        let message: Value = serde_json::from_slice(encoded).map_err(|error| {
            McpError::transport(server, format!("outgoing JSON-RPC is malformed: {error}"))
        })?;
        let object = message
            .as_object()
            .ok_or_else(|| McpError::transport(server, "outgoing JSON-RPC is not an object"))?;
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(McpError::transport(
                server,
                "outgoing JSON-RPC revision is not 2.0",
            ));
        }
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| McpError::transport(server, "outgoing JSON-RPC has no method"))?;
        let request_id = object.get("id").cloned();
        if expects_response != request_id.is_some() {
            return Err(McpError::transport(
                server,
                "JSON-RPC request/notification shape does not match transport operation",
            ));
        }

        let config = self.server(server)?;
        let session_lock = self
            .session_locks
            .get(server)
            .cloned()
            .ok_or_else(|| McpError::transport(server, "server session lock is unavailable"))?;
        let _session_guard = session_lock.lock().await;
        let initializing = method == "initialize";
        if initializing {
            self.sessions.lock().remove(server);
        }
        let mut session_id = if initializing {
            None
        } else {
            self.session_id(server)
        };
        let body = String::from_utf8(encoded.to_vec())
            .map_err(|_| McpError::transport(server, "outgoing JSON-RPC is not UTF-8"))?;
        let mut response = self
            .execute(
                ctx,
                server,
                config,
                "POST",
                self.post_headers(session_id.as_deref()),
                Some(body.clone()),
            )
            .await?;

        if response.status == 404 && session_id.is_some() {
            self.sessions.lock().remove(server);
            self.reinitialize_locked(ctx, server, config).await?;
            if method == "tools/call" {
                return Err(McpError::transport(
                    server,
                    "remote MCP session expired after tools/call; the outcome is uncertain and the call was not retried",
                ));
            }
            session_id = self.session_id(server);
            response = self
                .execute(
                    ctx,
                    server,
                    config,
                    "POST",
                    self.post_headers(session_id.as_deref()),
                    Some(body),
                )
                .await?;
            if response.status == 404 && session_id.is_some() {
                self.sessions.lock().remove(server);
                return Err(McpError::transport(
                    server,
                    "remote MCP session expired immediately after reinitialization",
                ));
            }
        }
        if !expects_response {
            if response.status != 202 || !response.body.is_empty() {
                return Err(McpError::transport(
                    server,
                    format!(
                        "MCP notification expected empty HTTP 202, got {}",
                        response.status
                    ),
                ));
            }
            return Ok(None);
        }
        if response.status != 200 {
            return Err(McpError::transport(
                server,
                format!("MCP request returned HTTP {}", response.status),
            ));
        }
        let returned_session = response.headers.get(EXPOSED_SESSION_ID).cloned();
        if let Some(value) = returned_session.as_deref() {
            validate_session_id(server, value, self.bounds.max_session_id_bytes)?;
        }
        if !initializing
            && let Some(returned_session) = returned_session.as_deref()
            && session_id.as_deref() != Some(returned_session)
        {
            return Err(McpError::transport(
                server,
                "remote MCP session id changed outside initialization",
            ));
        }

        let request_id = request_id.expect("request shape checked above");
        let resume_session = returned_session.as_deref().or(session_id.as_deref());
        let encoded = self
            .parse_response_with_resume(ctx, server, config, response, &request_id, resume_session)
            .await?;
        if initializing {
            match returned_session {
                Some(session_id) => {
                    self.sessions.lock().insert(server.to_string(), session_id);
                }
                None => {
                    self.sessions.lock().remove(server);
                }
            }
        }
        Ok(Some(encoded))
    }

    async fn reinitialize_locked(
        &self,
        ctx: &InvocationCtx,
        server: &str,
        config: &McpHttpServerConfig,
    ) -> Result<()> {
        let request_id = Value::String("tm-session-reinitialize".to_string());
        let request = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "TempestMiku",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }))
        .map_err(|error| {
            McpError::transport(server, format!("reinitialize encoding failed: {error}"))
        })?;
        let response = self
            .execute(
                ctx,
                server,
                config,
                "POST",
                self.post_headers(None),
                Some(request),
            )
            .await?;
        if response.status != 200 {
            return Err(McpError::transport(
                server,
                format!("MCP reinitialize returned HTTP {}", response.status),
            ));
        }
        let returned_session = response.headers.get(EXPOSED_SESSION_ID).cloned();
        if let Some(value) = returned_session.as_deref() {
            validate_session_id(server, value, self.bounds.max_session_id_bytes)?;
        }
        let encoded = self
            .parse_response_with_resume(
                ctx,
                server,
                config,
                response,
                &request_id,
                returned_session.as_deref(),
            )
            .await?;
        let value: Value = serde_json::from_slice(&encoded).map_err(|error| {
            McpError::transport(
                server,
                format!("reinitialize response JSON failed: {error}"),
            )
        })?;
        let protocol = value
            .pointer("/result/protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                McpError::transport(server, "reinitialize omitted negotiated protocol version")
            })?;
        if protocol != MCP_PROTOCOL_VERSION {
            return Err(McpError::ProtocolVersion {
                server: server.to_string(),
                expected: MCP_PROTOCOL_VERSION.to_string(),
                actual: protocol.to_string(),
            });
        }
        match returned_session {
            Some(session_id) => {
                self.sessions.lock().insert(server.to_string(), session_id);
            }
            None => {
                self.sessions.lock().remove(server);
            }
        }
        let notification = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .map_err(|error| {
            McpError::transport(server, format!("initialized encoding failed: {error}"))
        })?;
        let session_id = self.session_id(server);
        let response = self
            .execute(
                ctx,
                server,
                config,
                "POST",
                self.post_headers(session_id.as_deref()),
                Some(notification),
            )
            .await?;
        if response.status != 202 || !response.body.is_empty() {
            return Err(McpError::transport(
                server,
                format!(
                    "reinitialize notification expected empty HTTP 202, got {}",
                    response.status
                ),
            ));
        }
        Ok(())
    }

    async fn parse_response_with_resume(
        &self,
        ctx: &InvocationCtx,
        server: &str,
        config: &McpHttpServerConfig,
        response: HttpResponse,
        request_id: &Value,
        session_id: Option<&str>,
    ) -> Result<Vec<u8>> {
        validate_response_bounds(server, &response, self.bounds)?;
        let content_type = response
            .content_type
            .as_deref()
            .and_then(media_type)
            .ok_or_else(|| McpError::transport(server, "MCP response omitted Content-Type"))?;
        if content_type.eq_ignore_ascii_case(CONTENT_TYPE_JSON) {
            return validate_json_response(server, &response.body, request_id);
        }
        if !content_type.eq_ignore_ascii_case(CONTENT_TYPE_SSE) {
            return Err(McpError::transport(
                server,
                format!("unsupported MCP response Content-Type {content_type}"),
            ));
        }

        let mut state = SseState::default();
        parse_sse_chunk(server, &response.body, request_id, self.bounds, &mut state)?;
        for _ in 0..=self.bounds.max_sse_resumes {
            if let Some(value) = state.matched.take() {
                return serde_json::to_vec(&value).map_err(|error| {
                    McpError::transport(server, format!("response encoding failed: {error}"))
                });
            }
            let cursor = state.last_event_id.clone().ok_or_else(|| {
                McpError::transport(
                    server,
                    "MCP SSE ended without response or resumable event id",
                )
            })?;
            if state.resume_count >= self.bounds.max_sse_resumes {
                break;
            }
            state.resume_count += 1;
            if state.retry_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(state.retry_ms)).await;
            }
            let response = self
                .execute(
                    ctx,
                    server,
                    config,
                    "GET",
                    self.get_headers(session_id, &cursor),
                    None,
                )
                .await?;
            if response.status == 404 && session_id.is_some() {
                self.sessions.lock().remove(server);
                return Err(McpError::transport(
                    server,
                    "MCP SSE session expired while the request outcome was uncertain",
                ));
            }
            if response.status != 200 {
                return Err(McpError::transport(
                    server,
                    format!("MCP SSE resume returned HTTP {}", response.status),
                ));
            }
            validate_response_bounds(server, &response, self.bounds)?;
            let resumed_type = response
                .content_type
                .as_deref()
                .and_then(media_type)
                .ok_or_else(|| {
                    McpError::transport(server, "MCP SSE resume omitted Content-Type")
                })?;
            if !resumed_type.eq_ignore_ascii_case(CONTENT_TYPE_SSE) {
                return Err(McpError::transport(
                    server,
                    "MCP SSE resume did not return text/event-stream",
                ));
            }
            parse_sse_chunk(server, &response.body, request_id, self.bounds, &mut state)?;
        }
        Err(McpError::Bounds {
            target: format!("{server} MCP SSE resumptions"),
            limit: format!("more than {} resumptions", self.bounds.max_sse_resumes),
        })
    }
}

#[async_trait]
impl McpTransport for EgressMcpTransport {
    async fn request(&self, ctx: &InvocationCtx, server: &str, request: &[u8]) -> Result<Vec<u8>> {
        self.post(ctx, server, request, true)
            .await?
            .ok_or_else(|| McpError::transport(server, "MCP request returned no response"))
    }

    async fn notify(&self, ctx: &InvocationCtx, server: &str, notification: &[u8]) -> Result<()> {
        self.post(ctx, server, notification, false).await?;
        Ok(())
    }
}

fn validate_bounds(bounds: McpHttpTransportBounds) -> Result<()> {
    if bounds.max_response_bytes == 0
        || bounds.max_session_id_bytes == 0
        || bounds.max_sse_events == 0
        || bounds.max_sse_line_bytes == 0
        || bounds.max_sse_resumes == 0
    {
        return Err(McpError::InvalidConfig(
            "MCP HTTP transport bounds must be positive".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_server(server: &McpHttpServerConfig) -> Result<()> {
    crate::validate::validate_server_alias(&server.alias)?;
    if server.destination_id.is_empty() || server.destination_id.len() > 128 {
        return Err(McpError::InvalidConfig(format!(
            "MCP server {} has an invalid destination id",
            server.alias
        )));
    }
    if server
        .secret_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 128)
    {
        return Err(McpError::InvalidConfig(format!(
            "MCP server {} has an invalid secret id",
            server.alias
        )));
    }
    let url = Url::parse(&server.url).map_err(|_| {
        McpError::InvalidConfig(format!("MCP server {} has an invalid URL", server.alias))
    })?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(McpError::InvalidConfig(format!(
            "MCP server {} URL is not an absolute destination URL",
            server.alias
        )));
    }
    Ok(())
}

fn validate_session_id(server: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.is_empty()
        || value.len() > max_bytes
        || !value.bytes().all(|byte| matches!(byte, 0x21..=0x7e))
    {
        return Err(McpError::transport(
            server,
            "remote MCP session id is invalid or exceeds bounds",
        ));
    }
    Ok(())
}

fn media_type(value: &str) -> Option<&str> {
    value.split(';').next().map(str::trim)
}

fn validate_json_response(server: &str, body: &str, request_id: &Value) -> Result<Vec<u8>> {
    let value: Value = serde_json::from_str(body).map_err(|error| {
        McpError::transport(server, format!("malformed JSON response: {error}"))
    })?;
    validate_response_value(server, &value, request_id)?;
    serde_json::to_vec(&value)
        .map_err(|error| McpError::transport(server, format!("response encoding failed: {error}")))
}

#[derive(Default)]
struct SseState {
    matched: Option<Value>,
    last_event_id: Option<String>,
    retry_ms: u64,
    event_count: usize,
    resume_count: usize,
}

fn parse_sse_chunk(
    server: &str,
    body: &str,
    request_id: &Value,
    bounds: McpHttpTransportBounds,
    state: &mut SseState,
) -> Result<()> {
    let mut data = Vec::<String>::new();
    let mut event_kind: Option<String> = None;
    let mut event_id: Option<String> = None;
    let mut retry_ms: Option<u64> = None;

    for raw_line in body.split('\n').chain(std::iter::once("")) {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.len() > bounds.max_sse_line_bytes {
            return Err(McpError::Bounds {
                target: format!("{server} MCP SSE line"),
                limit: format!("exceeds {} bytes", bounds.max_sse_line_bytes),
            });
        }
        if line.is_empty() {
            if data.is_empty() && event_id.is_none() && retry_ms.is_none() {
                event_kind = None;
                continue;
            }
            state.event_count = state.event_count.saturating_add(1);
            if state.event_count > bounds.max_sse_events {
                return Err(McpError::Bounds {
                    target: format!("{server} MCP SSE events"),
                    limit: format!("more than {} events", bounds.max_sse_events),
                });
            }
            if event_kind.as_deref().is_some_and(|kind| kind != "message") {
                return Err(McpError::transport(
                    server,
                    "unsupported MCP SSE event type",
                ));
            }
            if let Some(id) = event_id.take() {
                state.last_event_id = Some(id);
            }
            if let Some(retry_ms) = retry_ms.take() {
                state.retry_ms = retry_ms;
            }
            let encoded = data.join("\n");
            if encoded.is_empty() {
                data.clear();
                event_kind = None;
                continue;
            }
            if encoded.len() > bounds.max_response_bytes {
                return Err(McpError::Bounds {
                    target: format!("{server} MCP SSE data"),
                    limit: format!("exceeds {} bytes", bounds.max_response_bytes),
                });
            }
            let value: Value = serde_json::from_str(&encoded).map_err(|error| {
                McpError::transport(server, format!("malformed MCP SSE JSON: {error}"))
            })?;
            let object = value.as_object().ok_or_else(|| {
                McpError::transport(server, "MCP SSE data is not a JSON-RPC object")
            })?;
            if object.contains_key("method") {
                if object.contains_key("id")
                    || object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
                {
                    return Err(McpError::transport(
                        server,
                        "unnegotiated server JSON-RPC request over SSE is unsupported",
                    ));
                }
                // Notifications are bounded and intentionally ignored. The immutable startup
                // catalog does not accept remote list-changed authority during a user turn.
            } else if object.get("id") == Some(request_id) {
                validate_response_value(server, &value, request_id)?;
                if state.matched.replace(value).is_some() {
                    return Err(McpError::transport(
                        server,
                        "MCP SSE returned multiple responses for one request",
                    ));
                }
            } else {
                return Err(McpError::transport(
                    server,
                    "MCP SSE response id did not match the request",
                ));
            }
            data.clear();
            event_kind = None;
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => data.push(value.to_string()),
            "event" if event_kind.is_none() => event_kind = Some(value.to_string()),
            "id" if event_id.is_none()
                && !value.is_empty()
                && value.len() <= bounds.max_session_id_bytes
                && !value
                    .bytes()
                    .any(|byte| matches!(byte, b'\r' | b'\n' | b'\0')) =>
            {
                event_id = Some(value.to_string());
            }
            "retry" => {
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| McpError::transport(server, "MCP SSE retry is not an integer"))?;
                if parsed > bounds.max_sse_retry_ms {
                    return Err(McpError::Bounds {
                        target: format!("{server} MCP SSE retry"),
                        limit: format!("{parsed}ms exceeds {}ms", bounds.max_sse_retry_ms),
                    });
                }
                retry_ms = Some(parsed);
            }
            _ => {
                return Err(McpError::transport(
                    server,
                    "malformed or unsupported MCP SSE field",
                ));
            }
        }
    }
    Ok(())
}

fn validate_response_bounds(
    server: &str,
    response: &HttpResponse,
    bounds: McpHttpTransportBounds,
) -> Result<()> {
    if response.response_bytes > bounds.max_response_bytes
        || response.body.len() > bounds.max_response_bytes
    {
        return Err(McpError::Bounds {
            target: format!("{server} MCP HTTP response"),
            limit: format!("exceeds {} bytes", bounds.max_response_bytes),
        });
    }
    Ok(())
}

fn validate_response_value(server: &str, value: &Value, request_id: &Value) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| McpError::transport(server, "MCP response is not an object"))?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object.get("id") != Some(request_id)
    {
        return Err(McpError::transport(
            server,
            "MCP response has invalid jsonrpc or id",
        ));
    }
    if object.contains_key("result") == object.contains_key("error") {
        return Err(McpError::transport(
            server,
            "MCP response must contain exactly one result or error",
        ));
    }
    Ok(())
}

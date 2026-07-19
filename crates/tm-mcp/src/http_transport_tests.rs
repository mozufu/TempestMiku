use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, Response, StatusCode, header},
    routing::post,
};
use serde_json::{Value, json};
use serial_test::serial;
use tm_egress::{DnsResolver, EgressRuntime};
use tm_host::{
    ApprovalDecision, ApprovalPolicy, CapabilityGrants, EgressConfig, EgressDestinationConfig,
    EgressSecretConfig, EgressSecretInjection, EgressSessionLimits, HostEventSink, HostRegistry,
    InvocationCtx, ResourceRegistry,
};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{
    EgressMcpTransport, MCP_PROTOCOL_VERSION, McpBounds, McpCatalogContext, McpCatalogManager,
    McpError, McpHttpServerConfig, McpHttpTransportBounds, McpObjectAllowlist, McpServerSpec,
    McpToolPolicy,
};

const TEST_SECRET_ENV: &str = "TM_MCP_HTTP_TRANSPORT_TEST_SECRET";
const TEST_SECRET: &str = "mcp-test-secret-value";

#[derive(Debug, Clone, Copy)]
enum ReplyMode {
    Json,
    Sse,
    ResumableSse,
    ExpireOnce,
    MalformedSse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestRecord {
    method: String,
    protocol_version: Option<String>,
    session_id: Option<String>,
    authenticated: bool,
    last_event_id: Option<String>,
}

#[derive(Clone)]
struct TestState {
    mode: ReplyMode,
    session_id: String,
    records: Arc<Mutex<Vec<RequestRecord>>>,
    expired_once: Arc<AtomicBool>,
    initialize_count: Arc<AtomicUsize>,
    pending_sse: Arc<Mutex<Option<(Value, Value)>>>,
}

struct TestServer {
    address: SocketAddr,
    records: Arc<Mutex<Vec<RequestRecord>>>,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_server(mode: ReplyMode, session_id: impl Into<String>) -> TestServer {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind MCP test server");
    let address = listener.local_addr().expect("MCP test server address");
    let records = Arc::new(Mutex::new(Vec::new()));
    let state = TestState {
        mode,
        session_id: session_id.into(),
        records: Arc::clone(&records),
        expired_once: Arc::new(AtomicBool::new(false)),
        initialize_count: Arc::new(AtomicUsize::new(0)),
        pending_sse: Arc::new(Mutex::new(None)),
    };
    let app = Router::new()
        .route("/mcp", post(mcp_post).get(mcp_get).delete(mcp_delete))
        .with_state(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("MCP test server should remain available");
    });
    TestServer {
        address,
        records,
        task,
    }
}

async fn mcp_post(
    State(state): State<TestState>,
    headers: HeaderMap,
    body: String,
) -> Response<Body> {
    let value: Value = serde_json::from_str(&body).expect("test client sends JSON");
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .expect("JSON-RPC method")
        .to_string();
    record_request(&state, &headers, method.clone());
    if matches!(state.mode, ReplyMode::ExpireOnce)
        && method == "tools/call"
        && !state.expired_once.swap(true, Ordering::SeqCst)
    {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .expect("expired session response");
    }
    if value.get("id").is_none() {
        return Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Body::empty())
            .expect("notification response");
    }
    let id = value.get("id").cloned().expect("request id");
    let result = match method.as_str() {
        "initialize" => json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "loopback-mcp", "version": "1" }
        }),
        "tools/list" => json!({
            "tools": [{
                "name": "lookup",
                "inputSchema": { "type": "object" }
            }]
        }),
        "tools/call" => json!({
            "content": [{ "type": "text", "text": "loopback result" }]
        }),
        other => panic!("unexpected test MCP method {other}"),
    };
    let rpc = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let (content_type, body) = match (state.mode, method.as_str()) {
        (ReplyMode::ResumableSse, "tools/list") => {
            *state.pending_sse.lock().expect("pending SSE lock") = Some((id, rpc));
            (
                "text/event-stream",
                "id: cursor-1\nretry: 0\ndata:\n\n".to_string(),
            )
        }
        (ReplyMode::Sse, "tools/list" | "tools/call") => (
            "text/event-stream",
            format!("event: message\ndata: {rpc}\n\n"),
        ),
        (ReplyMode::MalformedSse, "tools/list") => (
            "text/event-stream",
            "event: message\ndata: not-json\n\n".to_string(),
        ),
        _ => ("application/json", rpc.to_string()),
    };
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type);
    if method == "initialize" {
        let session_id = if matches!(state.mode, ReplyMode::ExpireOnce) {
            let generation = state.initialize_count.fetch_add(1, Ordering::SeqCst) + 1;
            format!("{}-{generation}", state.session_id)
        } else {
            state.session_id.clone()
        };
        response = response.header("Mcp-Session-Id", session_id);
    }
    response.body(Body::from(body)).expect("MCP response")
}

async fn mcp_get(State(state): State<TestState>, headers: HeaderMap) -> Response<Body> {
    record_request(&state, &headers, "GET".to_string());
    let pending = state.pending_sse.lock().expect("pending SSE lock").take();
    let Some((_id, rpc)) = pending else {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::empty())
            .expect("missing pending SSE response");
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!("id: cursor-2\ndata: {rpc}\n\n")))
        .expect("resumed SSE response")
}

async fn mcp_delete(State(state): State<TestState>, headers: HeaderMap) -> Response<Body> {
    record_request(&state, &headers, "DELETE".to_string());
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .expect("delete response")
}

fn record_request(state: &TestState, headers: &HeaderMap, method: String) {
    let value = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    state
        .records
        .lock()
        .expect("record lock")
        .push(RequestRecord {
            method,
            protocol_version: value("MCP-Protocol-Version"),
            session_id: value("Mcp-Session-Id"),
            authenticated: value("Authorization").as_deref()
                == Some("Bearer mcp-test-secret-value"),
            last_event_id: value("Last-Event-ID"),
        });
}

#[derive(Clone)]
struct StaticResolver(IpAddr);

#[async_trait]
impl DnsResolver for StaticResolver {
    async fn resolve(&self, _host: &str, port: u16) -> tm_egress::Result<Vec<SocketAddr>> {
        Ok(vec![SocketAddr::new(self.0, port)])
    }
}

#[derive(Default)]
struct CaptureEvents(Mutex<Vec<(String, Value)>>);

#[async_trait]
impl HostEventSink for CaptureEvents {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        self.0
            .lock()
            .expect("event lock")
            .push((event_type.to_string(), payload_json));
        Ok(())
    }
}

impl CaptureEvents {
    fn snapshot(&self) -> Vec<(String, Value)> {
        self.0.lock().expect("event lock").clone()
    }
}

#[derive(Default)]
struct MutationEvents(CaptureEvents);

#[async_trait]
impl HostEventSink for MutationEvents {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        self.0.emit(event_type, payload_json).await
    }

    fn effect_scope_id(&self) -> Option<String> {
        Some("http-transport-mutation-scope".to_string())
    }
}

struct Approve;

#[async_trait]
impl ApprovalPolicy for Approve {
    async fn request(
        &self,
        _action: &str,
        _timeout: Duration,
    ) -> tm_host::Result<ApprovalDecision> {
        Ok(ApprovalDecision::Approved)
    }
}

struct EnvGuard;

impl EnvGuard {
    fn install() -> Self {
        // SAFETY: these tests are serialized and this unique variable is consumed only by the
        // test-owned egress runtime.
        unsafe { std::env::set_var(TEST_SECRET_ENV, TEST_SECRET) };
        Self
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: see `install`; the serialized test is the sole owner of this variable.
        unsafe { std::env::remove_var(TEST_SECRET_ENV) };
    }
}

fn destination(port: u16, response_cap: usize) -> EgressDestinationConfig {
    EgressDestinationConfig {
        id: "mcp-docs".to_string(),
        version: 1,
        scheme: "http".to_string(),
        host: "mcp.test".to_string(),
        port,
        path_prefixes: vec!["/mcp".to_string()],
        methods: BTreeSet::from(["POST".to_string(), "GET".to_string(), "DELETE".to_string()]),
        redirect_to: BTreeSet::new(),
        max_redirects: 0,
        allow_private_ips: true,
        request_timeout_ms: 1_000,
        connect_timeout_ms: 500,
        max_request_bytes: 32 * 1024,
        max_response_bytes: response_cap,
        max_requests_per_session: 16,
        max_request_bytes_per_session: 256 * 1024,
        max_response_bytes_per_session: response_cap.saturating_mul(16),
        max_time_ms_per_session: 10_000,
        allowed_request_headers: BTreeSet::from([
            "accept".to_string(),
            "content-type".to_string(),
            "last-event-id".to_string(),
            "mcp-protocol-version".to_string(),
            "mcp-session-id".to_string(),
        ]),
    }
}

fn egress(server: &TestServer, response_cap: usize) -> EgressRuntime {
    EgressRuntime::with_test_http_resolver(
        EgressConfig {
            enabled: true,
            session_limits: EgressSessionLimits {
                max_requests: 32,
                max_request_bytes: 512 * 1024,
                max_response_bytes: response_cap.saturating_mul(32),
                max_time_ms: 20_000,
            },
            destinations: vec![destination(server.address.port(), response_cap)],
            secrets: vec![EgressSecretConfig {
                id: "mcp-token".to_string(),
                version: 1,
                env: TEST_SECRET_ENV.to_string(),
                destinations: BTreeSet::from(["mcp-docs".to_string()]),
                injection: EgressSecretInjection::AuthorizationBearer,
            }],
        },
        Arc::new(StaticResolver(server.address.ip())),
    )
    .expect("test egress")
}

fn transport(server: &TestServer, bounds: McpHttpTransportBounds) -> Arc<EgressMcpTransport> {
    Arc::new(
        EgressMcpTransport::new(
            egress(server, bounds.max_response_bytes),
            vec![McpHttpServerConfig {
                alias: "docs".to_string(),
                url: format!("http://mcp.test:{}/mcp", server.address.port()),
                destination_id: "mcp-docs".to_string(),
                secret_id: Some("mcp-token".to_string()),
                timeout_ms: Some(1_000),
            }],
            bounds,
        )
        .expect("MCP transport"),
    )
}

fn grants(include_mcp: bool) -> CapabilityGrants {
    let grants = CapabilityGrants::default()
        .allow_many(["egress.destination:mcp-docs", "secrets.use:mcp-token"]);
    if include_mcp {
        grants.allow("mcp.docs.lookup")
    } else {
        grants
    }
}

fn manager(
    transport: Arc<EgressMcpTransport>,
    catalog_events: Arc<CaptureEvents>,
) -> McpCatalogManager {
    let catalog_context = McpCatalogContext::new(
        InvocationCtx::new(grants(false))
            .with_session_id("mcp-catalog-host")
            .with_event_sink(catalog_events),
    )
    .expect("host-owned catalog context");
    McpCatalogManager::new(transport, McpBounds::default(), catalog_context).expect("manager")
}

fn spec() -> McpServerSpec {
    McpServerSpec {
        alias: "docs".to_string(),
        allow: McpObjectAllowlist {
            tools: BTreeMap::from([("lookup".to_string(), McpToolPolicy::read_only())]),
            resources: BTreeSet::new(),
            prompts: BTreeSet::new(),
        },
    }
}

fn mutation_spec() -> McpServerSpec {
    McpServerSpec {
        alias: "docs".to_string(),
        allow: McpObjectAllowlist {
            tools: BTreeMap::from([("lookup".to_string(), McpToolPolicy::mutation())]),
            resources: BTreeSet::new(),
            prompts: BTreeSet::new(),
        },
    }
}

#[tokio::test]
#[serial]
async fn json_session_lifecycle_uses_current_context_grants_and_audits() {
    let _env = EnvGuard::install();
    let server = spawn_server(ReplyMode::Json, "remote-session-1").await;
    let transport = transport(&server, McpHttpTransportBounds::default());
    let catalog_events = Arc::new(CaptureEvents::default());
    let manager = manager(Arc::clone(&transport), Arc::clone(&catalog_events));
    manager.reload(&[spec()]).await.expect("catalog reload");
    assert_eq!(
        transport.session_id("docs").as_deref(),
        Some("remote-session-1")
    );

    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    manager
        .bindings()
        .expect("bindings")
        .register_into(&mut host, &mut resources)
        .expect("register bindings");

    let denied_events = Arc::new(CaptureEvents::default());
    let denied = InvocationCtx::new(
        CapabilityGrants::default()
            .allow("mcp.docs.lookup")
            .allow("secrets.use:mcp-token"),
    )
    .with_session_id("user-session")
    .with_event_sink(denied_events.clone());
    assert!(
        host.invoke("mcp.docs.lookup", json!({}), &denied)
            .await
            .is_err()
    );
    assert!(
        denied_events
            .snapshot()
            .iter()
            .any(|(kind, _)| kind == "egress_denied")
    );

    let invocation_events = Arc::new(CaptureEvents::default());
    let invocation = InvocationCtx::new(grants(true))
        .with_session_id("user-session")
        .with_event_sink(invocation_events.clone());
    let result = host
        .invoke("mcp.docs.lookup", json!({}), &invocation)
        .await
        .expect("context-authorized MCP call");
    assert_eq!(
        result.get("trust").and_then(Value::as_str),
        Some("untrusted")
    );
    assert!(
        invocation_events
            .snapshot()
            .iter()
            .any(|(kind, _)| kind == "egress_completed")
    );

    transport
        .terminate(&invocation, "docs")
        .await
        .expect("terminate remote session");
    assert_eq!(transport.session_id("docs"), None);

    let records = server.records.lock().expect("record lock").clone();
    assert_eq!(records[0].method, "initialize");
    assert_eq!(records[0].session_id, None);
    assert!(records.iter().skip(1).all(|record| {
        record.protocol_version.as_deref() == Some(MCP_PROTOCOL_VERSION)
            && record.authenticated
            && record.session_id.as_deref() == Some("remote-session-1")
    }));
    assert_eq!(
        records.last().map(|record| record.method.as_str()),
        Some("DELETE")
    );

    let audits = serde_json::to_string(&(
        catalog_events.snapshot(),
        denied_events.snapshot(),
        invocation_events.snapshot(),
    ))
    .expect("audit JSON");
    assert!(!audits.contains(TEST_SECRET));
}

#[tokio::test]
#[serial]
async fn finite_sse_responses_are_parsed_with_the_same_session() {
    let _env = EnvGuard::install();
    let server = spawn_server(ReplyMode::Sse, "remote-sse-session").await;
    let transport = transport(&server, McpHttpTransportBounds::default());
    let manager = manager(transport, Arc::new(CaptureEvents::default()));
    manager.reload(&[spec()]).await.expect("SSE catalog reload");

    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    manager
        .bindings()
        .expect("bindings")
        .register_into(&mut host, &mut resources)
        .expect("register bindings");
    let value = host
        .invoke(
            "mcp.docs.lookup",
            json!({}),
            &InvocationCtx::new(grants(true)).with_session_id("sse-user"),
        )
        .await
        .expect("SSE tool result");
    assert!(
        value
            .pointer("/data/content/0/text")
            .is_some_and(|value| value == "loopback result")
    );
}

#[tokio::test]
#[serial]
async fn primed_sse_resumes_with_bounded_retry_and_last_event_id() {
    let _env = EnvGuard::install();
    let server = spawn_server(ReplyMode::ResumableSse, "remote-resume-session").await;
    let manager = manager(
        transport(&server, McpHttpTransportBounds::default()),
        Arc::new(CaptureEvents::default()),
    );
    manager.reload(&[spec()]).await.expect("resumed SSE reload");

    let records = server.records.lock().expect("record lock").clone();
    let get = records
        .iter()
        .find(|record| record.method == "GET")
        .expect("SSE resume GET");
    assert_eq!(get.last_event_id.as_deref(), Some("cursor-1"));
    assert_eq!(get.session_id.as_deref(), Some("remote-resume-session"));
    assert_eq!(get.protocol_version.as_deref(), Some(MCP_PROTOCOL_VERSION));
}

#[tokio::test]
#[serial]
async fn expired_session_reinitializes_but_never_retries_tools_call() {
    let _env = EnvGuard::install();
    let server = spawn_server(ReplyMode::ExpireOnce, "remote-session").await;
    let transport = transport(&server, McpHttpTransportBounds::default());
    let manager = manager(Arc::clone(&transport), Arc::new(CaptureEvents::default()));
    manager.reload(&[spec()]).await.expect("initial catalog");

    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    manager
        .bindings()
        .expect("bindings")
        .register_into(&mut host, &mut resources)
        .expect("register bindings");
    let error = host
        .invoke(
            "mcp.docs.lookup",
            json!({}),
            &InvocationCtx::new(grants(true)).with_session_id("user-session"),
        )
        .await
        .expect_err("an expired tools/call outcome must stay uncertain");
    assert!(
        error.to_string().contains("outcome is uncertain"),
        "{error}"
    );

    assert_eq!(
        transport.session_id("docs").as_deref(),
        Some("remote-session-2")
    );
    let records = server.records.lock().expect("record lock").clone();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.method == "initialize")
            .count(),
        2
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.method == "tools/call")
            .count(),
        1
    );
    assert_eq!(
        records
            .iter()
            .rev()
            .find(|record| record.method == "tools/call")
            .and_then(|record| record.session_id.as_deref()),
        Some("remote-session-1")
    );
}

#[tokio::test]
#[serial]
async fn expired_mutation_is_persisted_uncertain_and_replays_without_resend() {
    let _env = EnvGuard::install();
    let server = spawn_server(ReplyMode::ExpireOnce, "remote-mutation-session").await;
    let transport = transport(&server, McpHttpTransportBounds::default());
    let manager = manager(Arc::clone(&transport), Arc::new(CaptureEvents::default()));
    manager
        .reload(&[mutation_spec()])
        .await
        .expect("initial mutation catalog");

    let bindings = manager.bindings().expect("bindings");
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    bindings
        .register_into(&mut host, &mut resources)
        .expect("register mutation binding");

    let invocation =
        InvocationCtx::with_approvals(grants(true), Arc::new(Approve), Duration::from_secs(1))
            .with_session_id("user-mutation-session")
            .with_event_sink(Arc::new(MutationEvents::default()));
    let first = host
        .invoke(
            "mcp.docs.lookup",
            json!({ "operation": "once" }),
            &invocation,
        )
        .await
        .expect_err("expired mutation outcome must stay uncertain");
    assert!(
        first.to_string().contains("outcome is uncertain"),
        "{first}"
    );
    let replay = host
        .invoke(
            "mcp.docs.lookup",
            json!({ "operation": "once" }),
            &invocation,
        )
        .await
        .expect("uncertain mutation must return a no-resend replay receipt");
    assert_eq!(
        replay.pointer("/data/reexecuted").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        replay.pointer("/data/status").and_then(Value::as_str),
        Some("uncertain")
    );

    let records = server.records.lock().expect("record lock").clone();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.method == "initialize")
            .count(),
        2
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.method == "tools/call")
            .count(),
        1
    );
}

#[tokio::test]
#[serial]
async fn malformed_sse_and_oversized_session_id_fail_closed_without_catalog_activation() {
    let _env = EnvGuard::install();
    let server = spawn_server(ReplyMode::MalformedSse, "valid-session").await;
    let malformed_manager = manager(
        transport(&server, McpHttpTransportBounds::default()),
        Arc::new(CaptureEvents::default()),
    );
    assert!(matches!(
        malformed_manager.reload(&[spec()]).await,
        Err(McpError::Transport { .. })
    ));
    assert_eq!(malformed_manager.catalog().generation, 0);

    let server = spawn_server(ReplyMode::Json, "session-id-is-too-long").await;
    let bounds = McpHttpTransportBounds {
        max_session_id_bytes: 8,
        ..McpHttpTransportBounds::default()
    };
    let manager = manager(
        transport(&server, bounds),
        Arc::new(CaptureEvents::default()),
    );
    assert!(matches!(
        manager.reload(&[spec()]).await,
        Err(McpError::Transport { .. })
    ));
    assert_eq!(manager.catalog().generation, 0);
}

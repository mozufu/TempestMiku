use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Router,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use serde_json::{Value, json};
use serial_test::serial;
use tm_host::{
    ApprovalDecision, ApprovalPolicy, CapabilityGrants, EgressConfig, EgressDestinationConfig,
    EgressSecretConfig, EgressSecretInjection, EgressSessionLimits, HostError, HostEventSink,
    HostFn, InvocationCtx,
};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{
    DnsResolver, EgressBudgetRequest, EgressBudgetReservation, EgressError, EgressMutationClaim,
    EgressMutationIntent, EgressMutationRecord, EgressMutationStatus, EgressRuntime,
    EgressStateStore, HttpRequest, HttpRequestFn, MAX_SECRET_HANDLES_PER_SESSION,
    MutationExecution, VolatileEgressStateStore, runtime::EgressRuntimeOptions,
    validate_resolved_addresses,
};

#[derive(Default)]
struct RecordingEvents {
    values: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl HostEventSink for RecordingEvents {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        self.values
            .lock()
            .expect("event lock poisoned")
            .push((event_type.to_string(), payload_json));
        Ok(())
    }
}

impl RecordingEvents {
    fn snapshot(&self) -> Vec<(String, Value)> {
        self.values.lock().expect("event lock poisoned").clone()
    }
}

struct DurableRecordingEvents {
    scope_id: String,
    values: Mutex<Vec<(String, Value)>>,
}

impl DurableRecordingEvents {
    fn new(scope_id: impl Into<String>) -> Self {
        Self {
            scope_id: scope_id.into(),
            values: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl HostEventSink for DurableRecordingEvents {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        self.values
            .lock()
            .expect("event lock poisoned")
            .push((event_type.to_string(), payload_json));
        Ok(())
    }

    fn effect_scope_id(&self) -> Option<String> {
        Some(self.scope_id.clone())
    }
}

struct TimelineEvents {
    scope_id: String,
    timeline: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl HostEventSink for TimelineEvents {
    async fn emit(&self, event_type: &str, _payload_json: Value) -> tm_host::Result<()> {
        self.timeline
            .lock()
            .expect("timeline lock poisoned")
            .push(event_type.to_string());
        Ok(())
    }

    fn effect_scope_id(&self) -> Option<String> {
        Some(self.scope_id.clone())
    }
}

struct TimelineStateStore {
    inner: VolatileEgressStateStore,
    timeline: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl EgressStateStore for TimelineStateStore {
    async fn begin_mutation(
        &self,
        intent: EgressMutationIntent,
    ) -> crate::Result<EgressMutationClaim> {
        self.inner.begin_mutation(intent).await
    }

    async fn finish_mutation(
        &self,
        effect_id: &str,
        status: EgressMutationStatus,
        result_digest: Option<&str>,
        result_bytes: Option<usize>,
        error_code: Option<&str>,
        error_digest: Option<&str>,
    ) -> crate::Result<EgressMutationRecord> {
        let record = self
            .inner
            .finish_mutation(
                effect_id,
                status,
                result_digest,
                result_bytes,
                error_code,
                error_digest,
            )
            .await?;
        self.timeline
            .lock()
            .expect("timeline lock poisoned")
            .push("effect_finished".into());
        Ok(record)
    }

    async fn reserve_budget(
        &self,
        request: EgressBudgetRequest,
    ) -> crate::Result<EgressBudgetReservation> {
        self.inner.reserve_budget(request).await
    }

    async fn settle_budget(
        &self,
        reservation: EgressBudgetReservation,
        response_bytes: u64,
        elapsed_ms: u64,
    ) -> crate::Result<()> {
        self.inner
            .settle_budget(reservation, response_bytes, elapsed_ms)
            .await
    }

    async fn clear_session(&self, session_id: &str) -> crate::Result<()> {
        self.inner.clear_session(session_id).await
    }
}

struct AlwaysApprove;

#[async_trait]
impl ApprovalPolicy for AlwaysApprove {
    async fn request(
        &self,
        _action: &str,
        _timeout: Duration,
    ) -> tm_host::Result<ApprovalDecision> {
        Ok(ApprovalDecision::Approved)
    }
}

#[derive(Clone)]
struct StaticResolver {
    ip: IpAddr,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl DnsResolver for StaticResolver {
    async fn resolve(&self, _host: &str, port: u16) -> crate::Result<Vec<SocketAddr>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![SocketAddr::new(self.ip, port)])
    }
}

struct TestServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_server() -> TestServer {
    async fn echo_authorization(headers: HeaderMap) -> String {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("missing")
            .to_string()
    }

    async fn slow() -> &'static str {
        tokio::time::sleep(Duration::from_millis(100)).await;
        "late"
    }

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("test server address");
    let other = format!("http://other.test:{}/ok", address.port());
    let app = Router::new()
        .route("/ok", get(|| async { "ok" }).post(|| async { "posted" }))
        .route(
            "/redirect-same",
            get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/ok")]) }),
        )
        .route(
            "/redirect-long",
            get(|| async {
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, format!("/ok?q={}", "x".repeat(160)))],
                )
            }),
        )
        .route(
            "/redirect-other",
            get(move || {
                let other = other.clone();
                async move { (StatusCode::FOUND, [(header::LOCATION, other)]) }
            }),
        )
        .route("/slow", get(slow))
        .route("/large", get(|| async { "x".repeat(256) }))
        .route(
            "/session-header",
            get(|| async { ([("Mcp-Session-Id", "host-only-session")], "ok") }),
        )
        .route("/echo-auth", get(echo_authorization))
        .fallback(|| async { StatusCode::NOT_FOUND.into_response() });
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("test server should remain available");
    });
    TestServer { address, task }
}

fn destination(id: &str, host: &str, port: u16) -> EgressDestinationConfig {
    EgressDestinationConfig {
        id: id.into(),
        version: 1,
        scheme: "http".into(),
        host: host.into(),
        port,
        path_prefixes: vec!["/".into()],
        methods: BTreeSet::from(["GET".into(), "POST".into()]),
        redirect_to: BTreeSet::new(),
        max_redirects: 2,
        allow_private_ips: true,
        request_timeout_ms: 500,
        connect_timeout_ms: 200,
        max_request_bytes: 4 * 1024,
        max_response_bytes: 4 * 1024,
        max_requests_per_session: 8,
        max_request_bytes_per_session: 32 * 1024,
        max_response_bytes_per_session: 32 * 1024,
        max_time_ms_per_session: 4_000,
        allowed_request_headers: BTreeSet::from(["x-test".into()]),
    }
}

fn config(destinations: Vec<EgressDestinationConfig>) -> EgressConfig {
    EgressConfig {
        enabled: true,
        session_limits: EgressSessionLimits {
            max_requests: 16,
            max_request_bytes: 64 * 1024,
            max_response_bytes: 64 * 1024,
            max_time_ms: 8_000,
        },
        destinations,
        secrets: vec![],
    }
}

fn runtime(config: EgressConfig, server: &TestServer) -> (EgressRuntime, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let resolver = Arc::new(StaticResolver {
        ip: server.address.ip(),
        calls: calls.clone(),
    });
    let runtime = EgressRuntime::with_resolver_and_options(
        config,
        resolver,
        EgressRuntimeOptions { allow_http: true },
    )
    .expect("valid test egress config");
    (runtime, calls)
}

fn context(
    grants: impl IntoIterator<Item = &'static str>,
) -> (InvocationCtx, Arc<RecordingEvents>) {
    let events = Arc::new(RecordingEvents::default());
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow_many(grants))
        .with_session_id("session-a")
        .with_event_sink(events.clone());
    (ctx, events)
}

fn request(method: &str, url: String) -> HttpRequest {
    HttpRequest {
        method: method.into(),
        url,
        headers: BTreeMap::new(),
        body: None,
        auth: None,
        timeout_ms: None,
    }
}

#[tokio::test]
async fn disabled_is_default_and_denial_is_audited() {
    let runtime = EgressRuntime::new(EgressConfig::default()).expect("disabled config is valid");
    let (ctx, events) = context([]);
    let error = runtime
        .execute(&ctx, request("GET", "https://example.com/".into()))
        .await
        .expect_err("default egress must fail closed");

    assert_eq!(error, EgressError::Disabled);
    assert_eq!(events.snapshot()[0].0, "egress_denied");
}

#[tokio::test]
async fn opt_in_live_https_canary_uses_the_production_resolver_and_audit_path() {
    if std::env::var_os("TM_EGRESS_LIVE_TESTS").is_none() {
        return;
    }

    let destination = EgressDestinationConfig {
        id: "iana_example".into(),
        version: 1,
        scheme: "https".into(),
        host: "example.com".into(),
        port: 443,
        path_prefixes: vec!["/".into()],
        methods: BTreeSet::from(["GET".into()]),
        redirect_to: BTreeSet::new(),
        max_redirects: 0,
        allow_private_ips: false,
        request_timeout_ms: 8_000,
        connect_timeout_ms: 4_000,
        max_request_bytes: 8 * 1024,
        max_response_bytes: 64 * 1024,
        max_requests_per_session: 1,
        max_request_bytes_per_session: 8 * 1024,
        max_response_bytes_per_session: 64 * 1024,
        max_time_ms_per_session: 8_000,
        allowed_request_headers: BTreeSet::new(),
    };
    let runtime = EgressRuntime::new(config(vec![destination]))
        .expect("live canary uses a valid production HTTPS policy");
    let (ctx, events) = context(["egress.destination:iana_example"]);
    let response = runtime
        .execute(&ctx, request("GET", "https://example.com/".into()))
        .await
        .expect("explicit live HTTPS canary");

    assert_eq!(response.status, 200);
    assert_eq!(response.destination_id, "iana_example");
    assert!(response.body.contains("Example Domain"));
    let events = events.snapshot();
    assert!(events.iter().any(|(kind, _)| kind == "egress_started"));
    assert!(events.iter().any(|(kind, _)| kind == "egress_completed"));
}

#[test]
fn private_opt_in_never_allows_link_local_or_metadata() {
    for ip in [
        Ipv4Addr::new(169, 254, 1, 1),
        Ipv4Addr::new(169, 254, 169, 254),
        Ipv4Addr::new(100, 100, 100, 200),
        Ipv4Addr::new(168, 63, 129, 16),
    ] {
        assert!(matches!(
            validate_resolved_addresses(&[SocketAddr::new(ip.into(), 443)], 443, true),
            Err(EgressError::Denied(_))
        ));
    }
    for ip in ["fe80::1", "fd00:ec2::254"] {
        assert!(matches!(
            validate_resolved_addresses(
                &[SocketAddr::new(ip.parse().expect("valid test IPv6"), 443)],
                443,
                true
            ),
            Err(EgressError::Denied(_))
        ));
    }
    assert!(
        validate_resolved_addresses(
            &[SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 443)],
            443,
            true
        )
        .is_ok()
    );
    assert!(
        validate_resolved_addresses(
            &[SocketAddr::new(Ipv4Addr::new(10, 0, 0, 1).into(), 443)],
            443,
            false
        )
        .is_err()
    );
}

#[tokio::test]
async fn exact_policy_and_caller_credential_headers_fail_closed() {
    let server = spawn_server().await;
    let mut destination = destination("primary", "egress.test", server.address.port());
    destination.path_prefixes = vec!["/ok".into()];
    destination.methods = BTreeSet::from(["GET".into()]);
    let (runtime, calls) = runtime(config(vec![destination]), &server);
    let (ctx, events) = context(["egress.destination:primary"]);

    for mut denied in [
        request(
            "POST",
            format!("http://egress.test:{}/ok", server.address.port()),
        ),
        request(
            "GET",
            format!("http://egress.test:{}/other", server.address.port()),
        ),
    ] {
        assert!(matches!(
            runtime.execute(&ctx, denied.clone()).await,
            Err(EgressError::Denied(_))
        ));
        denied.method.clear();
    }
    let mut raw_auth = request(
        "GET",
        format!("http://egress.test:{}/ok", server.address.port()),
    );
    raw_auth
        .headers
        .insert("Authorization".into(), "Bearer exfiltrate".into());
    assert!(matches!(
        runtime.execute(&ctx, raw_auth).await,
        Err(EgressError::Denied(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        events
            .snapshot()
            .iter()
            .filter(|(kind, _)| kind == "egress_denied")
            .count(),
        3
    );
}

#[tokio::test]
async fn multi_encoded_delimiters_and_traversal_fail_before_dns() {
    let server = spawn_server().await;
    let mut selected = destination("selected", "egress.test", server.address.port());
    selected.path_prefixes = vec!["/allowed".into()];
    let (runtime, calls) = runtime(config(vec![selected.clone()]), &server);
    let (ctx, _) = context(["egress.destination:selected"]);

    for path in [
        "/allowed/%252e%252e/secret",
        "/allowed/%25252e%25252e/secret",
        "/allowed/%25%32%65%25%32%65/secret",
        "/allowed/%252fsecret",
        "/allowed/%255csecret",
    ] {
        assert!(matches!(
            runtime
                .execute(
                    &ctx,
                    request(
                        "GET",
                        format!("http://egress.test:{}{path}", server.address.port())
                    )
                )
                .await,
            Err(EgressError::Denied(_))
        ));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    for prefix in ["/allowed/..", "/allowed/%252e", "/allowed\\secret"] {
        selected.path_prefixes = vec![prefix.into()];
        assert!(matches!(
            EgressRuntime::with_resolver_and_options(
                config(vec![selected.clone()]),
                Arc::new(StaticResolver {
                    ip: server.address.ip(),
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
                EgressRuntimeOptions { allow_http: true },
            ),
            Err(EgressError::InvalidConfig(_))
        ));
    }
}

#[tokio::test]
async fn transport_does_not_inherit_process_proxy_environment() {
    const CHILD_MARKER: &str = "TM_EGRESS_PROXY_REGRESSION_CHILD";
    if std::env::var_os(CHILD_MARKER).is_none() {
        let status =
            tokio::process::Command::new(std::env::current_exe().expect("current test executable"))
                .arg("transport_does_not_inherit_process_proxy_environment")
                .arg("--test-threads=1")
                .env(CHILD_MARKER, "1")
                .env("HTTP_PROXY", "http://127.0.0.1:9")
                .env("HTTPS_PROXY", "http://127.0.0.1:9")
                .env("ALL_PROXY", "http://127.0.0.1:9")
                .env("http_proxy", "http://127.0.0.1:9")
                .env("https_proxy", "http://127.0.0.1:9")
                .env("all_proxy", "http://127.0.0.1:9")
                .env_remove("NO_PROXY")
                .env_remove("no_proxy")
                .status()
                .await
                .expect("run isolated proxy regression child");
        assert!(status.success(), "isolated proxy regression child failed");
        return;
    }

    let server = spawn_server().await;
    let selected = destination("selected", "egress.test", server.address.port());
    let (runtime, _) = runtime(config(vec![selected]), &server);
    let (ctx, _) = context(["egress.destination:selected"]);
    let response = runtime
        .execute(
            &ctx,
            request(
                "GET",
                format!("http://egress.test:{}/ok", server.address.port()),
            ),
        )
        .await
        .expect("pinned transport must ignore poisoned process proxy variables");
    assert_eq!(response.body, "ok");
}

#[tokio::test]
async fn scoped_protocol_request_refuses_a_different_destination_before_dns() {
    let server = spawn_server().await;
    let selected = destination("selected", "egress.test", server.address.port());
    let (runtime, calls) = runtime(config(vec![selected]), &server);
    let (ctx, events) = context(["egress.destination:selected"]);
    let error = runtime
        .execute_for_destination(
            &ctx,
            "configured-mcp-server",
            request(
                "GET",
                format!("http://egress.test:{}/ok", server.address.port()),
            ),
        )
        .await
        .expect_err("scoped request must not fall through to another destination");

    assert!(matches!(error, EgressError::Denied(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(events.snapshot()[0].0, "egress_denied");
}

#[tokio::test]
async fn bounded_protocol_response_header_is_host_only_not_model_serialized() {
    let server = spawn_server().await;
    let selected = destination("selected", "egress.test", server.address.port());
    let (runtime, _) = runtime(config(vec![selected]), &server);
    let (ctx, _) = context(["egress.destination:selected"]);
    let response = runtime
        .execute_for_destination(
            &ctx,
            "selected",
            request(
                "GET",
                format!(
                    "http://egress.test:{}/session-header",
                    server.address.port()
                ),
            ),
        )
        .await
        .expect("bounded response header");

    assert_eq!(
        response.headers.get("mcp-session-id").map(String::as_str),
        Some("host-only-session")
    );
    assert!(
        !serde_json::to_string(&response)
            .expect("model-visible response JSON")
            .contains("host-only-session")
    );
}

#[tokio::test]
async fn redirects_are_manual_pinned_and_reauthorized_per_hop() {
    let server = spawn_server().await;
    let mut primary = destination("primary", "egress.test", server.address.port());
    primary.redirect_to.insert("other".into());
    let other = destination("other", "other.test", server.address.port());
    let (runtime, calls) = runtime(config(vec![primary, other]), &server);
    let redirect_url = format!(
        "http://egress.test:{}/redirect-other",
        server.address.port()
    );

    let (missing_hop_grant, _) = context(["egress.destination:primary"]);
    assert!(matches!(
        runtime
            .execute(&missing_hop_grant, request("GET", redirect_url.clone()))
            .await,
        Err(EgressError::Denied(_))
    ));

    let (ctx, _) = context(["egress.destination:primary", "egress.destination:other"]);
    let response = runtime
        .execute(&ctx, request("GET", redirect_url))
        .await
        .expect("allowlisted redirect with exact hop grant should pass");
    assert_eq!(response.body, "ok");
    assert_eq!(response.redirects, 1);
    assert_eq!(response.destination_id, "other");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn timeout_response_and_session_budgets_are_enforced() {
    let server = spawn_server().await;
    let mut primary = destination("primary", "egress.test", server.address.port());
    primary.max_response_bytes = 32;
    primary.max_requests_per_session = 2;
    let (runtime, _) = runtime(config(vec![primary]), &server);
    let (ctx, _) = context(["egress.destination:primary"]);

    let mut slow = request(
        "GET",
        format!("http://egress.test:{}/slow", server.address.port()),
    );
    slow.timeout_ms = Some(20);
    assert_eq!(runtime.execute(&ctx, slow).await, Err(EgressError::Timeout));
    assert!(matches!(
        runtime
            .execute(
                &ctx,
                request(
                    "GET",
                    format!("http://egress.test:{}/large", server.address.port())
                )
            )
            .await,
        Err(EgressError::Budget(_))
    ));
    assert!(matches!(
        runtime
            .execute(
                &ctx,
                request(
                    "GET",
                    format!("http://egress.test:{}/ok", server.address.port())
                )
            )
            .await,
        Err(EgressError::Budget(_))
    ));
}

#[tokio::test]
#[serial]
async fn secret_handles_are_session_scoped_destination_scoped_and_redacted() {
    const ENV_NAME: &str = "TM_EGRESS_TEST_TOKEN_019F";
    const SECRET: &str = "super-secret-value-019f";
    struct EnvGuard;
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: this serial test owns the unique test environment variable.
            unsafe { std::env::remove_var(ENV_NAME) };
        }
    }
    // SAFETY: this serial test owns the unique test environment variable.
    unsafe { std::env::set_var(ENV_NAME, SECRET) };
    let _guard = EnvGuard;

    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let mut config = config(vec![primary]);
    config.secrets.push(EgressSecretConfig {
        id: "api".into(),
        version: 1,
        env: ENV_NAME.into(),
        destinations: BTreeSet::from(["primary".into()]),
        injection: EgressSecretInjection::AuthorizationBearer,
    });
    let (runtime, _) = runtime(config, &server);
    let (ctx, events) = context(["egress.destination:primary", "secrets.use:api"]);
    let handle = runtime
        .issue_secret_handle(&ctx, "api")
        .await
        .expect("exact secret grant should issue a handle");
    let mut authorized = request(
        "GET",
        format!("http://egress.test:{}/echo-auth", server.address.port()),
    );
    authorized.auth = Some(handle.clone());
    let response = runtime
        .execute(&ctx, authorized.clone())
        .await
        .expect("authorized secret request should pass");
    assert_eq!(response.body, "Bearer [REDACTED_SECRET]");
    assert_eq!(response.secret_redactions, 1);

    let wrong_session = InvocationCtx::new(
        CapabilityGrants::default()
            .allow("egress.destination:primary")
            .allow("secrets.use:api"),
    )
    .with_session_id("session-b")
    .with_event_sink(events.clone());
    assert_eq!(
        runtime.execute(&wrong_session, authorized).await,
        Err(EgressError::InvalidSecretHandle)
    );
    let audit = serde_json::to_string(&events.snapshot()).expect("serialize audit events");
    assert!(!audit.contains(SECRET));
    assert!(!format!("{handle:?}").contains(&handle.token));
}

#[tokio::test]
async fn secret_handles_reuse_are_bounded_and_clear_with_the_session() {
    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let mut policy = config(vec![primary]);
    policy.secrets.push(EgressSecretConfig {
        id: "api".into(),
        version: 1,
        env: "TM_EGRESS_UNUSED_BOUNDED_TOKEN_019F".into(),
        destinations: BTreeSet::from(["primary".into()]),
        injection: EgressSecretInjection::AuthorizationBearer,
    });
    let (runtime, resolver_calls) = runtime(policy, &server);
    let grants = CapabilityGrants::default()
        .allow("egress.destination:primary")
        .allow("secrets.use:api");
    let events = Arc::new(RecordingEvents::default());
    let base = InvocationCtx::new(grants.clone())
        .with_session_id("bounded-session")
        .with_event_sink(events.clone());
    let first = runtime.issue_secret_handle(&base, "api").await.unwrap();
    let reused = runtime.issue_secret_handle(&base, "api").await.unwrap();
    assert_eq!(first, reused, "MCP-style repeated issuance must reuse");

    for index in 1..MAX_SECRET_HANDLES_PER_SESSION {
        let actor = InvocationCtx::new(grants.clone())
            .with_session_id("bounded-session")
            .with_actor_id(Some(format!("actor-{index}")))
            .with_event_sink(events.clone());
        runtime.issue_secret_handle(&actor, "api").await.unwrap();
    }
    let exhausted = InvocationCtx::new(grants.clone())
        .with_session_id("bounded-session")
        .with_actor_id(Some("one-too-many"))
        .with_event_sink(events.clone());
    assert!(matches!(
        runtime.issue_secret_handle(&exhausted, "api").await,
        Err(EgressError::Budget(_))
    ));

    runtime.clear_session("bounded-session").await.unwrap();
    let mut stale = request(
        "GET",
        format!("http://egress.test:{}/echo-auth", server.address.port()),
    );
    stale.auth = Some(first);
    assert_eq!(
        runtime.execute(&base, stale).await,
        Err(EgressError::InvalidSecretHandle)
    );
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
    let fresh = runtime
        .issue_secret_handle(&base, "api")
        .await
        .expect("session cleanup releases the handle budget");
    runtime
        .admin_handle()
        .revoke_secret("api")
        .await
        .expect("operator can revoke a configured secret");
    let mut revoked = request(
        "GET",
        format!("http://egress.test:{}/echo-auth", server.address.port()),
    );
    revoked.auth = Some(fresh);
    assert!(matches!(
        runtime.execute(&base, revoked).await,
        Err(EgressError::Revoked(_))
    ));
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn revocation_survives_same_version_replace_and_rotation_reopens() {
    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let current = config(vec![primary.clone()]);
    let (runtime, resolver_calls) = runtime(current.clone(), &server);
    let admin = runtime.admin_handle();
    let (ctx, _) = context(["egress.destination:primary"]);
    let url = format!("http://egress.test:{}/ok", server.address.port());

    admin
        .revoke_destination("primary")
        .await
        .expect("configured destination can be revoked");
    assert!(matches!(
        runtime.execute(&ctx, request("GET", url.clone())).await,
        Err(EgressError::Revoked(_))
    ));
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
    admin
        .replace(current)
        .await
        .expect("same-version replacement is valid");
    assert!(matches!(
        runtime.execute(&ctx, request("GET", url.clone())).await,
        Err(EgressError::Revoked(_))
    ));

    let mut rotated = primary;
    rotated.version = 2;
    admin
        .replace(config(vec![rotated]))
        .await
        .expect("explicit version rotation is valid");
    assert_eq!(admin.policy_generation().await, 3);
    assert_eq!(
        runtime
            .execute(&ctx, request("GET", url))
            .await
            .expect("version rotation should reopen the destination")
            .body,
        "ok"
    );
}

#[tokio::test]
async fn a_restart_reloads_policy_but_never_rehydrates_old_opaque_handles() {
    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let mut policy = config(vec![primary]);
    policy.secrets.push(EgressSecretConfig {
        id: "api".into(),
        version: 1,
        env: "TM_EGRESS_RESTART_TOKEN_019F".into(),
        destinations: BTreeSet::from(["primary".into()]),
        injection: EgressSecretInjection::AuthorizationBearer,
    });
    let (before_restart, _) = runtime(policy.clone(), &server);
    let (ctx, _) = context(["egress.destination:primary", "secrets.use:api"]);
    let stale = before_restart
        .issue_secret_handle(&ctx, "api")
        .await
        .expect("configured handle can be issued before restart");

    let (after_restart, resolver_calls) = runtime(policy, &server);
    let mut request = request(
        "GET",
        format!("http://egress.test:{}/echo-auth", server.address.port()),
    );
    request.auth = Some(stale);
    assert_eq!(
        after_restart.execute(&ctx, request).await,
        Err(EgressError::InvalidSecretHandle)
    );
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[serial]
async fn operator_reload_rolls_back_atomically_and_rotates_secret_material() {
    const OLD_ENV: &str = "TM_EGRESS_RELOAD_OLD_TOKEN_019F";
    const NEW_ENV: &str = "TM_EGRESS_RELOAD_NEW_TOKEN_019F";
    const OLD_SECRET: &str = "old-reload-secret-019f";
    const NEW_SECRET: &str = "new-reload-secret-019f";
    struct EnvGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }
    impl EnvGuard {
        fn set(name: &'static str, value: &'static str) -> Self {
            let previous = std::env::var_os(name);
            // SAFETY: the serial test owns its two unique environment names and restores them.
            unsafe { std::env::set_var(name, value) };
            Self { name, previous }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: the serial test owns its two unique environment names and restores them.
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(self.name, value),
                    None => std::env::remove_var(self.name),
                }
            }
        }
    }
    let _old_guard = EnvGuard::set(OLD_ENV, OLD_SECRET);
    let _new_guard = EnvGuard::set(NEW_ENV, NEW_SECRET);

    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let mut policy = config(vec![primary]);
    policy.secrets.push(EgressSecretConfig {
        id: "api".into(),
        version: 1,
        env: OLD_ENV.into(),
        destinations: BTreeSet::from(["primary".into()]),
        injection: EgressSecretInjection::AuthorizationBearer,
    });
    let (runtime, resolver_calls) = runtime(policy.clone(), &server);
    let admin = runtime.admin_handle();
    let (ctx, events) = context(["egress.destination:primary", "secrets.use:api"]);
    let old_handle = runtime.issue_secret_handle(&ctx, "api").await.unwrap();

    let mut invalid = policy.clone();
    invalid.destinations[0].max_response_bytes = 0;
    assert!(matches!(
        admin.replace(invalid).await,
        Err(EgressError::InvalidConfig(_))
    ));
    assert_eq!(admin.policy_generation().await, 1);
    let mut old_request = request(
        "GET",
        format!("http://egress.test:{}/echo-auth", server.address.port()),
    );
    old_request.auth = Some(old_handle.clone());
    let old_response = runtime
        .execute(&ctx, old_request)
        .await
        .expect("invalid reload leaves the old generation usable");
    assert_eq!(old_response.body, "Bearer [REDACTED_SECRET]");

    let mut rotated = policy;
    rotated.secrets[0].version = 2;
    rotated.secrets[0].env = NEW_ENV.into();
    admin
        .replace(rotated)
        .await
        .expect("valid secret rotation swaps atomically");
    assert_eq!(admin.policy_generation().await, 2);
    // The broker stores only the env reference and resolves into a Zeroizing buffer per request;
    // after rotation the old literal can disappear from the process environment immediately.
    // SAFETY: this serial test owns and later restores the unique environment name.
    unsafe { std::env::remove_var(OLD_ENV) };
    let calls_before_stale = resolver_calls.load(Ordering::SeqCst);
    let mut stale_request = request(
        "GET",
        format!("http://egress.test:{}/echo-auth", server.address.port()),
    );
    stale_request.auth = Some(old_handle);
    assert_eq!(
        runtime.execute(&ctx, stale_request).await,
        Err(EgressError::InvalidSecretHandle)
    );
    assert_eq!(resolver_calls.load(Ordering::SeqCst), calls_before_stale);

    let new_handle = runtime.issue_secret_handle(&ctx, "api").await.unwrap();
    let mut new_request = request(
        "GET",
        format!("http://egress.test:{}/echo-auth", server.address.port()),
    );
    new_request.auth = Some(new_handle);
    let new_response = runtime.execute(&ctx, new_request).await.unwrap();
    assert_eq!(new_response.body, "Bearer [REDACTED_SECRET]");
    let encoded = serde_json::to_string(&events.snapshot()).unwrap();
    assert!(!encoded.contains(OLD_SECRET));
    assert!(!encoded.contains(NEW_SECRET));
}

#[tokio::test]
async fn revocation_does_not_wait_for_an_in_flight_peer_and_new_calls_fail_closed() {
    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let (runtime, calls) = runtime(config(vec![primary]), &server);
    let (ctx, _) = context(["egress.destination:primary"]);
    let slow_url = format!("http://egress.test:{}/slow", server.address.port());
    let running = tokio::spawn({
        let runtime = runtime.clone();
        let ctx = ctx.clone();
        async move { runtime.execute(&ctx, request("GET", slow_url)).await }
    });
    tokio::time::timeout(Duration::from_millis(100), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request reaches the pinned resolver");

    tokio::time::timeout(
        Duration::from_millis(30),
        runtime.revoke_destination("primary"),
    )
    .await
    .expect("revocation must not wait for a slow peer")
    .expect("configured destination can be revoked");
    let calls_before_denial = calls.load(Ordering::SeqCst);
    assert!(matches!(
        runtime
            .execute(
                &ctx,
                request(
                    "GET",
                    format!("http://egress.test:{}/ok", server.address.port())
                )
            )
            .await,
        Err(EgressError::Revoked(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), calls_before_denial);

    assert_eq!(
        running
            .await
            .expect("in-flight request task should not panic")
            .expect("an already-authorized immutable snapshot may complete")
            .body,
        "late"
    );
}

#[derive(Default)]
struct RecordingApproval {
    actions: Mutex<Vec<String>>,
}

#[async_trait]
impl ApprovalPolicy for RecordingApproval {
    async fn request(&self, action: &str, _timeout: Duration) -> tm_host::Result<ApprovalDecision> {
        self.actions
            .lock()
            .expect("approval lock poisoned")
            .push(action.to_string());
        Ok(ApprovalDecision::Denied)
    }
}

#[tokio::test]
async fn approved_mutation_is_sent_once_and_same_intent_replays_a_receipt() {
    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let (runtime, resolver_calls) = runtime(config(vec![primary]), &server);
    let events = Arc::new(DurableRecordingEvents::new(
        "019f0000-0000-7000-8000-000000000001",
    ));
    let ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default()
            .allow("http.request")
            .allow("egress.destination:primary"),
        Arc::new(AlwaysApprove),
        Duration::from_millis(50),
    )
    .with_session_id("019f0000-0000-7000-8000-000000000002")
    .with_event_sink(events);
    let function = HttpRequestFn::new(runtime);
    let args = json!({
        "method": "POST",
        "url": format!("http://egress.test:{}/ok?mode=create", server.address.port()),
        "body": "{\"name\":\"one\"}",
    });

    let first = function
        .call(args.clone(), &ctx)
        .await
        .expect("first approved mutation executes");
    assert_eq!(first["body"], "posted");
    let replay = function
        .call(args, &ctx)
        .await
        .expect("same durable intent returns a receipt");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["status"], "succeeded");
    assert!(replay["effectId"].as_str().is_some());
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mutation_result_is_durable_before_completion_audit() {
    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let (runtime, _) = runtime(config(vec![primary]), &server);
    let timeline = Arc::new(Mutex::new(Vec::new()));
    runtime
        .admin_handle()
        .install_state_store(Arc::new(TimelineStateStore {
            inner: VolatileEgressStateStore::default(),
            timeline: timeline.clone(),
        }));
    let events = Arc::new(TimelineEvents {
        scope_id: "019f0000-0000-7000-8000-000000000009".into(),
        timeline: timeline.clone(),
    });
    let ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default()
            .allow("http.request")
            .allow("egress.destination:primary"),
        Arc::new(AlwaysApprove),
        Duration::from_millis(50),
    )
    .with_session_id("019f0000-0000-7000-8000-00000000000a")
    .with_event_sink(events);

    HttpRequestFn::new(runtime)
        .call(
            json!({
                "method": "POST",
                "url": format!("http://egress.test:{}/ok", server.address.port()),
            }),
            &ctx,
        )
        .await
        .expect("approved mutation completes");

    let timeline = timeline.lock().expect("timeline lock poisoned");
    let finished = timeline
        .iter()
        .position(|entry| entry == "effect_finished")
        .expect("effect result is durable");
    let completed = timeline
        .iter()
        .position(|entry| entry == "egress_completed")
        .expect("completion audit is emitted");
    assert!(finished < completed, "timeline: {timeline:?}");
}

#[tokio::test]
async fn interrupted_started_mutation_becomes_uncertain_and_is_never_resent() {
    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let (runtime, resolver_calls) = runtime(config(vec![primary]), &server);
    let events = Arc::new(DurableRecordingEvents::new(
        "019f0000-0000-7000-8000-000000000003",
    ));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("egress.destination:primary"))
        .with_session_id("019f0000-0000-7000-8000-000000000004")
        .with_event_sink(events);
    let request = request(
        "POST",
        format!("http://egress.test:{}/ok", server.address.port()),
    );
    let prepared = runtime.prepare_mutation(&ctx, &request).await.unwrap();
    assert!(matches!(
        runtime.begin_mutation(&ctx, &prepared).await.unwrap(),
        MutationExecution::Execute(_)
    ));
    let replay = runtime.begin_mutation(&ctx, &prepared).await.unwrap();
    let MutationExecution::Replay(record) = replay else {
        panic!("an interrupted started intent must not execute again");
    };
    assert_eq!(record.status, EgressMutationStatus::Uncertain);
    assert_eq!(
        record.error_code.as_deref(),
        Some("interrupted_before_terminal_persistence")
    );
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn durable_effect_id_collision_is_rejected() {
    let store = VolatileEgressStateStore::default();
    let intent = EgressMutationIntent {
        effect_id: "a".repeat(64),
        session_id: "019f0000-0000-7000-8000-000000000005".into(),
        effect_scope_id: "019f0000-0000-7000-8000-000000000006".into(),
        session_digest: "b".repeat(64),
        actor_digest: "c".repeat(64),
        destination_id: "primary".into(),
        destination_version: 1,
        target_digest: "d".repeat(64),
        request_digest: "e".repeat(64),
        request_bytes: 12,
    };
    assert!(store.begin_mutation(intent.clone()).await.unwrap().created);
    let mut collision = intent;
    collision.request_digest = "f".repeat(64);
    assert!(matches!(
        store.begin_mutation(collision).await,
        Err(EgressError::Durability(_))
    ));
}

struct RevokeSecretOnApprove {
    admin: crate::EgressAdmin,
    actions: Mutex<Vec<String>>,
}

#[async_trait]
impl ApprovalPolicy for RevokeSecretOnApprove {
    async fn request(&self, action: &str, _timeout: Duration) -> tm_host::Result<ApprovalDecision> {
        self.actions
            .lock()
            .expect("approval action lock poisoned")
            .push(action.to_string());
        self.admin
            .revoke_secret("api")
            .await
            .map_err(|error| HostError::HostCall(error.to_string()))?;
        Ok(ApprovalDecision::Approved)
    }
}

#[tokio::test]
async fn approval_revalidates_secret_snapshot_without_reading_secret_value() {
    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let mut policy = config(vec![primary]);
    policy.secrets.push(EgressSecretConfig {
        id: "api".into(),
        version: 7,
        env: "TM_EGRESS_INTENTIONALLY_MISSING_APPROVAL_SECRET_019F".into(),
        destinations: BTreeSet::from(["primary".into()]),
        injection: EgressSecretInjection::AuthorizationBearer,
    });
    let (runtime, resolver_calls) = runtime(policy, &server);
    let approval = Arc::new(RevokeSecretOnApprove {
        admin: runtime.admin_handle(),
        actions: Mutex::new(Vec::new()),
    });
    let events = Arc::new(DurableRecordingEvents::new(
        "019f0000-0000-7000-8000-000000000007",
    ));
    let ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default()
            .allow("http.request")
            .allow("egress.destination:primary")
            .allow("secrets.use:api"),
        approval.clone(),
        Duration::from_millis(50),
    )
    .with_session_id("019f0000-0000-7000-8000-000000000008")
    .with_event_sink(events);
    let handle = runtime.issue_secret_handle(&ctx, "api").await.unwrap();
    let error = HttpRequestFn::new(runtime)
        .call(
            json!({
                "method": "POST",
                "url": format!("http://egress.test:{}/ok?token=hidden", server.address.port()),
                "auth": handle,
            }),
            &ctx,
        )
        .await
        .expect_err("post-approval secret revocation must fail before transport");
    assert!(matches!(error, HostError::CapabilityDenied(_)));
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
    let action = &approval.actions.lock().expect("approval lock poisoned")[0];
    let preview: Value = serde_json::from_str(action).unwrap();
    assert_eq!(preview["secretId"], "api");
    assert_eq!(preview["secretVersion"], 7);
    assert_eq!(preview["queryPreview"]["pairs"][0]["value"], "[REDACTED]");
    assert!(!action.contains("TM_EGRESS_INTENTIONALLY_MISSING"));
    assert!(!action.contains("hidden"));
    assert!(!action.contains(&handle.token));
}

#[tokio::test]
async fn redirect_request_size_uses_the_current_hop_url() {
    let server = spawn_server().await;
    let mut primary = destination("primary", "egress.test", server.address.port());
    primary.max_request_bytes = 96;
    let (runtime, resolver_calls) = runtime(config(vec![primary]), &server);
    let (ctx, _) = context(["egress.destination:primary"]);
    let error = runtime
        .execute(
            &ctx,
            request(
                "GET",
                format!("http://egress.test:{}/redirect-long", server.address.port()),
            ),
        )
        .await
        .expect_err("the longer redirect URL must exceed the per-hop request cap");
    assert!(matches!(error, EgressError::Budget(_)));
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn non_get_host_call_requires_bounded_redacted_approval() {
    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let (runtime, calls) = runtime(config(vec![primary]), &server);
    let approvals = Arc::new(RecordingApproval::default());
    let ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default()
            .allow("http.request")
            .allow("egress.destination:primary"),
        approvals.clone(),
        Duration::from_millis(50),
    )
    .with_session_id("session-a");
    let function = HttpRequestFn::new(runtime);
    let error = function
        .call(
            json!({
                "method": "POST",
                "url": format!("http://egress.test:{}/ok?private=query", server.address.port()),
                "headers": { "x-test": "private-header" },
                "body": "private-body",
            }),
            &ctx,
        )
        .await
        .expect_err("non-GET must honor approval denial");
    assert!(matches!(error, HostError::ApprovalDenied(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let action = approvals.actions.lock().expect("approval lock poisoned")[0].clone();
    for private in ["private-header", "private-body"] {
        assert!(!action.contains(private));
    }
    let preview: Value = serde_json::from_str(&action).expect("structured approval preview");
    assert_eq!(preview["path"], "/ok");
    assert_eq!(preview["method"], "POST");
    assert_eq!(preview["headerNames"], json!(["x-test"]));
    assert_eq!(preview["bodyBytes"], "private-body".len());
    assert_eq!(preview["queryPreview"]["pairs"][0]["name"], "private");
    assert_eq!(preview["queryPreview"]["pairs"][0]["value"], "query");
    assert!(preview["queryDigest"].as_str().is_some());
    assert!(preview["requestDigest"].as_str().is_some());
    assert!(preview["targetDigest"].as_str().is_some());
    assert!(preview["bodySha256"].as_str().is_some());
    assert!(preview["bodyPreview"].is_null());
    assert!(action.len() < 2_048);
}

#[tokio::test]
async fn non_get_approval_has_bounded_redacted_json_semantics() {
    let server = spawn_server().await;
    let primary = destination("primary", "egress.test", server.address.port());
    let (runtime, calls) = runtime(config(vec![primary]), &server);
    let approvals = Arc::new(RecordingApproval::default());
    let ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default()
            .allow("http.request")
            .allow("egress.destination:primary"),
        approvals.clone(),
        Duration::from_millis(50),
    )
    .with_session_id("session-a");
    let function = HttpRequestFn::new(runtime);
    let body = json!({
        "operation": "create_note",
        "title": "Taipei plan",
        "count": 3,
        "apiToken": "literal-secret-must-not-persist",
        "nested": {
            "enabled": true,
            "password": "another-secret",
            "opaque": "sk-test-secret-1234567890"
        },
        "items": [1, "visible"]
    })
    .to_string();
    let error = function
        .call(
            json!({
                "method": "POST",
                "url": format!(
                    "http://egress.test:{}/notes/token/sk-path-secret?private=query",
                    server.address.port()
                ),
                "headers": { "x-test": "private-header-value" },
                "body": body,
            }),
            &ctx,
        )
        .await
        .expect_err("manual approval denial remains fail closed");
    assert!(matches!(error, HostError::ApprovalDenied(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let action = approvals.actions.lock().expect("approval lock poisoned")[0].clone();
    let preview: Value = serde_json::from_str(&action).expect("structured approval preview");
    assert_eq!(preview["method"], "POST");
    assert_eq!(preview["path"], "/notes/token/[REDACTED]");
    assert_eq!(preview["headerNames"], json!(["x-test"]));
    assert_eq!(preview["bodyPreview"]["operation"], "create_note");
    assert_eq!(preview["bodyPreview"]["title"], "Taipei plan");
    assert_eq!(preview["bodyPreview"]["count"], 3);
    assert_eq!(preview["bodyPreview"]["nested"]["enabled"], true);
    assert_eq!(preview["bodyPreview"]["items"], json!([1, "visible"]));
    assert_eq!(preview["bodyPreview"]["apiToken"], "[REDACTED]");
    assert_eq!(preview["bodyPreview"]["nested"]["password"], "[REDACTED]");
    assert_eq!(preview["bodyPreview"]["nested"]["opaque"], "[REDACTED]");
    assert!(preview["bodySha256"].as_str().is_some());
    for secret in [
        "literal-secret-must-not-persist",
        "another-secret",
        "sk-test-secret-1234567890",
        "sk-path-secret",
        "private=query",
        "private-header-value",
    ] {
        assert!(
            !action.contains(secret),
            "approval leaked {secret}: {action}"
        );
    }
    assert!(action.len() < 4_096, "approval preview is not bounded");
}

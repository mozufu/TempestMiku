use super::*;
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::atomic::{AtomicUsize, Ordering},
};
use tm_egress::{DnsResolver, EgressError, EgressRuntime, HttpRequest, register_egress_functions};
use tm_host::{EgressConfig, HostEventSink};

fn enabled_egress() -> EgressConfig {
    let config: EgressConfig = serde_json::from_value(json!({
        "enabled": true,
        "destinations": [{
            "id": "research",
            "scheme": "https",
            "host": "api.example.com",
            "port": 443,
            "path_prefixes": ["/v1"],
            "methods": ["GET", "POST"]
        }],
        "secrets": [{
            "id": "research_api",
            "env": "RESEARCH_API_TOKEN",
            "destinations": ["research"],
            "injection": {"kind": "authorization_bearer"}
        }]
    }))
    .unwrap();
    config.validate().unwrap();
    config
}

fn assert_exact_egress_grants(capabilities: &[String]) {
    for expected in [
        "http.request",
        "egress.destination:research",
        "secrets.use",
        "secrets.use:research_api",
    ] {
        assert!(
            capabilities.iter().any(|capability| capability == expected),
            "missing {expected}: {capabilities:?}"
        );
    }
    for forbidden in [
        "http.get",
        "egress.*",
        "egress.destination:*",
        "secrets.*",
        "secrets.use:*",
    ] {
        assert!(
            !capabilities
                .iter()
                .any(|capability| capability == forbidden),
            "wildcard egress authority leaked into turn: {capabilities:?}"
        );
    }
}

#[tokio::test]
async fn configured_egress_extends_only_existing_network_mode_envelopes() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(RecordingChatRunner::default());
    let backend = Arc::new(RecordingBackend::default());
    let state = AppState::new(
        store,
        memory,
        Arc::clone(&chat),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_egress_config(&enabled_egress())
    .with_coding_backend(backend.clone());
    let router = app(state);

    let general = create_with_body(&router, Body::from(r#"{"mode":"general"}"#)).await;
    post_user_message(&router, general.id, "research from the assistant").await;
    let serious = create_with_body(&router, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    post_user_message(&router, serious.id, "research from engineering mode").await;

    let chat_turns = chat.turns.lock();
    assert_eq!(chat_turns.len(), 1);
    assert_exact_egress_grants(&chat_turns[0].capabilities);
    drop(chat_turns);

    let coding_turns = backend.turns.lock();
    assert_eq!(coding_turns.len(), 1);
    assert_exact_egress_grants(&coding_turns[0].capabilities);
}

#[test]
fn disabled_egress_adds_no_turn_authority_and_non_network_modes_stay_closed() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_egress_config(&EgressConfig::default());

    let mut network = vec!["http.request".to_string()];
    state.extend_egress_turn_capabilities(&mut network);
    assert_eq!(network, ["http.request"]);

    let mut non_network = vec!["drive.search".to_string()];
    let enabled = state.with_egress_config(&enabled_egress());
    enabled.extend_egress_turn_capabilities(&mut non_network);
    assert_eq!(non_network, ["drive.search"]);
}

#[tokio::test(flavor = "current_thread")]
async fn production_egress_denial_is_persisted_and_replayable() {
    let code = r#"
let denied = handle (@http.request {method: "GET", url: "https://not-configured.example/private"}) with error {
  | CapabilityDeniedError {message, ...} -> {name: "CapabilityDeniedError", message: message}
  | other -> rethrow other
};
denied |> display {kind: "json"}
"#;
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_disabled_egress".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(json!({"code": code}).to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("denial recorded".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
    ]));
    let mut options = TmSandboxOptions::default();
    register_egress_functions(
        &mut options.host_registry,
        EgressRuntime::new(EgressConfig::default()).unwrap(),
    );
    let chat = Arc::new(AgentChatRunner::tm(
        llm,
        AgentConfig {
            model: "fake".to_string(),
            max_turns: 3,
            ..AgentConfig::default()
        },
        options,
    ));
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let router = app(AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    ));
    let session = create_with_body(&router, Body::from(r#"{"mode":"general"}"#)).await;

    post_user_message(&router, session.id, "prove disabled production egress").await;

    let events = store.events_after(session.id, None).await.unwrap();
    let denied = events
        .iter()
        .find(|event| event.event_type == "egress_denied")
        .expect("egress denial is durable");
    assert_eq!(denied.payload_json["errorCode"], "disabled");
    assert_eq!(denied.payload_json["method"], "GET");
    assert!(denied.payload_json.get("url").is_none());
    let replay = store
        .events_after(session.id, Some(denied.seq.saturating_sub(1)))
        .await
        .unwrap();
    assert!(
        replay
            .iter()
            .any(|event| event.seq == denied.seq && event.event_type == "egress_denied")
    );
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(!encoded.contains("not-configured.example"), "{encoded}");
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_destination_revocation_denies_before_dns_and_is_durable() {
    let code = r#"
let denied = handle (@http.request {method: "GET", url: "https://api.example.com/v1/private"}) with error {
  | CapabilityDeniedError {message, ...} -> {name: "CapabilityDeniedError", message: message}
  | other -> rethrow other
};
denied |> display {kind: "json"}
"#;
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_revoked_egress".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(json!({"code": code}).to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("revocation recorded".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
    ]));
    let config = enabled_egress();
    let runtime = EgressRuntime::new(config.clone()).unwrap();
    runtime.revoke_destination("research").await.unwrap();
    let mut options = TmSandboxOptions::default();
    register_egress_functions(&mut options.host_registry, runtime);
    let chat = Arc::new(AgentChatRunner::tm(
        llm,
        AgentConfig {
            model: "fake".to_string(),
            max_turns: 3,
            ..AgentConfig::default()
        },
        options,
    ));
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let router = app(AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_egress_config(&config));
    let session = create_with_body(&router, Body::from(r#"{"mode":"general"}"#)).await;

    post_user_message(&router, session.id, "prove destination revocation").await;

    let events = store.events_after(session.id, None).await.unwrap();
    let denied = events
        .iter()
        .find(|event| event.event_type == "egress_denied")
        .expect("revocation denial is durable");
    assert_eq!(denied.payload_json["errorCode"], "revoked");
    assert_eq!(denied.payload_json["destinationId"], "research");
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(!encoded.contains("/v1/private"), "{encoded}");
}

#[derive(Clone)]
struct CountingResolver(Arc<AtomicUsize>);

#[async_trait]
impl DnsResolver for CountingResolver {
    async fn resolve(&self, _host: &str, port: u16) -> tm_egress::Result<Vec<SocketAddr>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)])
    }
}

#[derive(Default)]
struct NoopEgressEvents;

#[async_trait]
impl HostEventSink for NoopEgressEvents {
    async fn emit(&self, _event_type: &str, _payload_json: Value) -> tm_host::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn ending_session_clears_opaque_secret_handles_before_future_dns() {
    let config = enabled_egress();
    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let runtime = EgressRuntime::with_resolver(
        config,
        Arc::new(CountingResolver(Arc::clone(&resolver_calls))),
    )
    .unwrap();
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let state = AppState::new(
        store,
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_egress_admin(runtime.admin_handle());
    let router = app(state);
    let session = create_with_body(&router, Body::from(r#"{"mode":"general"}"#)).await;
    let ctx = InvocationCtx::new(
        CapabilityGrants::default()
            .allow("egress.destination:research")
            .allow("secrets.use:research_api"),
    )
    .with_session_id(session.id.to_string())
    .with_event_sink(Arc::new(NoopEgressEvents));
    let handle = runtime
        .issue_secret_handle(&ctx, "research_api")
        .await
        .expect("issue session-bound handle before teardown");

    let ended = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/end", session.id))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ended.status(), StatusCode::OK);

    assert_eq!(
        runtime
            .execute(
                &ctx,
                HttpRequest {
                    method: "GET".into(),
                    url: "https://api.example.com/v1/resource".into(),
                    headers: Default::default(),
                    body: None,
                    auth: Some(handle),
                    timeout_ms: None,
                },
            )
            .await,
        Err(EgressError::InvalidSecretHandle)
    );
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
}

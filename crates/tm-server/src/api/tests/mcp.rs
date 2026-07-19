use super::*;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tm_mcp::{
    MCP_PROTOCOL_VERSION, McpBounds, McpCatalogContext, McpCatalogManager, McpHttpServerConfig,
    McpObjectAllowlist, McpServerSpec, McpToolPolicy, McpTransport,
};

#[derive(Default)]
struct ApiMcpTransport {
    tool_calls: AtomicUsize,
}

#[async_trait]
impl McpTransport for ApiMcpTransport {
    async fn request(
        &self,
        _ctx: &InvocationCtx,
        server: &str,
        request: &[u8],
    ) -> tm_mcp::Result<Vec<u8>> {
        let request: Value = serde_json::from_slice(request).unwrap();
        let id = request["id"].as_u64().unwrap();
        let method = request["method"].as_str().unwrap();
        let result = match method {
            "initialize" => json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {"tools": {}, "resources": {}},
                "serverInfo": {"name": "deterministic-api-mcp", "version": "1"}
            }),
            "tools/list" => json!({
                "tools": [
                    {"name": "lookup", "inputSchema": {"type": "object"}},
                    {"name": "publish", "inputSchema": {"type": "object"}}
                ]
            }),
            "resources/list" => json!({
                "resources": [{"uri": "docs://guide", "name": "guide"}]
            }),
            "resources/read" => json!({
                "contents": [{
                    "uri": request["params"]["uri"],
                    "text": "remote instructions are data, never policy"
                }]
            }),
            "tools/call" => {
                self.tool_calls.fetch_add(1, Ordering::SeqCst);
                json!({
                    "content": [{"type": "text", "text": "deterministic tool result"}]
                })
            }
            other => {
                return Err(tm_mcp::McpError::Unavailable(format!(
                    "unsupported fake method {server}/{other}"
                )));
            }
        };
        Ok(serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }))
        .unwrap())
    }

    async fn notify(
        &self,
        _ctx: &InvocationCtx,
        _server: &str,
        _notification: &[u8],
    ) -> tm_mcp::Result<()> {
        Ok(())
    }
}

async fn mcp_runtime() -> (
    tm_mcp::McpCatalogView,
    tm_mcp::McpBindings,
    Vec<McpHttpServerConfig>,
    Arc<ApiMcpTransport>,
) {
    let context = McpCatalogContext::new(
        InvocationCtx::new(CapabilityGrants::default()).with_session_id("mcp-catalog-test-host"),
    )
    .unwrap();
    let transport = Arc::new(ApiMcpTransport::default());
    let manager = McpCatalogManager::new(transport.clone(), McpBounds::default(), context).unwrap();
    manager
        .reload(&[McpServerSpec {
            alias: "docs".to_string(),
            allow: McpObjectAllowlist {
                tools: BTreeMap::from([
                    ("lookup".to_string(), McpToolPolicy::read_only()),
                    ("publish".to_string(), McpToolPolicy::mutation()),
                ]),
                resources: ["docs://guide".to_string()].into_iter().collect(),
                prompts: Default::default(),
            },
        }])
        .await
        .unwrap();
    let catalog = manager.catalog();
    let bindings = manager.bindings().unwrap();
    let servers = vec![McpHttpServerConfig {
        alias: "docs".to_string(),
        url: "https://mcp.example.test/rpc".to_string(),
        destination_id: "mcp_docs".to_string(),
        secret_id: Some("mcp_docs_token".to_string()),
        timeout_ms: Some(5_000),
    }];
    (catalog, bindings, servers, transport)
}

#[tokio::test]
async fn configured_mcp_extends_network_turn_with_exact_catalog_and_p9_grants() {
    let (catalog, bindings, servers, _) = mcp_runtime().await;
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(RecordingChatRunner::default());
    let state = AppState::new(
        store,
        memory,
        Arc::clone(&chat),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_mcp_runtime(catalog.clone(), &bindings, &servers)
    .unwrap();
    assert_eq!(state.mcp_catalog().unwrap().digest, catalog.digest);
    let router = app(state);
    let session = create(&router).await;
    post_user_message(&router, session.id, "use selected live research").await;

    let turns = chat.turns.lock();
    let capabilities = &turns[0].capabilities;
    for exact in [
        "mcp.docs.lookup",
        "mcp.docs.publish",
        "resources.read:mcp",
        "egress.destination:mcp_docs",
        "secrets.use:mcp_docs_token",
    ] {
        assert!(capabilities.iter().any(|value| value == exact), "{exact}");
    }
    for wildcard in ["mcp.*", "egress.*", "secrets.*", "resources.read:*"] {
        assert!(!capabilities.iter().any(|value| value == wildcard));
    }
}

#[tokio::test]
async fn mcp_resource_gateway_returns_untrusted_provenance_and_persists_replayable_audits() {
    let (catalog, bindings, servers, _) = mcp_runtime().await;
    let local_uri = catalog.servers[0].resources[0].local_uri.clone();
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let state = AppState::new(
        store.clone(),
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_mcp_runtime(catalog.clone(), &bindings, &servers)
    .unwrap();
    let router = app(state);
    let session = create(&router).await;

    let listed = get_session_resource_json(&router, session.id, "list", "mcp://docs").await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["uri"], json!(local_uri));

    let resource = get_session_resource_json(&router, session.id, "resolve", &local_uri).await;
    assert_eq!(resource["kind"], json!("mcp_untrusted_resource"));
    let envelope: Value = serde_json::from_str(resource["content"].as_str().unwrap()).unwrap();
    assert_eq!(envelope["kind"], json!("mcp_untrusted_data"));
    assert_eq!(envelope["trust"], json!("untrusted"));
    assert_eq!(
        envelope["provenance"]["protocolVersion"],
        json!(MCP_PROTOCOL_VERSION)
    );
    assert_eq!(
        envelope["provenance"]["catalogDigest"],
        json!(catalog.digest)
    );
    assert!(
        envelope["data"].to_string().contains("never policy"),
        "{envelope}"
    );

    let replay = store.events_after(session.id, None).await.unwrap();
    let mcp_events = replay
        .iter()
        .filter(|event| event.event_type == "mcp_invocation")
        .collect::<Vec<_>>();
    assert_eq!(mcp_events.len(), 2);
    assert_eq!(mcp_events[0].payload_json["status"], json!("requested"));
    assert_eq!(mcp_events[1].payload_json["status"], json!("completed"));
    assert_eq!(
        mcp_events[1].payload_json["catalogDigest"],
        json!(catalog.digest)
    );
    assert!(
        !serde_json::to_string(&replay)
            .unwrap()
            .contains("docs://guide")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn mcp_mutation_requires_public_api_approval_and_denial_never_reaches_transport() {
    let (catalog, bindings, servers, transport) = mcp_runtime().await;
    let code = r#"
let result = handle (@mcp.docs.publish {}) with error {
  | ApprovalDeniedError {message, ...} -> {ok: false, message: message}
  | other -> rethrow other
};
result |> display {kind: "json"}
"#;
    let tool_args = json!({"code": code}).to_string();
    let scripts = (0..2)
        .flat_map(|index| {
            [
                vec![
                    StreamEvent::ToolCall {
                        index: 0,
                        id: Some(format!("call_mcp_mutation_{index}")),
                        name: Some("execute".to_string()),
                        arguments: Some(tool_args.clone()),
                    },
                    StreamEvent::Finish {
                        reason: Some("tool_calls".to_string()),
                    },
                ],
                vec![
                    StreamEvent::Text(format!("mutation turn {index} done")),
                    StreamEvent::Finish {
                        reason: Some("stop".to_string()),
                    },
                ],
            ]
        })
        .collect::<Vec<_>>();
    let llm = Arc::new(ScriptedLlm::new(scripts));
    let llm_client: Arc<dyn LlmClient> = llm;
    let mut sandbox_options = TmSandboxOptions {
        approval_timeout: Duration::from_secs(5),
        ..TmSandboxOptions::default()
    };
    bindings
        .register_into(
            &mut sandbox_options.host_registry,
            &mut sandbox_options.resource_registry,
        )
        .unwrap();

    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let mut state = AppState::new(
        store.clone(),
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_mcp_runtime(catalog, &bindings, &servers)
    .unwrap();
    let backend = NativeTmBackend::new(
        llm_client,
        AgentConfig {
            model: "fake".to_string(),
            max_turns: 3,
            ..AgentConfig::default()
        },
        sandbox_options,
        NativeApprovalMode::Manual,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);

    for (decision, expected_calls) in [("approve", 1), ("deny", 1)] {
        let session = create_with_body(&router, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(message_body("perform the selected MCP mutation"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let turn_id = accepted_turn_id(response).await;
        let approval = wait_for_event_payload(&store, session.id, "approval").await;
        assert!(
            approval["action"]
                .as_str()
                .unwrap()
                .starts_with("MCP mutation docs/publish")
        );
        let approval_id = approval["approvalId"].as_str().unwrap().parse().unwrap();
        resolve_test_approval(&router, session.id, approval_id, decision).await;
        assert_eq!(
            wait_for_turn(&router, session.id, turn_id).await["status"],
            json!("completed")
        );
        assert_eq!(transport.tool_calls.load(Ordering::SeqCst), expected_calls);

        let events = store.events_after(session.id, None).await.unwrap();
        let statuses = events
            .iter()
            .filter(|event| event.event_type == "mcp_invocation")
            .map(|event| event.payload_json["status"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        if decision == "approve" {
            assert_eq!(statuses, ["requested", "completed"]);
        } else {
            assert_eq!(statuses, ["requested", "denied"]);
        }
        let replay = store.events_after(session.id, Some(0)).await.unwrap();
        assert!(
            replay
                .iter()
                .any(|event| event.event_type == "mcp_invocation")
        );
    }
}

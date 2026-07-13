use super::*;

#[tokio::test]
async fn serious_engineer_session_uses_authoritative_global_scope_and_recalls_next_session() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session_a = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    assert_eq!(session_a.voice_cap, "off");
    assert_eq!(session_a.default_scope, "global");
    assert_eq!(session_a.active_skills, vec!["serious-engineer-ops"]);
    post_user_message(&app, session_a.id, "tempestmiku code open loop").await;
    let chunks = store
        .recall_chunks("global", "tempestmiku", 5)
        .await
        .unwrap();
    assert_eq!(chunks.len(), 1);
    assert!(
        chunks[0]
            .text
            .contains("Project summary/open loop from session")
    );
    let session_b = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    post_user_message(&app, session_b.id, "tempestmiku code open loop").await;
    let events = store.events_after(session_b.id, None).await.unwrap();
    let final_event = events
        .iter()
        .find(|event| event.event_type == "final")
        .unwrap();
    assert!(
        final_event
            .payload_json
            .to_string()
            .contains("Project summary/open loop from session")
    );
}

#[tokio::test]
async fn serious_engineer_uses_coding_backend_and_replays_events() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_coding_backend(Arc::new(ScriptedBackend::events()));
    let (app, store) = test_app_with_state(state);
    let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

    post_user_message(&app, session.id, "ship p0a production code").await;

    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["mode", "text", "diff", "artifact", "final"]
    );
    let replay = store.events_after(session.id, Some(1)).await.unwrap();
    assert_eq!(
        replay
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["text", "diff", "artifact", "final"]
    );
    let final_event = events
        .iter()
        .find(|event| event.event_type == "final")
        .unwrap();
    let final_text = final_event.payload_json.to_string();
    assert!(final_text.contains("artifact://"));
    assert!(!final_text.contains("喵"));
}

#[tokio::test]
async fn approval_route_resolves_pending_backend_permission() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    );
    let broker = Arc::clone(&state.approval_broker);
    state = state.with_coding_backend(Arc::new(ScriptedBackend::approval(Arc::clone(&broker))));
    let (app, store) = test_app_with_state(state);
    let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(message_body("needs approval for a code patch"))
                .unwrap(),
        )
        .await
        .unwrap();
    let turn_id = accepted_turn_id(response).await;

    let approval_id = wait_for_event_payload(&store, session.id, "approval").await["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/sessions/{}/approvals/{}",
                    session.id, approval_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"decision":"approve"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        wait_for_turn(&app, session.id, turn_id).await["status"],
        json!("completed")
    );

    let resolved = store
        .events_after(session.id, None)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.event_type == "approval_resolved")
        .unwrap();
    assert_eq!(resolved.payload_json["outcome"], json!("selected"));
    assert_eq!(resolved.payload_json["optionId"], json!("allow"));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn native_deno_backend_approval_route_approves_proc_run() {
    let (app, store, llm, session, _temp) =
        native_deno_approval_app(Duration::from_secs(5), native_proc_script()).await;
    let session_id = session.id;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/messages"))
                .header("content-type", "application/json")
                .body(message_body("run an unsafe proc command"))
                .unwrap(),
        )
        .await
        .unwrap();
    let turn_id = accepted_turn_id(response).await;

    let approval = wait_for_event_payload(&store, session_id, "approval").await;
    assert_eq!(approval["backend"], json!("native-deno"));
    assert_eq!(approval["action"], json!("proc.run cargo clean"));
    let approval_id = approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/approvals/{approval_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"decision":"approve"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        wait_for_turn(&app, session_id, turn_id).await["status"],
        json!("completed")
    );

    let resolved = store
        .events_after(session_id, None)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.event_type == "approval_resolved")
        .unwrap();
    assert_eq!(resolved.payload_json["backend"], json!("native-deno"));
    assert_eq!(resolved.payload_json["optionId"], json!("allow"));
    assert_eq!(resolved.turn_id, Some(turn_id));
    assert!(native_tool_result(&llm).contains("\"ok\":true"));
    assert!(
        store
            .events_after(session_id, None)
            .await
            .unwrap()
            .iter()
            .any(|event| event.event_type == "cell_result")
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn native_deno_backend_approval_route_denies_proc_run() {
    let (app, store, llm, session, _temp) =
        native_deno_approval_app(Duration::from_secs(5), native_proc_script()).await;
    let session_id = session.id;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/messages"))
                .header("content-type", "application/json")
                .body(message_body("deny an unsafe proc command"))
                .unwrap(),
        )
        .await
        .unwrap();
    let turn_id = accepted_turn_id(response).await;

    let approval_id = wait_for_event_payload(&store, session_id, "approval").await["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/approvals/{approval_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"decision":"deny"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        wait_for_turn(&app, session_id, turn_id).await["status"],
        json!("completed")
    );
    assert!(native_tool_result(&llm).contains("ApprovalDeniedError"));
    let resolved = store
        .events_after(session_id, None)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.event_type == "approval_resolved")
        .unwrap();
    assert_eq!(resolved.payload_json["backend"], json!("native-deno"));
    assert_eq!(resolved.payload_json["optionId"], json!("reject"));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn native_deno_backend_approval_timeout_defaults_to_deny() {
    let (app, store, llm, session, _temp) =
        native_deno_approval_app(Duration::from_millis(1), native_proc_script()).await;
    post_user_message(&app, session.id, "timeout an unsafe proc command").await;
    assert!(native_tool_result(&llm).contains("ApprovalTimeoutError"));
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().any(|event| {
        event.event_type == "approval" && event.payload_json["backend"] == json!("native-deno")
    }));
    assert!(events.iter().any(|event| {
        event.event_type == "approval_resolved"
            && event.payload_json["backend"] == json!("native-deno")
            && event.payload_json["optionId"] == json!("reject")
    }));
}

#[tokio::test]
async fn native_child_actor_receives_only_delegated_grants() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(linked_root.join("src")).unwrap();
    std::fs::write(
        linked_root.join("Cargo.toml"),
        "[package]\nname = \"native-child-approval-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(linked_root.join("src/lib.rs"), "pub fn x() -> i32 { 1 }\n").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
    }])
    .unwrap();

    let parent_code = r#"
const digest = await agents.run("worker", "Verify undelegated repository capabilities are denied, then create a child artifact.");
display(digest);
"#;
    let child_code = r#"
const denied = await Promise.all([
  fs.read("repo:Cargo.toml").catch(err => err.name),
  code.search({ pattern: "native-child", paths: ["repo:Cargo.toml"], regex: false }).catch(err => err.name),
  proc.run("cargo", ["clean"], { cwd: "repo:" }).catch(err => err.name),
  resources.read("linked://repo/Cargo.toml").catch(err => err.name)
]);
const artifact = artifacts.put("child resource open ok", { title: "child output" });
display({ denied, artifact: artifact.uri });
"#;
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("parent_call".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(json!({ "code": parent_code }).to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("child_call".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(json!({ "code": child_code }).to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("child done with artifact://0".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("parent saw child finish".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
    ]));
    let cfg = AgentConfig {
        model: "fake".to_string(),
        max_turns: 4,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };

    let roster = Arc::new(tm_agents::MailboxRegistry::new());
    let mut sandbox_options = DenoSandboxOptions {
        artifact_root: artifact_root.clone(),
        linked_folders: Some(linked.clone()),
        approval_timeout: Duration::from_secs(5),
        ..DenoSandboxOptions::default()
    };
    tm_agents::register(
        &mut sandbox_options.host_registry,
        &mut sandbox_options.resource_registry,
        Arc::clone(&roster),
    );

    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone())
    .with_linked_folders(linked.clone())
    .with_actor_roster(Arc::clone(&roster));
    let broker = Arc::clone(&state.approval_broker);

    let executor_options = sandbox_options.clone();
    let executor_roster = Arc::clone(&roster);
    let executor_approval_roster = Arc::clone(&roster);
    let executor_broker = Arc::clone(&broker);
    let llm_for_executor: Arc<dyn LlmClient> = llm.clone();
    let executor: Arc<dyn tm_agents::ActorExecutor> =
        Arc::new(crate::ChatActorExecutor::with_actor_context(
            llm_for_executor,
            cfg.clone(),
            move |session_id, actor_id, grants, session_scope, cancellation| {
                let mut opts = executor_options.clone();
                opts.session_id = session_id.to_string();
                opts.actor_id = actor_id.map(str::to_string);
                opts.session_scope = session_scope.map(str::to_string);
                opts.cancellation = cancellation;
                opts.grants = tm_sandbox::core_sandbox_grants()
                    .allow_many(grants.names().map(str::to_string));
                let sink: Arc<dyn CodingEventSink> = Arc::new(crate::RosterCodingEventSink::new(
                    session_id,
                    Arc::clone(&executor_approval_roster),
                ));
                opts.approval_policy = Arc::new(
                    crate::HttpApprovalPolicy::new(Arc::clone(&executor_broker), session_id, sink)
                        .with_actor_id(actor_id.map(str::to_string)),
                );
                Arc::new(tm_sandbox::DenoSandbox::new(opts)) as Arc<dyn tm_core::Sandbox>
            },
            Some(artifact_root.clone()),
            executor_roster,
        ));
    roster.set_executor(executor);

    let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
    let backend = NativeDenoBackend::new(
        llm_for_backend,
        cfg,
        sandbox_options,
        NativeApprovalMode::Manual,
        broker,
    );
    state = state.with_coding_backend(Arc::new(backend));
    state.wire_lifecycle_sink();
    let app = app(state);
    let session = create_with_body(&app, Body::from(r#"{"mode":"handoff"}"#)).await;
    let session_id = session.id;

    post_user_message(&app, session_id, "handoff child grant check").await;

    let completed = wait_for_event_payload(&store, session.id, "actor_completed").await;
    let actor_id = completed["actor_id"]
        .as_str()
        .expect("completed child should carry actorId")
        .to_string();
    assert_eq!(completed["actor_id"], json!(actor_id));
    assert_eq!(completed["artifact_uri"], json!("artifact://0"));
    assert_eq!(
        completed["history_uri"],
        json!(format!("history://{actor_id}"))
    );
    let linked = wait_for_event_payload(&store, session.id, "actor_resources_linked").await;
    assert_eq!(linked["actor_id"], json!(actor_id));
    assert_eq!(linked["source_event_type"], json!("actor_completed"));
    assert_eq!(linked["artifact_uri"], json!("artifact://0"));
    assert_eq!(
        linked["history_uri"],
        json!(format!("history://{actor_id}"))
    );

    let events = store.events_after(session.id, None).await.unwrap();
    let expected_history_uri = format!("history://{actor_id}");
    let kinds = events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect::<Vec<_>>();
    let completed_event = events
        .iter()
        .find(|event| event.event_type == "actor_completed")
        .expect("actor_completed should be persisted");
    let completed_seq = completed_event.seq;
    assert_eq!(completed_event.actor_id.as_deref(), Some(actor_id.as_str()));
    assert_eq!(
        completed_event.artifact_uri.as_deref(),
        Some("artifact://0")
    );
    assert_eq!(
        completed_event.history_uri.as_deref(),
        Some(expected_history_uri.as_str())
    );
    let linked_event = events
        .iter()
        .find(|event| event.event_type == "actor_resources_linked")
        .expect("actor_resources_linked should be persisted");
    assert_eq!(linked_event.actor_id.as_deref(), Some(actor_id.as_str()));
    assert_eq!(linked_event.artifact_uri.as_deref(), Some("artifact://0"));
    assert_eq!(
        linked_event.history_uri.as_deref(),
        Some(expected_history_uri.as_str())
    );
    assert_eq!(linked["source_event_seq"], json!(completed_seq));
    assert!(kinds.contains(&"actor_spawned"));
    assert!(!kinds.contains(&"approval"));
    assert!(!kinds.contains(&"approval_resolved"));
    assert!(kinds.contains(&"actor_completed"));
    assert!(kinds.contains(&"actor_resources_linked"));
    assert!(kinds.contains(&"final"));

    let child_result_content = {
        let requests = llm.requests.lock();
        requests[2]
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("child tool result is fed back before its final turn")
            .content
            .clone()
    };
    assert_eq!(
        child_result_content
            .matches("CapabilityDeniedError")
            .count(),
        4,
        "child tool result: {child_result_content}"
    );

    let artifact = get_session_resource_json(&app, session.id, "resolve", "artifact://0").await;
    assert!(
        artifact["content"]
            .as_str()
            .unwrap_or_default()
            .contains("child resource open ok")
    );
    let history_uri = format!("history://{actor_id}");
    let history = get_session_resource_json(&app, session.id, "resolve", &history_uri).await;
    assert!(
        history["content"]
            .as_str()
            .unwrap_or_default()
            .contains("[cell_result]")
    );
}

#[tokio::test]
async fn actor_cancelled_event_replays_and_agent_resource_is_terminal() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let roster = Arc::new(tm_agents::MailboxRegistry::new());
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_actor_roster(Arc::clone(&roster));
    state.wire_lifecycle_sink();
    let app = app(state);
    let session = create_with_body(&app, Body::from(r#"{"mode":"handoff"}"#)).await;

    let actor_id = tm_agents::ActorId::new("Worker").unwrap();
    roster
        .track(tm_agents::ActorRecord {
            id: actor_id.clone(),
            parent: None,
            status: tm_agents::ActorStatus::Running,
            mode: Some("worker".to_string()),
            budget: tm_agents::ActorBudget::default(),
            spawned_at: Utc::now(),
            completed_at: None,
            cancelled: false,
            failure_reason: None,
            last_summary: None,
            artifact_uri: None,
            history_uri: None,
        })
        .await;

    let result = roster
        .cancel_actor(&session.id.to_string(), &actor_id)
        .await;
    assert_eq!(result.as_str(), "cancelled");

    let cancelled = wait_for_event_payload(&store, session.id, "actor_cancelled").await;
    assert_eq!(cancelled["actor_id"], json!("Worker"));
    assert_eq!(cancelled["kind"], json!("cancelled"));

    let events = store.events_after(session.id, None).await.unwrap();
    let cancelled_seq = events
        .iter()
        .find(|event| event.event_type == "actor_cancelled")
        .map(|event| event.seq)
        .expect("actor_cancelled seq");
    let replay = store
        .events_after(session.id, cancelled_seq.checked_sub(1))
        .await
        .unwrap();
    assert!(
        replay
            .iter()
            .any(|event| event.event_type == "actor_cancelled"),
        "Last-Event-ID replay should include actor_cancelled"
    );

    let resource = get_session_resource_json(&app, session.id, "resolve", "agent://Worker").await;
    assert_eq!(resource["uri"], json!("agent://Worker"));
    let record: serde_json::Value =
        serde_json::from_str(resource["content"].as_str().unwrap()).unwrap();
    assert_eq!(record["status"], json!("terminated"));
    assert_eq!(record["cancelled"], json!(true));
    assert_eq!(record["failure_reason"]["kind"], json!("cancelled"));

    store
        .append_event(session.id, "final", json!({"text": "done"}))
        .await
        .unwrap();
    store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    let sse = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/events?lastEventId={}",
                    session.id,
                    cancelled_seq - 1
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sse.status(), StatusCode::OK);
    let body = axum::body::to_bytes(sse.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(body.matches("event: session_event").count(), 3, "{body}");
    assert!(body.contains(r#""type":"actor_cancelled""#), "{body}");
    assert!(body.contains(r#""type":"final""#), "{body}");
    assert!(body.contains(r#""type":"session_end""#), "{body}");
}

#[tokio::test]
async fn actor_failed_event_replays_and_agent_resource_is_terminal() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let roster = Arc::new(tm_agents::MailboxRegistry::new());
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_actor_roster(Arc::clone(&roster));
    state.wire_lifecycle_sink();
    let app = app(state);
    let session = create_with_body(&app, Body::from(r#"{"mode":"handoff"}"#)).await;

    let actor_id = tm_agents::ActorId::new("ResearchWorker").unwrap();
    roster
        .track(tm_agents::ActorRecord {
            id: actor_id.clone(),
            parent: None,
            status: tm_agents::ActorStatus::Running,
            mode: Some("researcher".to_string()),
            budget: tm_agents::ActorBudget::default(),
            spawned_at: Utc::now(),
            completed_at: None,
            cancelled: false,
            failure_reason: None,
            last_summary: None,
            artifact_uri: None,
            history_uri: None,
        })
        .await;

    let decision = roster
        .record_actor_error(
            &session.id.to_string(),
            &actor_id,
            tm_agents::FailureReason::Timeout,
        )
        .await;
    assert!(decision.is_some());

    let failed = wait_for_event_payload(&store, session.id, "actor_failed").await;
    assert_eq!(failed["actor_id"], json!("ResearchWorker"));
    assert_eq!(failed["reason"]["kind"], json!("timeout"));

    let events = store.events_after(session.id, None).await.unwrap();
    let failed_seq = events
        .iter()
        .find(|event| event.event_type == "actor_failed")
        .map(|event| event.seq)
        .expect("actor_failed seq");
    let replay = store
        .events_after(session.id, failed_seq.checked_sub(1))
        .await
        .unwrap();
    assert!(
        replay
            .iter()
            .any(|event| event.event_type == "actor_failed"),
        "Last-Event-ID replay should include actor_failed"
    );

    let resource =
        get_session_resource_json(&app, session.id, "resolve", "agent://ResearchWorker").await;
    assert_eq!(resource["uri"], json!("agent://ResearchWorker"));
    let record: serde_json::Value =
        serde_json::from_str(resource["content"].as_str().unwrap()).unwrap();
    assert_eq!(record["status"], json!("terminated"));
    assert_eq!(record["cancelled"], json!(false));
    assert_eq!(record["failure_reason"]["kind"], json!("timeout"));

    store
        .append_event(session.id, "final", json!({"text": "done"}))
        .await
        .unwrap();
    store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    let sse = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/events?lastEventId={}",
                    session.id,
                    failed_seq - 1
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sse.status(), StatusCode::OK);
    let body = axum::body::to_bytes(sse.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains(r#""type":"actor_failed""#), "{body}");
    assert!(body.contains(r#""type":"final""#), "{body}");
    assert!(body.contains(r#""type":"session_end""#), "{body}");
}

#[tokio::test]
async fn artifact_resource_route_reads_session_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
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
    .with_artifact_root(root.clone())
    .with_coding_backend(Arc::new(ScriptedBackend::artifact(root)));
    let (app, _) = test_app_with_state(state);
    let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

    post_user_message(&app, session.id, "write code transcript").await;

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/resources/artifacts", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = response_json(list).await;
    assert_eq!(list_json[0]["uri"], json!("artifact://0"));

    let read = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/resources/artifacts/0", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    let read_json = response_json(read).await;
    assert_eq!(read_json["uri"], json!("artifact://0"));
    assert!(
        read_json["content"]
            .as_str()
            .unwrap()
            .contains("scripted transcript")
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn general_and_handoff_turns_deny_undeclared_linked_repo_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(&linked_root).unwrap();
    std::fs::write(
        linked_root.join("Cargo.toml"),
        "[workspace]\nmembers = []\n",
    )
    .unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
    }])
    .unwrap();

    let code = r#"
const denied = await Promise.all([
  fs.read("repo:Cargo.toml").catch(err => err.name),
  code.search({ pattern: "workspace", paths: ["repo:Cargo.toml"], regex: false }).catch(err => err.name),
  proc.run("cargo", ["test"], { cwd: "repo:" }).catch(err => err.name),
  resources.read("linked://repo/Cargo.toml").catch(err => err.name)
]);
display({ denied });
"#;
    let tool_turn = || {
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_denied_linked".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(json!({ "code": code }).to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ]
    };
    let final_turn = || {
        vec![
            StreamEvent::Text("denials checked".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ]
    };
    let llm = Arc::new(ScriptedLlm::new(vec![
        tool_turn(),
        final_turn(),
        tool_turn(),
        final_turn(),
    ]));
    let chat = Arc::new(AgentChatRunner::deno(
        llm.clone(),
        AgentConfig {
            model: "fake".to_string(),
            max_turns: 3,
            ..AgentConfig::default()
        },
        DenoSandboxOptions {
            artifact_root,
            linked_folders: Some(linked.clone()),
            ..DenoSandboxOptions::default()
        },
    ));
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_linked_folders(linked);
    let router = app(state);

    for mode in ["general", "handoff"] {
        let session =
            create_with_body(&router, Body::from(json!({ "mode": mode }).to_string())).await;
        post_user_message(&router, session.id, "inspect the linked project").await;
    }

    let requests = llm.requests.lock();
    assert_eq!(requests.len(), 4);
    for request_index in [1, 3] {
        let tool_result = requests[request_index]
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("tool result is fed back before final turn");
        assert_eq!(
            tool_result.content.matches("CapabilityDeniedError").count(),
            4,
            "tool result: {}",
            tool_result.content
        );
    }
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn serious_server_agent_route_uses_deno_sdk_and_preserves_denials() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(&linked_root).unwrap();
    std::fs::write(
        linked_root.join("Cargo.toml"),
        "[workspace]\nmembers = []\n",
    )
    .unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
    }])
    .unwrap();

    let code = r#"
const doc = await fs.read("linked://repo/Cargo.toml");
const artifact = artifacts.put(`server sdk artifact\n${doc.content}`, {
  title: "server-sdk",
  mime: "text/plain"
});
const unsafeProc = await proc.run("cargo", ["clean"], { cwd: "repo:" })
  .catch(err => ({ name: err.name, retryable: err.retryable }));
const deniedHttp = await http.get("https://evil.test/")
  .catch(err => ({ name: err.name, capability: err.capability }));
display({
  ok: doc.content.includes("[workspace]"),
  artifact: artifact.uri,
  unsafeProc,
  deniedHttp
});
"#;
    let tool_args = json!({ "code": code }).to_string();
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_server_sdk".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(tool_args),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("done".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
    ]));
    let cfg = AgentConfig {
        model: "fake".to_string(),
        max_turns: 3,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };
    let chat = Arc::new(AgentChatRunner::deno(
        llm.clone(),
        cfg,
        DenoSandboxOptions {
            artifact_root: artifact_root.clone(),
            linked_folders: Some(linked.clone()),
            approval_timeout: Duration::from_millis(1),
            ..DenoSandboxOptions::default()
        },
    ));
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone())
    .with_linked_folders(linked);
    let router = app(state);
    let session = create_with_body(
        &router,
        Body::from(r#"{"mode":"serious_engineer","scope":"project:repo"}"#),
    )
    .await;

    post_user_message(&router, session.id, "exercise the server sdk").await;

    let tool_result_content = {
        let requests = llm.requests.lock();
        assert_eq!(requests.len(), 2);
        requests[1]
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("tool result is fed back before final turn")
            .content
            .clone()
    };
    assert!(
        tool_result_content.contains("\"ok\":true"),
        "tool result: {tool_result_content}"
    );
    assert!(
        tool_result_content.contains("artifact://0"),
        "tool result: {tool_result_content}"
    );
    assert!(
        tool_result_content.contains("ApprovalTimeoutError"),
        "tool result: {tool_result_content}"
    );
    assert!(
        tool_result_content.contains("CapabilityDeniedError"),
        "tool result: {tool_result_content}"
    );

    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().any(|event| event.event_type == "cell_start"));
    assert!(events.iter().any(|event| event.event_type == "cell_result"));
    assert!(events.iter().any(
        |event| event.event_type == "final" && event.payload_json.to_string().contains("done")
    ));

    let list = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/resources/artifacts", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = response_json(list).await;
    assert_eq!(list_json[0]["uri"], json!("artifact://0"));

    let read = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/resources/artifacts/0", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    let read_json = response_json(read).await;
    assert!(
        read_json["content"]
            .as_str()
            .unwrap()
            .contains("server sdk artifact")
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn serious_engineer_native_deno_uses_linked_repo_and_scoped_drive_search() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(linked_root.join("src")).unwrap();
    std::fs::write(
        linked_root.join("Cargo.toml"),
        "[package]\nname = \"linked-drive-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(
        linked_root.join("src/lib.rs"),
        "pub fn linked_answer() -> i32 { 42 }\n",
    )
    .unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
    }])
    .unwrap();
    let drive_store = tm_drive::InMemoryDriveStore::new(
        tm_artifacts::ArtifactStore::open(temp.path(), "drive").unwrap(),
    );
    let filed = drive_store
        .put_bytes(
            b"# Scoped Linked Brief\nScoped drive metadata for the linked project.",
            tm_drive::DrivePutOptions {
                auto: true,
                suggested_path: Some("projects/repo/notes/scoped-linked.md".to_string()),
                project: Some("repo".to_string()),
                source_uri: Some("linked://repo/README.md".to_string()),
                approval_mode: tm_drive::DriveApprovalMode::Auto,
                ..tm_drive::DrivePutOptions::default()
            },
        )
        .unwrap();

    let code = format!(
        r#"
const read = await fs.read("repo:src/lib.rs");
const hits = await code.search({{ pattern: "linked_answer", paths: ["repo:src/lib.rs"], regex: false }});
const run = await proc.run("cargo", ["test", "--quiet"], {{ cwd: "repo:" }});
const driveHits = await drive.search("Scoped", {{ project: "repo", returnSnippets: true }});
display({{
  readOk: read.content.includes("linked_answer"),
  codeHits: hits.length,
  exitCode: run.exitCode,
  driveHits: driveHits.length,
  driveUri: driveHits[0]?.uri,
  driveProject: driveHits[0]?.project,
  expectedDriveUri: "{uri}"
}});
"#,
        uri = filed.uri
    );
    let tool_args = json!({ "code": code }).to_string();
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_linked_drive".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(tool_args),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("linked project checked".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
    ]));
    let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
    let cfg = AgentConfig {
        model: "fake".to_string(),
        max_turns: 3,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone())
    .with_linked_folders(linked.clone())
    .with_drive_store(drive_store.clone());
    let backend = NativeDenoBackend::new(
        llm_for_backend,
        cfg,
        DenoSandboxOptions {
            artifact_root,
            linked_folders: Some(linked),
            drive_store: Some(Arc::new(drive_store)),
            approval_timeout: Duration::from_secs(1),
            ..DenoSandboxOptions::default()
        },
        NativeApprovalMode::Deny,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);
    let session = create_with_body(
        &router,
        Body::from(r#"{"mode":"serious_engineer","scope":"project:repo"}"#),
    )
    .await;

    post_user_message(&router, session.id, "inspect the linked project").await;

    let tool_result = native_tool_result(&llm);
    assert!(tool_result.contains("\"readOk\":true"), "{tool_result}");
    assert!(tool_result.contains("\"codeHits\":1"), "{tool_result}");
    assert!(tool_result.contains("\"exitCode\":0"), "{tool_result}");
    assert!(tool_result.contains("\"driveHits\":1"), "{tool_result}");
    assert!(tool_result.contains(&filed.uri), "{tool_result}");
    assert!(
        tool_result.contains("\"driveProject\":\"repo\""),
        "{tool_result}"
    );
}

#[tokio::test]
async fn native_deno_drive_organizer_events_are_persisted_for_replay() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("linked");
    std::fs::create_dir_all(&linked_root).unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let drive_store = tm_drive::InMemoryDriveStore::new(
        tm_artifacts::ArtifactStore::open(temp.path(), "drive").unwrap(),
    );
    let code = r##"
await drive.put("# Raw\norganizer should move this into project notes", {
  suggestedPath: "inbox/raw.md",
  project: "TempestMiku",
  docKind: "note",
  approvalMode: "auto"
});
const proposals = await drive.organize();
display({
  proposals: proposals.length,
  status: proposals[0]?.status,
  source: proposals[0]?.sourcePath,
  target: proposals[0]?.proposedPath
});
"##;
    let tool_args = json!({ "code": code }).to_string();
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_drive_organize".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(tool_args),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("drive organizer checked".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
    ]));
    let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
    let cfg = AgentConfig {
        model: "fake".to_string(),
        max_turns: 3,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone())
    .with_linked_folders(linked.clone())
    .with_drive_store(drive_store.clone());
    let backend = NativeDenoBackend::new(
        llm_for_backend,
        cfg,
        DenoSandboxOptions {
            artifact_root,
            drive_store: Some(Arc::new(drive_store)),
            linked_folders: Some(linked),
            approval_timeout: Duration::from_secs(1),
            ..DenoSandboxOptions::default()
        },
        NativeApprovalMode::Deny,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);
    let session = create_with_body(
        &router,
        Body::from(r#"{"mode":"serious_engineer","scope":"project:tempestmiku"}"#),
    )
    .await;

    post_user_message(&router, session.id, "organize drive docs").await;

    let tool_result = native_tool_result(&llm);
    assert!(tool_result.contains("\"proposals\":1"), "{tool_result}");
    assert!(
        tool_result.contains("\"status\":\"pending\""),
        "{tool_result}"
    );
    let events = store.events_after(session.id, None).await.unwrap();
    let filed = events
        .iter()
        .find(|event| event.event_type == "drive_put")
        .expect("drive put event");
    assert_eq!(filed.payload_json["uri"], json!("drive://inbox/raw.md"));
    assert_eq!(
        filed.payload_json["preview"]["title"],
        json!("Filed drive document")
    );
    assert_eq!(
        filed.payload_json["resourceRefs"][0]["uri"],
        json!("drive://inbox/raw.md")
    );
    let started = events
        .iter()
        .find(|event| event.event_type == "drive_organizer_started")
        .expect("drive organizer start event");
    assert_eq!(started.payload_json["apply"], json!(false));
    let pending = events
        .iter()
        .find(|event| event.event_type == "write_proposal")
        .expect("drive write proposal event");
    assert_eq!(pending.payload_json["kind"], json!("drive"));
    assert_eq!(pending.payload_json["status"], json!("pending"));
    assert_eq!(
        pending.payload_json["sourceUri"],
        json!("drive://inbox/raw.md")
    );
    assert_eq!(
        pending.payload_json["proposedUri"],
        json!("drive://projects/tempestmiku/note/raw.md")
    );
    let completed = events
        .iter()
        .find(|event| event.event_type == "drive_organizer_completed")
        .expect("drive organizer completion event");
    assert_eq!(completed.payload_json["proposalCount"], json!(1));
    assert_eq!(
        completed.payload_json["proposals"][0]["sourceUri"],
        json!("drive://inbox/raw.md")
    );
    assert_eq!(
        completed.payload_json["proposals"][0]["proposedUri"],
        json!("drive://projects/tempestmiku/note/raw.md")
    );
    assert_eq!(
        completed.payload_json["resourceRefs"][0]["uri"],
        json!("drive://inbox/raw.md")
    );
    let replay = store
        .events_after(session.id, Some(started.seq - 1))
        .await
        .unwrap();
    assert!(
        replay
            .iter()
            .any(|event| event.event_type == "drive_organizer_started")
    );
    assert!(
        replay
            .iter()
            .any(|event| event.event_type == "drive_organizer_completed")
    );
    let transcript = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/messages", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(transcript.status(), StatusCode::OK);
    let transcript = response_json(transcript).await;
    let pending_events = transcript["pendingEvents"].as_array().unwrap();
    assert!(pending_events.iter().any(|event| {
        event["type"] == json!("write_proposal")
            && event["data"]["kind"] == json!("drive")
            && event["data"]["sourceUri"] == json!("drive://inbox/raw.md")
            && event["data"]["proposedUri"] == json!("drive://projects/tempestmiku/note/raw.md")
    }));
}

async fn native_deno_approval_app(
    timeout: Duration,
    code: &str,
) -> (
    Router,
    Arc<InMemoryStore>,
    Arc<ScriptedLlm>,
    CreateSessionResponse,
    tempfile::TempDir,
) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(linked_root.join("src")).unwrap();
    std::fs::write(
            linked_root.join("Cargo.toml"),
            "[package]\nname = \"native-deno-approval-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
    std::fs::write(linked_root.join("src/lib.rs"), "pub fn x() -> i32 { 1 }\n").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
    }])
    .unwrap();
    let tool_args = json!({ "code": code }).to_string();
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_native_deno".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(tool_args),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("done".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
    ]));
    let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
    let cfg = AgentConfig {
        model: "fake".to_string(),
        max_turns: 3,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone())
    .with_linked_folders(linked.clone());
    let backend = NativeDenoBackend::new(
        llm_for_backend,
        cfg,
        DenoSandboxOptions {
            artifact_root,
            linked_folders: Some(linked),
            approval_timeout: timeout,
            ..DenoSandboxOptions::default()
        },
        NativeApprovalMode::Manual,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);
    let session = create_with_body(
        &router,
        Body::from(r#"{"mode":"serious_engineer","scope":"project:repo"}"#),
    )
    .await;
    (router, store, llm, session, temp)
}

fn native_proc_script() -> &'static str {
    r#"
const result = await proc.run("cargo", ["clean"], { cwd: "repo:" })
  .then(ok => ({ ok: true, exitCode: ok.exitCode }))
  .catch(err => ({ ok: false, name: err.name, retryable: err.retryable }));
display(result);
"#
}

fn native_tool_result(llm: &ScriptedLlm) -> String {
    let requests = llm.requests.lock();
    requests[1]
        .iter()
        .find(|message| message.role == Role::Tool)
        .expect("tool result is fed back before final turn")
        .content
        .clone()
}

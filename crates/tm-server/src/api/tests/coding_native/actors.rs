use super::super::*;

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
let digest = @agents.run {role: "worker", task: "Verify only explicit read capabilities are delegated, then create a child artifact.", opts: {capabilities: ["http.get", "resources.read:artifact"]}};
digest |> display {kind: "json"}
"#;
    let child_code = r#"
let catalog = @tools.search {query: "", limit: 100};
let artifact = @artifacts.put {data: "child resource open ok", title: "child output"};
{catalog: catalog, artifact: artifact.uri} |> display {kind: "json"}
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
    let mut sandbox_options = TmSandboxOptions {
        artifact_root: artifact_root.clone(),
        linked_folders: Some(linked.clone()),
        approval_timeout: Duration::from_secs(5),
        ..TmSandboxOptions::default()
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
                opts.grants = tm_host::CapabilityGrants::default()
                    .allow_many(grants.names().map(str::to_string));
                let sink: Arc<dyn CodingEventSink> = Arc::new(crate::RosterCodingEventSink::new(
                    session_id,
                    Arc::clone(&executor_approval_roster),
                ));
                opts.approval_policy = Arc::new(
                    crate::HttpApprovalPolicy::new(Arc::clone(&executor_broker), session_id, sink)
                        .with_actor_id(actor_id.map(str::to_string)),
                );
                Arc::new(tm_lang::TmSandbox::new(opts)) as Arc<dyn tm_core::Sandbox>
            },
            Some(artifact_root.clone()),
            executor_roster,
        ));
    roster.set_executor(executor);

    let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
    let backend = NativeTmBackend::new(
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
    let child_result: Value = serde_json::from_str(
        child_result_content
            .strip_prefix("result:\n")
            .expect("tm tool result should contain a JSON result"),
    )
    .unwrap();
    for undelegated in ["fs.read", "code.search"] {
        let entry = child_result["catalog"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == undelegated)
            .unwrap_or_else(|| panic!("catalog should document {undelegated}"));
        assert_eq!(entry["granted"], json!(false), "{child_result_content}");
    }
    for delegated in [
        "http.get",
        "artifacts.get",
        "artifacts.slice",
        "artifacts.list",
    ] {
        let entry = child_result["catalog"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == delegated)
            .unwrap_or_else(|| panic!("catalog should document {delegated}"));
        assert_eq!(entry["granted"], json!(true), "{child_result_content}");
    }
    assert!(child_result_content.contains("artifact://0"));

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
        .track_for_session(
            &session.id.to_string(),
            tm_agents::ActorRecord {
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
            },
        )
        .await
        .unwrap();

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
        .track_for_session(
            &session.id.to_string(),
            tm_agents::ActorRecord {
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
            },
        )
        .await
        .unwrap();

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

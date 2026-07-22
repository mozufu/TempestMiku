use super::super::*;

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn gated_postgres_native_tm_cell_approves_and_replays_after_store_reconnect() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_native_cell_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let store = Arc::new(
        PostgresStore::connect_in_schema(&dsn, &schema)
            .await
            .unwrap(),
    );
    store.configure_owner_subject("brian").await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(&linked_root).unwrap();
    std::fs::write(linked_root.join("remove-me.txt"), "delete-me\n").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".into(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let code = r#"
let hits = @fs.grep {pattern: "delete-me", paths: ["repo:remove-me.txt"], regex: false};
hits |> par map (fun hit -> @fs.remove {path: hit.path, tag: hit.tag}) |> display {kind: "table"}
"#;
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("postgres_native_tm".into()),
                name: Some("execute".into()),
                arguments: Some(json!({"code": code}).to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::Text("done".into()),
            StreamEvent::Finish {
                reason: Some("stop".into()),
            },
        ],
    ]));
    let llm_client: Arc<dyn LlmClient> = llm;
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
    let backend = NativeTmBackend::new(
        llm_client,
        AgentConfig {
            model: "fake".into(),
            max_turns: 3,
            ..AgentConfig::default()
        },
        TmSandboxOptions {
            artifact_root,
            linked_folders: Some(linked),
            approval_timeout: Duration::from_secs(5),
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Manual,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);
    let session = create_with_body(
        &router,
        Body::from(r#"{"mode":"serious_engineer","projectId":"repo","memoryPolicy":"project"}"#),
    )
    .await;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(message_body("run the Postgres tm approval cell"))
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
    let approved = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/approvals/{approval_id}", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"decision":"approve"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    assert_eq!(
        wait_for_turn(&router, session.id, turn_id).await["status"],
        json!("completed")
    );
    assert!(!temp.path().join("repo/remove-me.txt").exists());
    let initial = store.events_after(session.id, None).await.unwrap();
    let cursor = initial
        .iter()
        .find(|event| event.event_type == "effect_suspended")
        .unwrap()
        .seq
        .saturating_sub(1);

    let reconnected = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let replay = reconnected
        .events_after(session.id, Some(cursor))
        .await
        .unwrap();
    for expected in [
        "effect_suspended",
        "approval",
        "approval_resolved",
        "effect_resumed",
        "effect_result",
        "scope_result",
        "display",
        "binding_committed",
        "cell_result",
        "final",
    ] {
        assert!(replay.iter().any(|event| event.event_type == expected));
    }

    drop(reconnected);
    drop(router);
    drop(store);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}

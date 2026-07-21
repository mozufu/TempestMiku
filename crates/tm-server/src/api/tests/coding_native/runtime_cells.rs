use super::super::*;
use super::support::{native_tm_effect_approval_app, native_tm_simple_app, native_tool_result};

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn native_tm_public_route_approves_one_redacted_effect_and_replays_trace() {
    let (app, store, llm, session, temp) =
        native_tm_effect_approval_app(Duration::from_secs(5)).await;
    let session_id = session.id;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/messages"))
                .header("content-type", "application/json")
                .body(message_body("remove the reviewed fixture through tm"))
                .unwrap(),
        )
        .await
        .unwrap();
    let turn_id = accepted_turn_id(response).await;

    let approval = wait_for_event_payload(&store, session_id, "approval").await;
    let approval_id = approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let before_approval = store.events_after(session_id, None).await.unwrap();
    let suspended = before_approval
        .iter()
        .find(|event| event.event_type == "effect_suspended")
        .expect("tm effect suspends before the HTTP approval");
    let replay_cursor = suspended.seq.saturating_sub(1);

    let approved = app
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
    assert_eq!(approved.status(), StatusCode::OK);
    assert_eq!(
        wait_for_turn(&app, session_id, turn_id).await["status"],
        json!("completed")
    );
    assert!(!temp.path().join("repo/remove-me.txt").exists());

    let events = store.events_after(session_id, None).await.unwrap();
    let effect_events = events
        .iter()
        .filter(|event| event.event_type.starts_with("effect_"))
        .collect::<Vec<_>>();
    assert_eq!(
        effect_events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec![
            "effect_start",
            "effect_result",
            "effect_start",
            "effect_suspended",
            "effect_resumed",
            "effect_result"
        ],
        "fs.grep and the exactly-once fs.remove each own one effect node"
    );
    let remove_start = effect_events
        .iter()
        .find(|event| event.payload_json["capability"] == json!("fs.remove"))
        .unwrap();
    assert_eq!(remove_start.payload_json["argsPreview"], "[redacted]");
    let remove_node = remove_start.payload_json["nodeId"].clone();
    assert_eq!(
        effect_events
            .iter()
            .filter(|event| event.payload_json["nodeId"] == remove_node)
            .count(),
        4
    );
    assert!(events.iter().any(|event| event.event_type == "scope_start"));
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "scope_result")
    );
    assert!(events.iter().any(|event| event.event_type == "display"));
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "binding_committed")
    );
    assert!(native_tool_result(&llm).contains("removed"));

    let replay = store
        .events_after(session_id, Some(replay_cursor))
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
        assert!(
            replay.iter().any(|event| event.event_type == expected),
            "missing {expected} from cursor replay: {:?}",
            replay
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>()
        );
    }

    let duplicate = app
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
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn native_tm_public_route_denial_rolls_back_bindings_and_effect() {
    let (app, store, llm, session, temp) =
        native_tm_effect_approval_app(Duration::from_secs(5)).await;
    let session_id = session.id;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/messages"))
                .header("content-type", "application/json")
                .body(message_body("deny the reviewed tm fixture removal"))
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
    let denied = app
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
    assert_eq!(denied.status(), StatusCode::OK);
    assert_eq!(
        wait_for_turn(&app, session_id, turn_id).await["status"],
        json!("completed")
    );
    assert!(temp.path().join("repo/remove-me.txt").exists());
    let events = store.events_after(session_id, None).await.unwrap();
    let remove_node = events
        .iter()
        .find(|event| {
            event.event_type == "effect_start"
                && event.payload_json["capability"] == json!("fs.remove")
        })
        .unwrap()
        .payload_json["nodeId"]
        .clone();
    assert!(events.iter().any(|event| {
        event.event_type == "effect_result"
            && event.payload_json["nodeId"] == remove_node
            && event.payload_json["status"] == json!("failed")
    }));
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "binding_committed"),
        "the top-level hits binding must roll back with the denied effect"
    );
    assert!(native_tool_result(&llm).contains("approval denied"));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn native_tm_public_route_timeout_defaults_to_deny_without_commit() {
    let (app, store, llm, session, temp) =
        native_tm_effect_approval_app(Duration::from_millis(1)).await;
    post_user_message(&app, session.id, "let the tm removal approval time out").await;
    assert!(temp.path().join("repo/remove-me.txt").exists());
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().any(|event| {
        event.event_type == "approval_resolved"
            && event.payload_json["status"] == json!("timed_out")
    }));
    assert!(events.iter().any(|event| {
        event.event_type == "effect_result" && event.payload_json["status"] == json!("failed")
    }));
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "binding_committed")
    );
    assert!(native_tool_result(&llm).contains("approval timed out"));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn native_tm_runtime_eviction_emits_reset_and_drops_ephemeral_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("tm_bind".into()),
                name: Some("execute".into()),
                arguments: Some(json!({"code": "let retained = 7"}).to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::Text("first".into()),
            StreamEvent::Finish {
                reason: Some("stop".into()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("tm_after_reset".into()),
                name: Some("execute".into()),
                arguments: Some(json!({"code": "retained"}).to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::Text("second".into()),
            StreamEvent::Finish {
                reason: Some("stop".into()),
            },
        ],
    ]));
    let llm_client: Arc<dyn LlmClient> = llm.clone();
    let cfg = AgentConfig {
        model: "fake".into(),
        max_turns: 3,
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
    );
    let backend = NativeTmBackend::new_with_options(
        llm_client,
        cfg,
        TmSandboxOptions {
            artifact_root,
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Deny,
        Arc::clone(&state.approval_broker),
        NativeTmBackendOptions {
            shard_count: 1,
            session_ttl: Duration::from_millis(1),
            event_channel_capacity: 32,
        },
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);
    let session = create_with_body(&router, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

    post_user_message(&router, session.id, "bind ephemeral tm state").await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    post_user_message(&router, session.id, "read after runtime eviction").await;

    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "runtime_reset")
            .count(),
        1
    );
    let reset = events
        .iter()
        .position(|event| event.event_type == "runtime_reset")
        .unwrap();
    let second_cell = events
        .iter()
        .rposition(|event| event.event_type == "cell_start")
        .unwrap();
    assert!(reset < second_cell);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "binding_committed")
            .count(),
        1,
        "only the pre-eviction successful cell commits a binding"
    );
    let requests = llm.requests.lock();
    let second_tool_result = requests[3]
        .iter()
        .find(|message| message.role == Role::Tool)
        .unwrap();
    assert!(second_tool_result.content.contains("unbound name retained"));
}

struct CancelledTmCell;

impl tm_core::CancellationToken for CancelledTmCell {
    fn is_cancelled(&self) -> bool {
        true
    }
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn native_tm_public_route_cancellation_never_commits() {
    let (app, store, llm, session, _temp) =
        native_tm_simple_app("let never = 1", Some(Arc::new(CancelledTmCell))).await;
    post_user_message(&app, session.id, "cancel this tm cell").await;
    assert!(native_tool_result(&llm).contains("cell cancelled"));
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "binding_committed")
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn native_tm_public_route_rejects_ungranted_effect_before_execution() {
    let (app, store, llm, session, _temp) =
        native_tm_simple_app("@agents.run {task: \"forbidden\"}", None).await;
    post_user_message(&app, session.id, "attempt an ungranted tm effect").await;
    assert!(native_tool_result(&llm).contains("unknown capability agents.run"));
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "effect_start"),
        "checker rejection must happen before any host effect starts"
    );
}

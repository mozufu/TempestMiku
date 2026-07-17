use super::super::*;
use super::support::{native_proc_script, native_tm_approval_app, native_tool_result};

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn native_tm_backend_approval_route_approves_proc_run() {
    let (app, store, llm, session, _temp) =
        native_tm_approval_app(Duration::from_secs(5), native_proc_script()).await;
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
    assert_eq!(approval["backend"], json!("native-tm"));
    let action: Value = serde_json::from_str(approval["action"].as_str().unwrap()).unwrap();
    assert_eq!(action["operation"], json!("proc.run"));
    assert_eq!(action["details"]["argvPreview"], json!(["cargo", "clean"]));
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
    assert_eq!(resolved.payload_json["backend"], json!("native-tm"));
    assert_eq!(resolved.payload_json["optionId"], json!("allow"));
    assert_eq!(resolved.turn_id, Some(turn_id));
    assert!(native_tool_result(&llm).contains("\"exitCode\": 0"));
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
async fn native_tm_backend_approval_route_denies_proc_run() {
    let (app, store, llm, session, _temp) =
        native_tm_approval_app(Duration::from_secs(5), native_proc_script()).await;
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
    assert_eq!(resolved.payload_json["backend"], json!("native-tm"));
    assert_eq!(resolved.payload_json["optionId"], json!("reject"));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn native_tm_backend_approval_timeout_defaults_to_deny() {
    let (app, store, llm, session, _temp) =
        native_tm_approval_app(Duration::from_millis(1), native_proc_script()).await;
    post_user_message(&app, session.id, "timeout an unsafe proc command").await;
    assert!(native_tool_result(&llm).contains("ApprovalTimeoutError"));
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().any(|event| {
        event.event_type == "approval" && event.payload_json["backend"] == json!("native-tm")
    }));
    assert!(events.iter().any(|event| {
        event.event_type == "approval_resolved"
            && event.payload_json["backend"] == json!("native-tm")
            && event.payload_json["optionId"] == json!("reject")
    }));
}

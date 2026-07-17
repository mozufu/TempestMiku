use super::*;

#[tokio::test]
async fn approval_detail_endpoint_returns_redacted_prompt_without_effect_payload() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let approval_id = Uuid::new_v4();
    let now = Utc::now();
    store
        .create_approval_request(crate::NewApprovalRequest {
            id: approval_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "native-tm".to_string(),
            action: "proc.run cargo test".to_string(),
            scope_json: json!({"capability": "proc.run"}),
            options_json: json!([
                {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
            ]),
            effect_type: "approval_continuation".to_string(),
            effect_payload_json: json!({"secretInternalValue": "must-not-leak"}),
            resumable: true,
            created_at: now,
            expires_at: now + chrono::Duration::minutes(5),
        })
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/approvals/{approval_id}", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["status"], json!("pending"));
    assert_eq!(response["action"], json!("proc.run cargo test"));
    assert_eq!(response["options"][0]["kind"], json!("allow_once"));
    assert!(!response.to_string().contains("secretInternalValue"));
    assert!(!response.to_string().contains("must-not-leak"));
}

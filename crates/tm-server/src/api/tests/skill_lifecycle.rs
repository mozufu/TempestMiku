use super::*;
use sha2::{Digest, Sha256};

fn managed_install(name: &str, body: &str, proposal_id: Uuid) -> tm_modes::ManagedSkillInstall {
    tm_modes::ManagedSkillInstall {
        name: name.to_string(),
        body: body.to_string(),
        content_digest: format!("sha256:{:x}", Sha256::digest(body.as_bytes())),
        source_proposal_id: proposal_id.to_string(),
        description: "Release workflow".to_string(),
        triggers: vec!["release notes".to_string()],
        use_criteria: "Use for release notes.".to_string(),
    }
}

#[tokio::test]
async fn managed_skill_resource_and_approved_rollback_survive_state_rebuild() {
    let root = tempfile::tempdir().unwrap();
    let persona = ModesConfig::default().with_managed_skills_path(root.path());
    let first = persona
        .install_managed_skill(managed_install(
            "release-workflow",
            "# release-workflow\n\nfirst\n",
            Uuid::new_v4(),
        ))
        .unwrap();
    let second = persona
        .install_managed_skill(managed_install(
            "release-workflow",
            "# release-workflow\n\nsecond\n",
            Uuid::new_v4(),
        ))
        .unwrap();

    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: persona.load_status(),
        })
        .await
        .unwrap();
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let state = AppState::new(
        store.clone(),
        memory,
        Arc::new(EchoChatRunner),
        persona.clone(),
        AuthConfig::NoAuth,
    );
    let (app, _) = test_app_with_state(state);

    let read = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/sessions/{}/resources/read?uri=skill://release-workflow",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    let read: Value = serde_json::from_slice(
        &axum::body::to_bytes(read.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(read["content"].as_str().unwrap().contains("second"));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/sessions/{}/evolution/skills/release-workflow/rollback",
                    session.id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "expectedActiveDigest": second.active.content_digest,
                        "targetDigest": first.active.content_digest,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let approval_id = response["approvalId"].as_str().unwrap();

    let resolved = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/approvals/{approval_id}", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"decision": "approve", "optionId": "allow"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status(), StatusCode::OK);
    assert_eq!(
        persona
            .managed_skill("release-workflow")
            .unwrap()
            .active
            .content_digest,
        first.active.content_digest
    );

    let restarted = ModesConfig::default().with_managed_skills_path(root.path());
    assert!(
        restarted
            .managed_skill_body("release-workflow")
            .unwrap()
            .1
            .contains("first")
    );
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().any(|event| {
        event.event_type == "write_proposal"
            && event.payload_json["kind"] == json!("skill_rollback")
            && event.payload_json["status"] == json!("approved")
    }));
}

#[tokio::test]
async fn stale_skill_rollback_is_rejected_before_approval() {
    let root = tempfile::tempdir().unwrap();
    let persona = ModesConfig::default().with_managed_skills_path(root.path());
    let first = persona
        .install_managed_skill(managed_install(
            "release-workflow",
            "# release-workflow\n\nfirst\n",
            Uuid::new_v4(),
        ))
        .unwrap();
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: persona.load_status(),
        })
        .await
        .unwrap();
    let state = AppState::new(
        store.clone(),
        Arc::new(StoreMemoryProvider::new(store)),
        Arc::new(EchoChatRunner),
        persona,
        AuthConfig::NoAuth,
    );
    let (app, _) = test_app_with_state(state);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/sessions/{}/evolution/skills/release-workflow/rollback",
                    session.id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "expectedActiveDigest": format!("sha256:{}", "0".repeat(64)),
                        "targetDigest": first.active.content_digest,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

use super::*;

#[tokio::test]
async fn in_memory_persistence_redacts_turn_messages_and_recursive_events_without_breaking_hash_idempotency()
 {
    let store = InMemoryStore::default();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let raw = "use sk-testsecret123456 for this request";
    let turn = store
        .enqueue_turn(session.id, "redacted-message", raw)
        .await
        .unwrap();
    assert_eq!(turn.content_hash, super::models::turn_content_hash(raw));
    assert!(!turn.content.contains("sk-testsecret123456"));
    assert_eq!(
        store
            .enqueue_turn(session.id, "redacted-message", raw)
            .await
            .unwrap()
            .id,
        turn.id
    );
    assert!(matches!(
        store
            .enqueue_turn(
                session.id,
                "redacted-message",
                "use sk-anothersecret123456 for this request",
            )
            .await,
        Err(ServerError::Conflict(_))
    ));
    let messages = store.session_messages(session.id).await.unwrap();
    assert!(!messages[0].content.contains("sk-testsecret123456"));

    let event = store
        .append_event(
            session.id,
            "diagnostic",
            json!({
                "nested": [{
                    "authorization": "short-secret",
                    "message": "Authorization: Bearer opaque-token-123456",
                }],
            }),
        )
        .await
        .unwrap();
    let serialized = serde_json::to_string(&event.payload_json).unwrap();
    assert!(!serialized.contains("short-secret"));
    assert!(!serialized.contains("opaque-token-123456"));
    assert!(serialized.contains("REDACTED"));

    let assistant = store
        .append_message(
            session.id,
            "assistant",
            "provider returned sk-assistantsecret123456",
        )
        .await
        .unwrap();
    assert!(!assistant.content.contains("sk-assistantsecret123456"));
}

#[tokio::test]
async fn in_memory_memory_and_project_store_boundaries_cannot_persist_credentials() {
    let store = InMemoryStore::default();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let secret = "sk-testsecret123456";
    let now = Utc::now();

    assert!(
        store
            .add_profile_fact(ProfileFactRecord {
                id: Uuid::new_v4(),
                subject: "brian".to_string(),
                predicate: "uses".to_string(),
                object: secret.to_string(),
                confidence: 1.0,
                importance: 1.0,
                provenance: "test".to_string(),
                valid_from: now,
                valid_to: None,
            })
            .await
            .is_err()
    );
    assert!(
        store
            .add_recall_chunk(RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: "global".to_string(),
                text: format!("remember {secret}"),
                source: "test".to_string(),
                importance: 1.0,
                created_at: now,
            })
            .await
            .is_err()
    );

    let project = store
        .upsert_project_item(NewProjectItem {
            project_id: "tempestmiku".to_string(),
            kind: ProjectItemKind::Summary,
            text: format!("provider returned {secret}"),
            target_uri: "project://tempestmiku/summary".to_string(),
            source_session_id: session.id,
            source_event_seq: None,
            source_uri: None,
            dedupe_key: "summary:test".to_string(),
            provenance_json: json!({"accessToken": "short-secret"}),
        })
        .await
        .unwrap();
    let project_json = serde_json::to_string(&project).unwrap();
    assert!(!project_json.contains(secret));
    assert!(!project_json.contains("short-secret"));

    let evidence = vec![MemoryEvidenceRef {
        session_id: session.id,
        event_seq: None,
        message_seq: None,
        uri: Some("history://Worker".to_string()),
        label: format!("evidence {secret}"),
    }];
    let summary = store
        .upsert_memory_summary(NewMemorySummaryRecord {
            kind: MemorySummaryKind::Session,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            title: format!("summary {secret}"),
            body: format!("body {secret}"),
            evidence: evidence.clone(),
            source_dream_id: Uuid::new_v4(),
            source_session_id: Some(session.id),
            dedupe_key: "summary:credential-test".to_string(),
        })
        .await
        .unwrap();
    assert!(!serde_json::to_string(&summary).unwrap().contains(secret));

    let skill = store
        .upsert_skill_proposal(NewSkillProposalRecord {
            name: "safe-workflow".to_string(),
            description: format!("description {secret}"),
            body: format!("body {secret}"),
            trigger: format!("trigger {secret}"),
            use_criteria: "when requested".to_string(),
            evidence,
            self_critique: format!("critique {secret}"),
            verification: SkillVerification {
                passed: true,
                checks: vec![format!("checked {secret}")],
            },
            dedupe_key: "skill:credential-test".to_string(),
            source_dream_id: Uuid::new_v4(),
            source_session_id: session.id,
        })
        .await
        .unwrap();
    assert!(!serde_json::to_string(&skill).unwrap().contains(secret));

    assert!(
        store
            .upsert_project_item(NewProjectItem {
                project_id: secret.to_string(),
                kind: ProjectItemKind::Summary,
                text: "safe".to_string(),
                target_uri: "project://safe".to_string(),
                source_session_id: session.id,
                source_event_seq: None,
                source_uri: None,
                dedupe_key: "reject:credential".to_string(),
                provenance_json: json!({}),
            })
            .await
            .is_err()
    );
}

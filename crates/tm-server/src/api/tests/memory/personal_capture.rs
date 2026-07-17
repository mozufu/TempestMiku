use super::*;

#[tokio::test]
async fn personal_assistant_state_capture_proposes_memory_through_approval_flow() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;

    post_user_message(
        &app,
        session_id,
        "Remember that I prefer approval-backed state capture summaries.",
    )
    .await;

    let pending = wait_for_event_payload(&store, session_id, "write_proposal").await;
    assert_eq!(pending["kind"], json!("memory"));
    assert_eq!(pending["memoryKind"], json!("profile_fact"));
    assert_eq!(pending["status"], json!("pending"));
    assert_eq!(
        pending["preview"],
        json!("brian prefers approval-backed state capture summaries")
    );
    assert!(pending.get("predicate").is_none());
    assert!(pending.get("object").is_none());
    let proposal_id = pending["proposalId"].as_str().unwrap().parse().unwrap();
    let candidate = store.evolution_memory_proposal(proposal_id).await.unwrap();
    assert_eq!(candidate.predicate.as_deref(), Some("prefers"));
    assert_eq!(
        candidate.object.as_deref(),
        Some("approval-backed state capture summaries")
    );
    assert_eq!(
        candidate.provenance_label,
        "personal-assistant-state-capture"
    );
    assert!(store.profile_facts("brian").await.unwrap().is_empty());

    let approval = wait_for_event_payload(&store, session_id, "approval").await;
    assert_eq!(approval["backend"], json!("memory"));
    assert_eq!(
        approval["scope"]["proposal"]["proposalId"],
        pending["proposalId"]
    );
    let approval_id = approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_test_approval(&app, session_id, approval_id, "approve").await;

    let approved =
        wait_for_write_proposal_status(&store, session_id, MemoryWriteStatus::Approved).await;
    assert_eq!(approved["record"]["kind"], json!("profile_fact"));
    assert!(
        approved["record"]["uri"]
            .as_str()
            .unwrap()
            .starts_with("memory://profile/brian/facts/")
    );
    let facts = store.profile_facts("brian").await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].predicate, "prefers");
    assert_eq!(facts[0].object, "approval-backed state capture summaries");
    assert_eq!(facts[0].provenance, "personal-assistant-state-capture");
    assert_eq!(facts[0].importance, 0.72);
}

#[tokio::test]
async fn personal_assistant_reminder_capture_persists_approved_recall_chunk() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;

    post_user_message(
        &app,
        session_id,
        "Remind me to review the P2 acceptance checklist by Friday.",
    )
    .await;

    let pending = wait_for_event_payload(&store, session_id, "write_proposal").await;
    assert_eq!(pending["kind"], json!("memory"));
    assert_eq!(pending["memoryKind"], json!("recall_chunk"));
    assert_eq!(pending["status"], json!("pending"));
    assert_eq!(
        pending["preview"],
        json!("Reminder: review the P2 acceptance checklist by Friday")
    );
    assert!(pending.get("text").is_none());
    assert!(pending.get("provenance").is_none());
    let proposal_id = pending["proposalId"].as_str().unwrap().parse().unwrap();
    let candidate = store.evolution_memory_proposal(proposal_id).await.unwrap();
    assert_eq!(
        candidate.provenance["capturedCategory"],
        "personal_reminder"
    );
    assert_eq!(candidate.importance_score, 0.64);
    assert!(
        store
            .recall_chunks("global", "P2 acceptance checklist", 5)
            .await
            .unwrap()
            .is_empty()
    );

    let approval = wait_for_event_payload(&store, session_id, "approval").await;
    assert_eq!(approval["backend"], json!("memory"));
    let approval_id = approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_test_approval(&app, session_id, approval_id, "approve").await;

    let approved =
        wait_for_write_proposal_status(&store, session_id, MemoryWriteStatus::Approved).await;
    assert_eq!(approved["record"]["kind"], json!("recall_chunk"));
    let record_uri = approved["record"]["uri"].as_str().unwrap();
    assert!(record_uri.starts_with("memory://scopes/global/chunks/"));
    let chunks = store
        .recall_chunks("global", "P2 acceptance checklist", 5)
        .await
        .unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(
        chunks[0].text,
        "Reminder: review the P2 acceptance checklist by Friday"
    );
    assert_eq!(chunks[0].importance, 0.64);

    let root_json = get_session_resource_json(&app, session_id, "resolve", "memory://root").await;
    assert!(
        root_json["content"]
            .as_str()
            .unwrap()
            .contains("P2 acceptance checklist")
    );
}

#[tokio::test]
async fn denied_personal_assistant_reminder_capture_does_not_persist() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;

    post_user_message(&app, session_id, "Reminder: send the P2 notes by Friday.").await;

    let pending = wait_for_event_payload(&store, session_id, "write_proposal").await;
    assert_eq!(pending["memoryKind"], json!("recall_chunk"));
    assert_eq!(pending["status"], json!("pending"));
    let approval = wait_for_event_payload(&store, session_id, "approval").await;
    let approval_id = approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_test_approval(&app, session_id, approval_id, "deny").await;

    let denied =
        wait_for_write_proposal_status(&store, session_id, MemoryWriteStatus::Denied).await;
    assert_eq!(denied["record"], json!(null));
    assert!(
        store
            .recall_chunks("global", "P2 notes", 5)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn personal_assistant_state_capture_does_not_propose_sensitive_or_transient_memory() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    post_user_message(&app, session.id, "Please remember my password is hunter2.").await;
    post_user_message(
        &app,
        session.id,
        "Reminder: rotate sk-testsecret123456 tomorrow.",
    )
    .await;
    post_user_message(
        &app,
        session.id,
        "Just venting: that meeting was annoying and I am grumpy.",
    )
    .await;

    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "write_proposal"),
        "sensitive/transient personal-assistant prompts should not emit memory proposals"
    );
    assert!(
        events.iter().all(|event| event.event_type != "approval"
            || event.payload_json["backend"] != json!("memory")),
        "skipped capture should not request memory approval"
    );
    assert!(store.profile_facts("brian").await.unwrap().is_empty());
    assert!(
        store
            .recall_chunks("global", "password", 5)
            .await
            .unwrap()
            .is_empty()
    );
}

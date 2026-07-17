use super::*;

#[tokio::test]
async fn host_missing_grant_fails_closed() {
    let (_dir, store) = store();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store, None);
    let ctx = InvocationCtx::new(CapabilityGrants::default());
    let err = host
        .invoke("drive.put", json!({ "content": "hello" }), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, HostError::CapabilityDenied("drive.put".to_string()));
}

#[tokio::test]
async fn auto_put_requires_policy_before_write() {
    let (_dir, store) = store();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store.clone(), None);

    let timeout_ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow("drive.put"),
        Arc::new(DefaultDenyApprovalPolicy),
        std::time::Duration::from_millis(1),
    );
    let err = host
        .invoke(
            "drive.put",
            json!({
                "content": "hello",
                "options": {
                    "auto": true,
                    "suggestedPath": "notes/a.txt"
                }
            }),
            &timeout_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalTimeout(_)));
    assert!(store.get("notes/a.txt").is_err());
    assert!(store.proposals().is_empty());

    let denied_ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow("drive.put"),
        Arc::new(StaticApproval(ApprovalDecision::Denied)),
        std::time::Duration::from_secs(1),
    );
    let err = host
        .invoke(
            "drive.put",
            json!({
                "content": "hello",
                "options": {
                    "auto": true,
                    "suggestedPath": "notes/a.txt"
                }
            }),
            &denied_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalDenied(_)));
    assert!(store.get("notes/a.txt").is_err());

    let approved_ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow("drive.put"),
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        std::time::Duration::from_secs(1),
    );
    let approved = host
        .invoke(
            "drive.put",
            json!({
                "content": "hello",
                "options": {
                    "auto": true,
                    "suggestedPath": "notes/a.txt"
                }
            }),
            &approved_ctx,
        )
        .await
        .unwrap();
    assert_eq!(approved["filed"], json!(true));
    assert!(store.get("notes/a.txt").is_ok());

    let err = host
        .invoke(
            "drive.put",
            json!({
                "content": "low risk",
                "options": {
                    "auto": true,
                    "approvalMode": "auto",
                    "suggestedPath": "notes/b.txt"
                }
            }),
            &timeout_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalTimeout(_)));
    assert!(store.get("notes/b.txt").is_err());
}

#[tokio::test]
async fn host_drive_put_accepts_blob_refs_and_rejects_other_uri_refs() {
    let (_dir, store) = store();
    let blob_uri = store.artifacts.put_blob(b"from blob ref").unwrap();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store.clone(), None);
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("drive.put"));

    let by_object = host
        .invoke(
            "drive.put",
            json!({
                "content": { "uri": blob_uri.clone() },
                "options": { "suggestedPath": "notes/blob-object.txt" }
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(by_object["filed"], json!(true));
    assert_eq!(
        String::from_utf8(store.read("notes/blob-object.txt").unwrap().bytes).unwrap(),
        "from blob ref"
    );

    let by_string = host
        .invoke(
            "drive.put",
            json!({
                "content": blob_uri,
                "options": { "suggestedPath": "notes/blob-string.txt" }
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(by_string["filed"], json!(true));
    assert_eq!(
        String::from_utf8(store.read("notes/blob-string.txt").unwrap().bytes).unwrap(),
        "from blob ref"
    );

    let err = host
        .invoke(
            "drive.put",
            json!({
                "content": { "uri": "artifact://0" },
                "options": { "suggestedPath": "notes/artifact-pointer.txt" }
            }),
            &ctx,
        )
        .await
        .unwrap_err();
    let HostError::InvalidArgs(message) = err else {
        panic!("expected invalid args for unsupported uri ref, got {err:?}");
    };
    assert!(message.contains("content.uri only supports blob:sha256"));
    assert!(store.get("notes/artifact-pointer.txt").is_err());
}

#[test]
fn trusted_direct_auto_put_can_file_without_host_approval() {
    let (_dir, store) = store();
    let result = store
        .put_bytes(
            b"trusted server import",
            DrivePutOptions {
                auto: true,
                approval_mode: crate::DriveApprovalMode::Auto,
                suggested_path: Some("imports/trusted.txt".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    assert_eq!(result.entry.path, "imports/trusted.txt");
    assert!(store.get("imports/trusted.txt").is_ok());
}

#[tokio::test]
async fn dropped_file_auto_put_records_approval_provenance_and_replay_event() {
    let (_dir, store) = store();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store.clone(), None);
    let events = Arc::new(RecordingHostEventSink::default());
    let ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow("drive.put"),
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        std::time::Duration::from_secs(1),
    )
    .with_session_id("session-drop")
    .with_session_scope("project:tempestmiku")
    .with_event_sink(events.clone());

    let result = host
        .invoke(
            "drive.put",
            json!({
                "content": "# Dropped Brief\nfile this approved drop",
                "options": {
                    "auto": true,
                    "suggestedPath": "inbox/drop.md",
                    "project": "TempestMiku",
                    "docKind": "note",
                    "sourceUri": "drop://browser/drop.md",
                    "eventSeq": 17
                }
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(result["filed"], json!(true));
    assert_eq!(result["entry"]["uri"], json!("drive://inbox/drop.md"));
    assert_eq!(
        result["entry"]["sourceUri"],
        json!("drop://browser/drop.md")
    );
    assert_eq!(
        result["entry"]["provenance"][0]["sourceUri"],
        json!("drop://browser/drop.md")
    );
    assert_eq!(
        result["entry"]["provenance"][0]["sessionId"],
        json!("session-drop")
    );
    assert_eq!(result["entry"]["provenance"][0]["eventSeq"], json!(17));
    assert_eq!(
        result["entry"]["provenance"][0]["contentHash"],
        result["entry"]["contentHash"]
    );

    let events = events.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "drive_put");
    assert_eq!(events[0].1["sourceUri"], json!("drop://browser/drop.md"));
    assert_eq!(events[0].1["uri"], json!("drive://inbox/drop.md"));
    assert_eq!(
        events[0].1["preview"]["title"],
        json!("Filed drive document")
    );
    assert_eq!(
        events[0].1["resourceRefs"][0]["uri"],
        json!("drive://inbox/drop.md")
    );
}

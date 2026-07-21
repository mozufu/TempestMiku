use super::*;

#[tokio::test]
async fn drive_organize_apply_is_approval_gated_and_updates_status() {
    let (_dir, store) = store();
    store
        .put_bytes(
            b"# Raw\norganizer should move this into project notes",
            DrivePutOptions {
                suggested_path: Some("inbox/raw.md".to_string()),
                project: Some("TempestMiku".to_string()),
                doc_kind: Some("note".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store.clone(), None);

    let denied_ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow("drive.organize"),
        Arc::new(StaticApproval(ApprovalDecision::Denied)),
        std::time::Duration::from_secs(1),
    );
    let err = host
        .invoke("drive.organize", json!({ "apply": true }), &denied_ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalDenied(_)));
    let proposals = store.proposals();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].status, ProposalStatus::Denied);
    assert!(store.get("inbox/raw.md").is_ok());
    assert!(
        store
            .get(proposals[0].proposed_path.as_deref().unwrap())
            .is_err()
    );

    let approved_ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow("drive.organize"),
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        std::time::Duration::from_secs(1),
    );
    let applied = host
        .invoke("drive.organize", json!({ "apply": true }), &approved_ctx)
        .await
        .unwrap();
    assert_eq!(applied.as_array().unwrap().len(), 1);
    assert_eq!(applied[0]["status"], json!("applied"));
    let proposed_path = applied[0]["proposedPath"].as_str().unwrap();
    assert!(store.get("inbox/raw.md").is_err());
    assert_eq!(store.get(proposed_path).unwrap().path, proposed_path);
}

#[tokio::test]
async fn drive_organize_emits_replayable_events_with_resource_refs() {
    let (_dir, store) = store();
    store
        .put_bytes(
            b"# Raw\norganizer should move this into project notes",
            DrivePutOptions {
                suggested_path: Some("inbox/raw.md".to_string()),
                project: Some("TempestMiku".to_string()),
                doc_kind: Some("note".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store.clone(), None);
    let events = Arc::new(RecordingHostEventSink::default());
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("drive.organize"))
        .with_event_sink(events.clone());

    let proposals = host
        .invoke("drive.organize", json!({}), &ctx)
        .await
        .unwrap();
    assert_eq!(proposals.as_array().unwrap().len(), 1);

    let events = events.events();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].0, "drive_organizer_started");
    assert_eq!(events[0].1["apply"], json!(false));
    assert_eq!(events[1].0, "write_proposal");
    assert_eq!(events[1].1["kind"], json!("drive"));
    assert_eq!(events[1].1["status"], json!("pending"));
    assert_eq!(events[1].1["sourceUri"], json!("drive://inbox/raw.md"));
    assert_eq!(
        events[1].1["proposedUri"],
        json!("drive://projects/tempestmiku/note/raw.md")
    );
    assert_eq!(events[2].0, "drive_organizer_completed");
    assert_eq!(events[2].1["proposalCount"], json!(1));
    assert_eq!(events[2].1["proposals"][0]["status"], json!("pending"));
    assert_eq!(
        events[2].1["proposals"][0]["sourceUri"],
        json!("drive://inbox/raw.md")
    );
    assert_eq!(
        events[2].1["proposals"][0]["proposedUri"],
        json!("drive://projects/tempestmiku/note/raw.md")
    );
    assert_eq!(
        events[2].1["proposals"][0]["resourceRefs"][0]["role"],
        json!("source")
    );
    assert_eq!(
        events[2].1["proposals"][0]["resourceRefs"][1]["role"],
        json!("proposed")
    );
    assert!(
        events[2].1["proposals"][0]["preview"]["subtitle"]
            .as_str()
            .unwrap()
            .contains("inbox/raw.md -> projects/tempestmiku/note/raw.md")
    );
    assert_eq!(
        events[2].1["resourceRefs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value["uri"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "drive://inbox/raw.md",
            "drive://projects/tempestmiku/note/raw.md"
        ]
    );
}

#[tokio::test]
async fn drive_organize_apply_marks_stale_sources_without_mutation() {
    let (_dir, store) = store();
    store
        .put_bytes(
            b"# Raw\norganizer should move this into project notes",
            DrivePutOptions {
                suggested_path: Some("inbox/raw.md".to_string()),
                project: Some("TempestMiku".to_string()),
                doc_kind: Some("note".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let proposal = store.organize().unwrap().remove(0);
    let proposed_path = proposal.proposed_path.clone().unwrap();
    store
        .move_entry(
            "inbox/raw.md",
            "manual/raw.md",
            DriveCollisionStrategy::Reject,
        )
        .unwrap();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store.clone(), None);
    let approved_ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow("drive.organize"),
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        std::time::Duration::from_secs(1),
    );

    let applied = host
        .invoke("drive.organize", json!({ "apply": true }), &approved_ctx)
        .await
        .unwrap();
    assert_eq!(applied.as_array().unwrap().len(), 1);
    assert_eq!(applied[0]["id"], json!(proposal.id));
    assert_eq!(applied[0]["status"], json!("stale"));
    assert!(store.get("manual/raw.md").is_ok());
    assert!(store.get(&proposed_path).is_err());
}

#[tokio::test]
async fn drive_organize_apply_timeout_marks_failed_without_mutation() {
    let (_dir, store) = store();
    store
        .put_bytes(
            b"# Raw Note\norganizer should move this into project notes",
            DrivePutOptions {
                suggested_path: Some("inbox/raw.md".to_string()),
                project: Some("TempestMiku".to_string()),
                doc_kind: Some("note".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store.clone(), None);
    let timeout_ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow("drive.organize"),
        Arc::new(DefaultDenyApprovalPolicy),
        std::time::Duration::from_millis(1),
    );

    let err = host
        .invoke("drive.organize", json!({ "apply": true }), &timeout_ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalTimeout(_)));
    let proposals = store.proposals();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].status, ProposalStatus::Failed);
    assert!(store.get("inbox/raw.md").is_ok());
    assert!(
        store
            .get(proposals[0].proposed_path.as_deref().unwrap())
            .is_err()
    );
}

#[tokio::test]
async fn drive_organize_apply_collision_fails_without_partial_metadata_write() {
    let (_dir, store) = store();
    store
        .put_bytes(
            b"# Raw\norganizer should move this into project notes",
            DrivePutOptions {
                suggested_path: Some("inbox/raw.md".to_string()),
                project: Some("TempestMiku".to_string()),
                doc_kind: Some("note".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    store
        .put_bytes(
            b"# Existing Note\nalready owns the proposed path",
            DrivePutOptions {
                suggested_path: Some("projects/tempestmiku/note/raw.md".to_string()),
                project: Some("TempestMiku".to_string()),
                doc_kind: Some("note".to_string()),
                title: Some("Raw".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store.clone(), None);
    let approved_ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow("drive.organize"),
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        std::time::Duration::from_secs(1),
    );

    let applied = host
        .invoke("drive.organize", json!({ "apply": true }), &approved_ctx)
        .await
        .unwrap();
    assert_eq!(applied.as_array().unwrap().len(), 1);
    assert_eq!(applied[0]["status"], json!("failed"));
    assert_eq!(store.get("inbox/raw.md").unwrap().path, "inbox/raw.md");
    let existing = store.read("projects/tempestmiku/note/raw.md").unwrap();
    assert_eq!(
        String::from_utf8(existing.bytes).unwrap(),
        "# Existing Note\nalready owns the proposed path"
    );
}

#[tokio::test]
async fn mutating_approval_timeout_writes_nothing() {
    let (_dir, store) = store();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store.clone(), None);
    let ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow_many(["drive.put", "drive.move"]),
        Arc::new(DefaultDenyApprovalPolicy),
        std::time::Duration::from_millis(1),
    );
    host.invoke(
        "drive.put",
        json!({
            "content": "hello",
            "options": { "suggestedPath": "notes/a.txt" }
        }),
        &ctx,
    )
    .await
    .unwrap();
    let err = host
        .invoke(
            "drive.move",
            json!({ "from": "notes/a.txt", "to": "notes/b.txt" }),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalTimeout(_)));
    assert!(store.get("notes/a.txt").is_ok());
    assert!(store.get("notes/b.txt").is_err());
}

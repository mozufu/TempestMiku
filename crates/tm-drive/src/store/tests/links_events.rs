use super::*;

#[tokio::test]
async fn drive_link_registers_shared_policy_only_after_approval() {
    let (_dir, store) = store();
    let project = tempfile::tempdir().unwrap();
    let linked = LinkedFolders::default();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(
        &mut host,
        &mut resources,
        store.clone(),
        Some(linked.clone()),
    );

    let timeout_ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow_many(["project.link", "project.unlink"]),
        Arc::new(DefaultDenyApprovalPolicy),
        std::time::Duration::from_millis(1),
    );
    let err = host
        .invoke(
            "project.link",
            json!({
                "hostPath": project.path(),
                "mode": "ro",
                "project": "Tempest Miku"
            }),
            &timeout_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalTimeout(_)));
    assert!(linked.policy("tempest-miku").is_err());

    let approved_ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow_many(["project.link", "project.unlink"]),
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        std::time::Duration::from_secs(1),
    );
    let linked_value = host
        .invoke(
            "project.link",
            json!({
                "hostPath": project.path(),
                "mode": "rw",
                "project": "Tempest Miku"
            }),
            &approved_ctx,
        )
        .await
        .unwrap();
    assert_eq!(linked_value["linkedUri"], json!("linked://tempest-miku/"));
    let policy = linked.policy("tempest-miku").unwrap();
    assert_eq!(policy.root, project.path().canonicalize().unwrap());
    assert_eq!(policy.mode, tm_host::FsMode::Rw);

    host.invoke(
        "project.link",
        json!({
            "hostPath": project.path(),
            "mode": "ro",
            "project": "Tempest Miku"
        }),
        &approved_ctx,
    )
    .await
    .unwrap();
    let policy = linked.policy("tempest-miku").unwrap();
    assert_eq!(policy.mode, tm_host::FsMode::Ro);

    let denied_ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow_many(["project.link", "project.unlink"]),
        Arc::new(StaticApproval(ApprovalDecision::Denied)),
        std::time::Duration::from_secs(1),
    );
    let other = tempfile::tempdir().unwrap();
    let err = host
        .invoke(
            "project.link",
            json!({
                "hostPath": other.path(),
                "mode": "ro",
                "project": "Blocked Project"
            }),
            &denied_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalDenied(_)));
    assert!(linked.policy("blocked-project").is_err());

    let err = host
        .invoke(
            "project.unlink",
            json!({ "alias": "linked://tempest-miku/" }),
            &denied_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalDenied(_)));
    assert!(linked.policy("tempest-miku").is_ok());

    let revoked = host
        .invoke(
            "project.unlink",
            json!({ "alias": "linked://tempest-miku/" }),
            &approved_ctx,
        )
        .await
        .unwrap();
    assert_eq!(revoked["linkedUri"], json!("linked://tempest-miku/"));
    assert_eq!(revoked["memoryScope"], json!("project:tempest-miku"));
    assert!(linked.policy("tempest-miku").is_err());
}

#[tokio::test]
async fn drive_mutations_emit_mobile_friendly_replay_events() {
    let (_dir, store) = store();
    let project = tempfile::tempdir().unwrap();
    let linked = LinkedFolders::default();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store, Some(linked.clone()));
    let events = Arc::new(RecordingHostEventSink::default());
    let ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow_many([
            "drive.put",
            "drive.move",
            "drive.tag",
            "project.link",
            "project.unlink",
        ]),
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        std::time::Duration::from_secs(1),
    )
    .with_event_sink(events.clone());

    host.invoke(
        "drive.put",
        json!({
            "content": "# Raw\nship the mobile event payloads",
            "options": {
                "suggestedPath": "inbox/raw.md",
                "project": "TempestMiku",
                "docKind": "note",
                "tags": ["planning"],
                "sourceUri": "drop://raw.md"
            }
        }),
        &ctx,
    )
    .await
    .unwrap();
    host.invoke(
        "drive.move",
        json!({
            "from": "inbox/raw.md",
            "to": "projects/tempestmiku/note/raw.md"
        }),
        &ctx,
    )
    .await
    .unwrap();
    host.invoke(
        "drive.tag",
        json!({
            "path": "projects/tempestmiku/note/raw.md",
            "tags": ["review"]
        }),
        &ctx,
    )
    .await
    .unwrap();
    host.invoke(
        "project.link",
        json!({
            "hostPath": project.path(),
            "mode": "ro",
            "project": "Tempest Miku"
        }),
        &ctx,
    )
    .await
    .unwrap();
    host.invoke("project.unlink", json!({ "alias": "tempest-miku" }), &ctx)
        .await
        .unwrap();

    let events = events.events();
    let event_types = events
        .iter()
        .map(|(event_type, _)| event_type.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        vec![
            "drive_put",
            "drive_moved",
            "drive_tagged",
            "project_linked",
            "project_unlinked"
        ]
    );
    assert_eq!(events[0].1["uri"], json!("drive://inbox/raw.md"));
    assert_eq!(
        events[0].1["preview"]["title"],
        json!("Filed drive document")
    );
    assert_eq!(
        events[0].1["resourceRefs"][0]["uri"],
        json!("drive://inbox/raw.md")
    );
    assert_eq!(events[1].1["fromUri"], json!("drive://inbox/raw.md"));
    assert_eq!(
        events[1].1["toUri"],
        json!("drive://projects/tempestmiku/note/raw.md")
    );
    assert_eq!(events[1].1["resourceRefs"][1]["role"], json!("current"));
    assert_eq!(events[2].1["tags"], json!(["note", "planning", "review"]));
    assert_eq!(
        events[2].1["preview"]["subtitle"],
        json!("projects/tempestmiku/note/raw.md")
    );
    assert_eq!(events[3].1["linkedUri"], json!("linked://tempest-miku/"));
    assert_eq!(
        events[3].1["resourceRefs"][0]["kind"],
        json!("linked_folder")
    );
    assert_eq!(events[4].1["linkedUri"], json!("linked://tempest-miku/"));
    assert_eq!(
        events[4].1["preview"]["title"],
        json!("Unlinked project folder")
    );
}

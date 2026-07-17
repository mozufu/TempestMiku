use super::*;

use sha2::Digest;
use tm_host::{
    ApprovalDecision, ApprovalPolicy, CapabilityGrants, DefaultDenyApprovalPolicy, HostEventSink,
};

#[derive(Default)]
struct RecordingHostEventSink {
    events: parking_lot::Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl HostEventSink for RecordingHostEventSink {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        self.events
            .lock()
            .push((event_type.to_string(), payload_json));
        Ok(())
    }
}

impl RecordingHostEventSink {
    fn events(&self) -> Vec<(String, Value)> {
        self.events.lock().clone()
    }
}

struct StaticApproval(ApprovalDecision);

#[async_trait]
impl ApprovalPolicy for StaticApproval {
    async fn request(
        &self,
        _action: &str,
        _timeout: std::time::Duration,
    ) -> tm_host::Result<ApprovalDecision> {
        Ok(self.0)
    }
}

fn store() -> (tempfile::TempDir, InMemoryDriveStore) {
    let dir = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(dir.path(), "drive").unwrap();
    let store = InMemoryDriveStore::new(artifacts);
    (dir, store)
}

#[test]
fn rejects_raw_host_and_traversal_paths() {
    assert!(normalize_canonical_path("/Users/brian/file.txt").is_err());
    assert!(normalize_canonical_path("C:/Users/brian/file.txt").is_err());
    assert!(normalize_canonical_path("notes/../secret.txt").is_err());
    assert_eq!(
        normalize_canonical_path("drive://notes/./a.txt").unwrap(),
        "notes/a.txt"
    );
}

#[test]
fn put_get_search_and_virtual_dirs_work_offline() {
    let (_dir, store) = store();
    let result = store
        .put_bytes(
            b"# Invoice 42\nDate 2026-07-08\nAmount due $42.00",
            DrivePutOptions {
                auto: true,
                tags: vec!["tax".to_string()],
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    assert_eq!(result.entry.path, "finance/2026/invoice/invoice-42.txt");
    store
        .put_bytes(
            b"# Meeting Notes\nRecent unrelated project chatter",
            DrivePutOptions {
                suggested_path: Some("notes/recent.md".to_string()),
                doc_kind: Some("note".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();

    let content = store.resource_content(&result.uri, Some("1-1")).unwrap();
    assert_eq!(content.content, "# Invoice 42");
    assert!(content.has_more);

    let hits = store
        .search(DriveSearchOptions {
            query: Some("invoice".to_string()),
            return_snippets: true,
            ..DriveSearchOptions::default()
        })
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].uri, result.uri);
    assert!(hits[0].snippet.as_ref().unwrap().contains("Invoice"));

    let by_type = store
        .list(DriveListOptions {
            path: Some("/by-type/invoice".to_string()),
            recursive: true,
            ..DriveListOptions::default()
        })
        .unwrap();
    assert_eq!(by_type.len(), 1);
}

#[test]
fn drive_redacts_text_secrets_before_hashing_and_blob_persistence() {
    let (_dir, store) = store();
    let raw_secret = "sk-testsecret123456";
    let filed = store
        .put_bytes(
            format!("# Deployment note\ntoken {raw_secret}").as_bytes(),
            DrivePutOptions {
                suggested_path: Some("notes/deployment.md".to_string()),
                title: Some(format!("Deployment {raw_secret}")),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();

    let read = store.read(&filed.uri).unwrap();
    let persisted = String::from_utf8(read.bytes).unwrap();
    assert!(!persisted.contains(raw_secret));
    assert!(persisted.contains("[REDACTED_TOKEN]"));
    assert!(!filed.entry.title.as_deref().unwrap().contains(raw_secret));
    assert_eq!(
        filed.entry.content_hash,
        hex::encode(sha2::Sha256::digest(persisted.as_bytes()))
    );
}

#[test]
fn drive_rejects_secrets_in_authority_and_identifier_metadata() {
    let (_dir, store) = store();
    for options in [
        DrivePutOptions {
            suggested_path: Some("notes/sk-testsecret123456.md".to_string()),
            ..DrivePutOptions::default()
        },
        DrivePutOptions {
            project: Some("sk-testsecret123456".to_string()),
            ..DrivePutOptions::default()
        },
        DrivePutOptions {
            source_uri: Some("https://owner:database-password@example.test/source".to_string()),
            ..DrivePutOptions::default()
        },
        DrivePutOptions {
            tags: vec!["sk-testsecret123456".to_string()],
            ..DrivePutOptions::default()
        },
    ] {
        let error = store.put_bytes(b"safe", options).unwrap_err().to_string();
        assert!(error.contains("sensitive data"), "{error}");
    }
}

#[test]
fn drive_rejects_detectable_secrets_in_non_utf8_content() {
    let (_dir, store) = store();
    let mut bytes = vec![0xff];
    bytes.extend_from_slice(b" sk-testsecret123456");
    let error = store
        .put_bytes(&bytes, DrivePutOptions::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("secret detector"));
}

#[test]
fn drive_put_classifies_representative_documents_without_llm() {
    let (_dir, store) = store();
    let cases = [
        (
            b"Meeting note\nFollow up with Miku about drive approvals.".as_slice(),
            "note",
            "notes/meeting.txt",
        ),
        (
            b"Receipt\nTotal paid: $18.25\nDate: 2026-07-08".as_slice(),
            "receipt",
            "receipts/lunch.txt",
        ),
        (
            b"Abstract\nA small agent runtime study.\nDOI: 10.1000/example\nReferences".as_slice(),
            "paper",
            "papers/runtime.txt",
        ),
        (
            b"Roadmap\nMilestone P5 adds drive and research workspace.".as_slice(),
            "project_doc",
            "projects/tempestmiku/roadmap.txt",
        ),
    ];

    for (content, expected_kind, path) in cases {
        let filed = store
            .put_bytes(
                content,
                DrivePutOptions {
                    suggested_path: Some(path.to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        assert_eq!(filed.entry.doc_kind.as_deref(), Some(expected_kind));
        assert!(
            filed
                .entry
                .attributes
                .iter()
                .any(|attr| attr.key == "doc_kind" && attr.value == expected_kind)
        );
    }
}

#[test]
fn duplicate_content_reuses_blob_but_allows_distinct_paths() {
    let (_dir, store) = store();
    let one = store
        .put_bytes(
            b"same",
            DrivePutOptions {
                suggested_path: Some("notes/a.txt".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let two = store
        .put_bytes(
            b"same",
            DrivePutOptions {
                suggested_path: Some("notes/b.txt".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    assert_eq!(one.entry.blob_uri, two.entry.blob_uri);
    assert_ne!(one.entry.id, two.entry.id);
}

#[test]
fn missing_get_returns_nearby_drive_paths_without_host_paths() {
    let (dir, store) = store();
    store
        .put_bytes(
            b"alpha",
            DrivePutOptions {
                suggested_path: Some("notes/a.md".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    store
        .put_bytes(
            b"beta",
            DrivePutOptions {
                suggested_path: Some("notes/b.md".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();

    let err = store.get("notes/missing.md").unwrap_err().to_string();
    assert!(err.contains("drive entry not found: notes/missing.md"));
    assert!(err.contains("nearby paths: notes/a.md, notes/b.md"));
    assert!(!err.contains(&dir.path().display().to_string()));
}

#[test]
fn organizer_run_claim_heartbeat_complete_and_stale_reclaim() {
    let (_dir, store) = store();
    let now = Utc::now();
    let queued = store.enqueue_organizer_run("manual", now - Duration::seconds(1));

    let claimed = store
        .claim_ready_organizer_run(now, Duration::seconds(30))
        .unwrap()
        .expect("ready organizer run");
    assert_eq!(claimed.id, queued.id);
    assert_eq!(claimed.status, OrganizerRunStatus::Running);
    assert_eq!(claimed.attempts, 1);
    assert_eq!(claimed.locked_at, Some(now));
    assert!(
        store
            .claim_ready_organizer_run(now + Duration::seconds(10), Duration::seconds(30))
            .unwrap()
            .is_none()
    );

    let reclaimed = store
        .claim_ready_organizer_run(now + Duration::seconds(31), Duration::seconds(30))
        .unwrap()
        .expect("stale organizer run");
    assert_eq!(reclaimed.id, queued.id);
    assert_eq!(reclaimed.status, OrganizerRunStatus::Running);
    assert_eq!(reclaimed.attempts, 2);

    let heartbeat_at = now + Duration::seconds(35);
    let heartbeated = store
        .heartbeat_organizer_run(queued.id, heartbeat_at)
        .unwrap();
    assert_eq!(heartbeated.locked_at, Some(heartbeat_at));

    let completed_at = now + Duration::seconds(40);
    let completed = store
        .complete_organizer_run(queued.id, vec![Uuid::nil()], completed_at)
        .unwrap();
    assert_eq!(completed.status, OrganizerRunStatus::Completed);
    assert_eq!(completed.locked_at, None);
    assert_eq!(completed.completed_at, Some(completed_at));
    assert_eq!(completed.proposal_ids, vec![Uuid::nil()]);
    assert!(
        store
            .claim_ready_organizer_run(now + Duration::seconds(90), Duration::seconds(30))
            .unwrap()
            .is_none()
    );
}

#[test]
fn organizer_run_failure_records_error_and_bounded_retry() {
    let (_dir, store) = store();
    let now = Utc::now();
    let queued = store.enqueue_organizer_run("manual", now);
    let claimed = store
        .claim_ready_organizer_run(now, Duration::seconds(30))
        .unwrap()
        .expect("ready organizer run");
    assert_eq!(claimed.id, queued.id);

    let retry_at = now + Duration::seconds(60);
    let retryable = store
        .fail_organizer_run(queued.id, "transient store error".to_string(), retry_at, 2)
        .unwrap();
    assert_eq!(retryable.status, OrganizerRunStatus::Queued);
    assert_eq!(retryable.locked_at, None);
    assert_eq!(retryable.available_at, retry_at);
    assert_eq!(
        retryable.last_error.as_deref(),
        Some("transient store error")
    );
    assert!(
        store
            .claim_ready_organizer_run(now + Duration::seconds(30), Duration::seconds(30))
            .unwrap()
            .is_none()
    );

    let second = store
        .claim_ready_organizer_run(retry_at, Duration::seconds(30))
        .unwrap()
        .expect("retryable organizer run");
    assert_eq!(second.attempts, 2);
    let terminal = store
        .fail_organizer_run(
            queued.id,
            "terminal policy error".to_string(),
            retry_at + Duration::seconds(60),
            2,
        )
        .unwrap();
    assert_eq!(terminal.status, OrganizerRunStatus::Failed);
    assert_eq!(terminal.locked_at, None);
    assert_eq!(
        terminal.last_error.as_deref(),
        Some("terminal policy error")
    );
    assert!(
        store
            .claim_ready_organizer_run(retry_at + Duration::seconds(90), Duration::seconds(30))
            .unwrap()
            .is_none()
    );
}

#[test]
fn duplicate_organizer_workers_cannot_claim_same_run() {
    let (_dir, store) = store();
    let now = Utc::now();
    let queued = store.enqueue_organizer_run("scheduled", now);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

    let handles = (0..2)
        .map(|_| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .claim_ready_organizer_run(now, Duration::seconds(30))
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let claims = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    let claimed = claims
        .iter()
        .filter(|claim| claim.as_ref().is_some_and(|run| run.id == queued.id))
        .count();

    assert_eq!(claimed, 1);
    assert_eq!(
        store.organizer_runs()[0].status,
        OrganizerRunStatus::Running
    );
}

#[test]
fn drive_organize_records_completed_run_and_proposal_refs() {
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

    let proposals = store.organize().unwrap();
    assert_eq!(proposals.len(), 1);
    let runs = store.organizer_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, OrganizerRunStatus::Completed);
    assert_eq!(runs[0].proposal_ids, vec![proposals[0].id]);
    assert_eq!(proposals[0].source_run_id, runs[0].id);
}

#[test]
fn same_path_collision_keeps_both_by_default() {
    let (_dir, store) = store();
    let one = store
        .put_bytes(
            b"one",
            DrivePutOptions {
                suggested_path: Some("notes/a.txt".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let two = store
        .put_bytes(
            b"two",
            DrivePutOptions {
                suggested_path: Some("notes/a.txt".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    assert_eq!(one.entry.path, "notes/a.txt");
    assert_eq!(two.entry.path, "notes/a-2.txt");
}

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
        CapabilityGrants::default().allow_many(["drive.link", "drive.unlink"]),
        Arc::new(DefaultDenyApprovalPolicy),
        std::time::Duration::from_millis(1),
    );
    let err = host
        .invoke(
            "drive.link",
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
        CapabilityGrants::default().allow_many(["drive.link", "drive.unlink"]),
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        std::time::Duration::from_secs(1),
    );
    let linked_value = host
        .invoke(
            "drive.link",
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
        "drive.link",
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
        CapabilityGrants::default().allow_many(["drive.link", "drive.unlink"]),
        Arc::new(StaticApproval(ApprovalDecision::Denied)),
        std::time::Duration::from_secs(1),
    );
    let other = tempfile::tempdir().unwrap();
    let err = host
        .invoke(
            "drive.link",
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
            "drive.unlink",
            json!({ "alias": "linked://tempest-miku/" }),
            &denied_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalDenied(_)));
    assert!(linked.policy("tempest-miku").is_ok());

    let revoked = host
        .invoke(
            "drive.unlink",
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
            "drive.link",
            "drive.unlink",
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
        "drive.link",
        json!({
            "hostPath": project.path(),
            "mode": "ro",
            "project": "Tempest Miku"
        }),
        &ctx,
    )
    .await
    .unwrap();
    host.invoke("drive.unlink", json!({ "alias": "tempest-miku" }), &ctx)
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
            "drive_linked",
            "drive_unlinked"
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
async fn host_drive_organize_rejects_auto_apply_config() {
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
    let ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow("drive.organize"),
        Arc::new(DefaultDenyApprovalPolicy),
        std::time::Duration::from_millis(1),
    );

    let err = host
        .invoke(
            "drive.organize",
            json!({
                "config": {
                    "tier": "conservative",
                    "autoApply": [{
                        "actions": ["move"],
                        "docKinds": ["note"],
                        "projects": ["TempestMiku"],
                        "minConfidence": 0.7
                    }]
                }
            }),
            &ctx,
        )
        .await
        .unwrap_err();
    let HostError::InvalidArgs(message) = err else {
        panic!("expected invalid args for host autoApply config, got {err:?}");
    };
    assert!(message.contains("autoApply"));
    assert!(store.get("inbox/raw.md").is_ok());
    assert!(store.proposals().is_empty());

    let err = host
        .invoke(
            "drive.organize",
            json!({ "config": { "tier": "moderate" } }),
            &ctx,
        )
        .await
        .unwrap_err();
    let HostError::InvalidArgs(message) = err else {
        panic!("expected invalid args for host non-conservative tier, got {err:?}");
    };
    assert!(message.contains("conservative tier"));
    assert!(store.get("inbox/raw.md").is_ok());
    assert!(store.proposals().is_empty());
}

#[test]
fn trusted_store_drive_organize_auto_apply_is_tier_and_rule_gated() {
    let (_dir, conservative_store) = store();
    conservative_store
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
    let conservative = conservative_store
        .organize_with_config(DriveOrganizerConfig {
            tier: DriveAutomationTier::Conservative,
            auto_apply: vec![crate::DriveOrganizerAutoApplyRule {
                actions: vec![OrganizerActionKind::Move],
                doc_kinds: vec!["note".to_string()],
                projects: vec!["TempestMiku".to_string()],
                min_confidence: 0.7,
            }],
        })
        .unwrap();
    assert_eq!(conservative[0].status, ProposalStatus::Pending);
    assert_eq!(
        conservative[0].policy_decision,
        PolicyDecision::ApprovalRequired
    );
    assert!(conservative_store.get("inbox/raw.md").is_ok());
    assert!(
        conservative_store
            .get(conservative[0].proposed_path.as_deref().unwrap())
            .is_err()
    );

    let (_dir, moderate_store) = store();
    moderate_store
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
    let applied = moderate_store
        .organize_with_config(DriveOrganizerConfig {
            tier: DriveAutomationTier::Moderate,
            auto_apply: vec![crate::DriveOrganizerAutoApplyRule {
                actions: vec![OrganizerActionKind::Move],
                doc_kinds: vec!["note".to_string()],
                projects: vec!["TempestMiku".to_string()],
                min_confidence: 0.7,
            }],
        })
        .unwrap();
    assert_eq!(applied[0].status, ProposalStatus::Applied);
    assert_eq!(applied[0].policy_decision, PolicyDecision::AutoApply);
    let proposed_path = applied[0].proposed_path.as_deref().unwrap();
    assert!(moderate_store.get("inbox/raw.md").is_err());
    assert_eq!(
        moderate_store.get(proposed_path).unwrap().path,
        proposed_path
    );

    let (_dir, strict_store) = store();
    strict_store
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
    let strict = strict_store
        .organize_with_config(DriveOrganizerConfig {
            tier: DriveAutomationTier::Moderate,
            auto_apply: vec![crate::DriveOrganizerAutoApplyRule {
                actions: vec![OrganizerActionKind::Move],
                doc_kinds: vec!["note".to_string()],
                projects: vec!["TempestMiku".to_string()],
                min_confidence: 0.95,
            }],
        })
        .unwrap();
    assert_eq!(strict[0].status, ProposalStatus::Pending);
    assert_eq!(strict[0].policy_decision, PolicyDecision::ApprovalRequired);
    assert!(strict_store.get("inbox/raw.md").is_ok());
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
    assert_eq!(events[0].1["tier"], json!("conservative"));
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
async fn drive_host_calls_require_exact_authoritative_project_scope() {
    let (_dir, store) = store();
    for project in ["alpha", "beta"] {
        store
            .put_bytes(
                format!("# {project}").as_bytes(),
                DrivePutOptions {
                    suggested_path: Some(format!("projects/{project}/note.md")),
                    project: Some(project.to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
    }
    store
        .put_bytes(
            b"# global note",
            DrivePutOptions {
                suggested_path: Some("notes/global.md".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store, None);
    let session_id = Uuid::new_v4().to_string();

    let global = InvocationCtx::new(CapabilityGrants::default().allow("drive.search"))
        .with_session_id(session_id.clone())
        .with_session_scope("global");
    let global_results = host
        .invoke("drive.search", json!({"query": "global"}), &global)
        .await
        .unwrap();
    assert_eq!(global_results.as_array().unwrap().len(), 1);
    assert!(global_results[0]["project"].is_null());
    assert!(matches!(
        host.invoke(
            "drive.search",
            json!({"query": "alpha", "project": "alpha"}),
            &global,
        )
        .await,
        Err(HostError::CapabilityDenied(_))
    ));

    let alpha = InvocationCtx::new(CapabilityGrants::default().allow("drive.search"))
        .with_session_id(session_id)
        .with_session_scope("project:alpha");
    assert!(matches!(
        host.invoke(
            "drive.search",
            json!({"query": "beta", "project": "beta"}),
            &alpha,
        )
        .await,
        Err(HostError::CapabilityDenied(_))
    ));
    let results = host
        .invoke("drive.search", json!({"query": "note"}), &alpha)
        .await
        .unwrap();
    assert_eq!(results.as_array().unwrap().len(), 1);
    assert_eq!(results[0]["project"], json!("alpha"));
}

#[tokio::test]
async fn research_drive_returns_bounded_local_digests_and_citations() {
    let (_dir, store) = store();
    store
        .put_bytes(
            b"# Approval Drop\nManual approval gates drive writes.\nExtra detail.",
            DrivePutOptions {
                suggested_path: Some("projects/tempestmiku/approval.md".to_string()),
                project: Some("tempestmiku".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store, None);
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("research.drive"))
        .with_session_id(Uuid::new_v4().to_string())
        .with_session_scope("project:tempestmiku");

    let result = host
        .invoke(
            "research.drive",
            json!({
                "query": "approval",
                "project": "tempestmiku",
                "maxDocs": 1,
                "maxSnippets": 1,
                "maxWorkers": 0,
                "maxBytesPerDoc": 80,
                "maxDigestBytes": 80
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(result["corpus"].as_array().unwrap().len(), 1);
    assert_eq!(result["digests"].as_array().unwrap().len(), 1);
    assert_eq!(result["citations"].as_array().unwrap().len(), 1);
    assert_eq!(result["citations"][0]["sourceKind"], json!("drive"));
    assert!(result["answer"].as_str().unwrap().contains("drive://"));
    assert_eq!(result["budget"]["maxWorkers"], json!(0));
    assert_eq!(result["budget"]["agentDocs"], json!(0));
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

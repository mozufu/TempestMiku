use super::*;

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

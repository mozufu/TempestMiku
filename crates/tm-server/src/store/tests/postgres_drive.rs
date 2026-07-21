use super::*;

#[tokio::test]
async fn gated_postgres_bootstraps_drive_schema() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    for table in [
        "drive_entries",
        "drive_attributes",
        "drive_tags",
        "drive_proposals",
        "drive_links",
        "drive_organizer_runs",
        "drive_corrections",
        "drive_entry_tombstones",
    ] {
        let name = format!("public.{table}");
        let exists: bool = store
            .client()
            .query_one("select to_regclass($1) is not null", &[&name])
            .await
            .unwrap()
            .get(0);
        assert!(exists, "{table} should exist");
    }

    for index in [
        "drive_entries_hash_idx",
        "drive_entries_project_idx",
        "drive_entries_doc_kind_idx",
        "drive_entries_search_fts_idx",
        "drive_tags_tag_idx",
        "drive_proposals_status_updated_idx",
        "drive_links_status_updated_idx",
        "drive_organizer_runs_ready_idx",
        "drive_entry_tombstones_path_idx",
    ] {
        let name = format!("public.{index}");
        let exists: bool = store
            .client()
            .query_one("select to_regclass($1) is not null", &[&name])
            .await
            .unwrap()
            .get(0);
        assert!(exists, "{index} should exist");
    }

    let run_id = Uuid::new_v4();
    let project = format!("Pg Drive {run_id}");
    let source_path = format!("inbox/{run_id}.md");
    let source_uri = format!("drop://postgres/{run_id}.md");
    let artifacts = tempfile::tempdir().unwrap();
    let in_memory = tm_drive::InMemoryDriveStore::new(
        tm_artifacts::ArtifactStore::open(artifacts.path(), "drive").unwrap(),
    );
    in_memory
        .put_bytes(
            b"# Pg Drive Brief\nApproval gates durable drive writes.",
            tm_drive::DrivePutOptions {
                suggested_path: Some(source_path.clone()),
                project: Some(project.clone()),
                doc_kind: Some("note".to_string()),
                source_uri: Some(source_uri.clone()),
                event_seq: Some(23),
                ..tm_drive::DrivePutOptions::default()
            },
        )
        .unwrap();
    let proposals = in_memory.organize().unwrap();
    assert_eq!(proposals.len(), 1);
    let applied = in_memory
        .apply_organizer_proposals(&[proposals[0].id])
        .unwrap();
    assert_eq!(applied[0].status, tm_drive::ProposalStatus::Applied);
    let final_path = applied[0].proposed_path.as_deref().unwrap().to_string();
    let tagged = in_memory
        .tag_entry(&final_path, vec!["review".to_string()])
        .unwrap();
    let memory_snapshot = LogicalDriveSnapshot::from_memory(&tagged, &applied[0]);

    insert_postgres_drive_snapshot(&store, &tagged, &applied[0]).await;
    let postgres_snapshot = postgres_drive_snapshot(&store, &tagged.uri, applied[0].id).await;
    assert_eq!(postgres_snapshot, memory_snapshot);

    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "postgres drive replay".to_string(),
            },
        })
        .await
        .unwrap();
    let drive_event = store
        .append_event(
            session.id,
            "drive_put",
            json!({
                "uri": tagged.uri,
                "path": tagged.path,
                "contentHash": tagged.content_hash,
                "sourceUri": tagged.source_uri,
                "resourceRefs": [{
                    "role": "document",
                    "uri": tagged.uri,
                    "kind": "drive_document",
                    "title": tagged.title,
                }]
            }),
        )
        .await
        .unwrap();
    let replay = store
        .events_after(session.id, drive_event.seq.checked_sub(1))
        .await
        .unwrap();
    assert!(replay.iter().any(|event| {
        event.event_type == "drive_put" && event.payload_json["uri"] == json!(tagged.uri)
    }));
}

#[tokio::test]
async fn gated_postgres_drive_metadata_survives_restart_and_revocation() {
    use tm_drive::{
        DriveLinkStatus, DriveMetadataStore, DriveOperations, DrivePutOptions, DriveService,
        OrganizerRun, OrganizerRunStatus, initial_record_version,
    };

    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    PostgresStore::connect(&dsn).await.unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let artifact_store =
        tm_artifacts::ArtifactStore::open(artifacts.path(), "durable-drive").unwrap();
    let metadata = PostgresDriveMetadataStore::connect(&dsn).await.unwrap();
    let drive = DriveService::with_metadata(artifact_store.clone(), metadata.clone());
    let suffix = Uuid::new_v4();
    let filed = DriveOperations::put_bytes(
        &drive,
        b"# Restart durable\nattributes, tags, proposals, and runs survive",
        DrivePutOptions {
            auto: true,
            suggested_path: Some(format!("inbox/{suffix}.md")),
            project: Some(format!("restart-{suffix}")),
            tags: vec!["durable".to_string()],
            ..DrivePutOptions::default()
        },
    )
    .await
    .unwrap();
    let tagged = DriveOperations::tag_entry(&drive, &filed.uri, vec!["restart".to_string()])
        .await
        .unwrap();
    let now = Utc::now();
    let run = metadata
        .insert_organizer_run(OrganizerRun {
            id: Uuid::new_v4(),
            version: initial_record_version(),
            trigger: "restart-test".to_string(),
            status: OrganizerRunStatus::Completed,
            attempts: 1,
            proposal_ids: filed
                .proposal
                .as_ref()
                .map(|proposal| vec![proposal.id])
                .unwrap_or_default(),
            created_at: now,
            available_at: now,
            locked_at: None,
            completed_at: Some(now),
            last_error: None,
        })
        .await
        .unwrap();
    let linked_root = tempfile::tempdir().unwrap();
    let plan = tm_drive::drive_link_plan(
        linked_root.path(),
        tm_host::FsMode::Ro,
        Some(&format!("restart-{suffix}")),
    )
    .unwrap();
    DriveOperations::record_link(&drive, &plan).await.unwrap();
    drop(drive);
    drop(metadata);

    let restarted_metadata = PostgresDriveMetadataStore::connect(&dsn).await.unwrap();
    let restarted = DriveService::with_metadata(artifact_store, restarted_metadata.clone());
    let snapshot = restarted.metadata_snapshot().await.unwrap();
    let entry = snapshot
        .entries
        .iter()
        .find(|entry| entry.id == tagged.id)
        .unwrap();
    assert_eq!(entry.version, tagged.version);
    assert!(entry.tags.iter().any(|tag| tag == "durable"));
    assert!(entry.tags.iter().any(|tag| tag == "restart"));
    assert!(!entry.attributes.is_empty());
    assert!(snapshot.proposals.iter().any(|proposal| {
        filed
            .proposal
            .as_ref()
            .is_some_and(|filed| filed.id == proposal.id)
    }));
    assert!(
        snapshot
            .organizer_runs
            .iter()
            .any(|stored| stored.id == run.id)
    );
    let link = snapshot
        .links
        .iter()
        .find(|link| link.alias == plan.alias)
        .unwrap();
    assert_eq!(link.status, DriveLinkStatus::Active);

    let revoked = DriveOperations::revoke_link(&restarted, &plan.alias)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(revoked.status, DriveLinkStatus::Revoked);
    drop(restarted);
    let after_restart = PostgresDriveMetadataStore::connect(&dsn).await.unwrap();
    assert_eq!(
        after_restart
            .link(&plan.alias)
            .await
            .unwrap()
            .unwrap()
            .status,
        DriveLinkStatus::Revoked
    );
}
#[tokio::test]
async fn gated_postgres_drive_entry_cas_allows_one_concurrent_writer() {
    use tm_drive::{DriveMetadataStore, DriveOperations, DrivePutOptions, DriveService};

    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    PostgresStore::connect(&dsn).await.unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let artifact_store = tm_artifacts::ArtifactStore::open(artifacts.path(), "drive-cas").unwrap();
    let first_store = PostgresDriveMetadataStore::connect(&dsn).await.unwrap();
    let second_store = PostgresDriveMetadataStore::connect(&dsn).await.unwrap();
    let drive = DriveService::with_metadata(artifact_store, first_store.clone());
    let filed = DriveOperations::put_bytes(
        &drive,
        b"compare and swap",
        DrivePutOptions {
            suggested_path: Some(format!("cas/{}.txt", Uuid::new_v4())),
            ..DrivePutOptions::default()
        },
    )
    .await
    .unwrap();
    let mut first = filed.entry.clone();
    first.tags.push("first".to_string());
    let mut second = filed.entry.clone();
    second.tags.push("second".to_string());
    let (left, right) = tokio::join!(
        first_store.compare_and_swap_entry(filed.entry.id, filed.entry.version, first),
        second_store.compare_and_swap_entry(filed.entry.id, filed.entry.version, second),
    );
    assert_ne!(left.is_ok(), right.is_ok());
    assert!(matches!(
        left.err().or_else(|| right.err()).unwrap(),
        tm_drive::DriveError::Conflict { .. }
    ));
}

#[tokio::test]
async fn gated_postgres_drive_move_and_organizer_commits_are_atomic() {
    use tm_drive::{
        DriveCorrectionRecord, DriveEntry, DriveEntryUpdate, DriveMetadataStore, DriveMoveCommit,
        DriveOperations, DriveOverwriteTarget, DrivePutOptions, DriveService,
        OrganizerProposalCommit, ProposalStatus, generate_organizer_proposals_for_run,
        initial_record_version,
    };

    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let schema_store = PostgresStore::connect(&dsn).await.unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let artifact_store =
        tm_artifacts::ArtifactStore::open(artifacts.path(), "drive-atomic").unwrap();
    let first_store = PostgresDriveMetadataStore::connect(&dsn).await.unwrap();
    let second_store = PostgresDriveMetadataStore::connect(&dsn).await.unwrap();
    let drive = DriveService::with_metadata(artifact_store, first_store.clone());
    let suffix = Uuid::new_v4();
    let source = DriveOperations::put_bytes(
        &drive,
        b"atomic source",
        DrivePutOptions {
            suggested_path: Some(format!("atomic/{suffix}/source.txt")),
            ..DrivePutOptions::default()
        },
    )
    .await
    .unwrap()
    .entry;
    let target = DriveOperations::put_bytes(
        &drive,
        b"atomic target",
        DrivePutOptions {
            suggested_path: Some(format!("atomic/{suffix}/target.txt")),
            ..DrivePutOptions::default()
        },
    )
    .await
    .unwrap()
    .entry;
    let mut concurrent = source.clone();
    concurrent.tags.push("concurrent".to_string());
    second_store
        .compare_and_swap_entry(source.id, source.version, concurrent)
        .await
        .unwrap();
    let mut stale_replacement = source.clone();
    stale_replacement.path = target.path.clone();
    stale_replacement.uri = DriveEntry::drive_uri(&target.path);
    let stale_correction_id = Uuid::new_v4();
    let error = first_store
        .commit_move(DriveMoveCommit {
            source: DriveEntryUpdate {
                expected_version: source.version,
                replacement: stale_replacement,
            },
            overwrite: Some(DriveOverwriteTarget {
                id: target.id,
                expected_version: target.version,
            }),
            correction: DriveCorrectionRecord {
                id: stale_correction_id,
                version: initial_record_version(),
                from: source.path.clone(),
                to: target.path.clone(),
                created_at: Utc::now(),
            },
        })
        .await
        .unwrap_err();
    assert!(matches!(error, tm_drive::DriveError::Conflict { .. }));
    assert_eq!(
        second_store
            .entry_by_path(&target.path)
            .await
            .unwrap()
            .unwrap()
            .id,
        target.id
    );
    let stale_corrections: i64 = schema_store
        .client()
        .query_one(
            "select count(*) from drive_corrections where id=$1",
            &[&stale_correction_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stale_corrections, 0);

    let organizer_content =
        format!("# Atomic {suffix}\norganizer should move this unique note into project notes");
    let organizer_entry = DriveOperations::put_bytes(
        &drive,
        organizer_content.as_bytes(),
        DrivePutOptions {
            suggested_path: Some(format!("inbox/{suffix}.md")),
            project: Some("TempestMiku".to_string()),
            doc_kind: Some("note".to_string()),
            ..DrivePutOptions::default()
        },
    )
    .await
    .unwrap()
    .entry;
    let proposal = generate_organizer_proposals_for_run(
        std::slice::from_ref(&organizer_entry),
        Uuid::new_v4(),
    )
    .into_iter()
    .next()
    .expect("organizer proposal");
    let proposal = first_store.insert_proposal(proposal).await.unwrap();
    let proposed_path = proposal.proposed_path.clone().expect("proposed path");
    let now = Utc::now();
    let correction_id = Uuid::new_v4();
    let mut replacement_entry = organizer_entry.clone();
    replacement_entry.path = proposed_path.clone();
    replacement_entry.uri = DriveEntry::drive_uri(&proposed_path);
    replacement_entry.updated_at = now;
    let mut replacement_proposal = proposal.clone();
    replacement_proposal.status = ProposalStatus::Applied;
    replacement_proposal.updated_at = now;
    let commit = OrganizerProposalCommit {
        expected_proposal_version: proposal.version,
        replacement: replacement_proposal,
        entry_update: Some(DriveEntryUpdate {
            expected_version: organizer_entry.version,
            replacement: replacement_entry,
        }),
        correction: Some(DriveCorrectionRecord {
            id: correction_id,
            version: initial_record_version(),
            from: organizer_entry.path.clone(),
            to: proposed_path.clone(),
            created_at: now,
        }),
    };
    let (left, right) = tokio::join!(
        first_store.commit_organizer_proposal(commit.clone()),
        second_store.commit_organizer_proposal(commit),
    );
    assert_eq!(left.is_ok() as usize + right.is_ok() as usize, 1);
    assert!(matches!(
        left.err().or_else(|| right.err()).unwrap(),
        tm_drive::DriveError::Conflict { .. }
    ));
    assert_eq!(
        second_store
            .entry_by_path(&proposed_path)
            .await
            .unwrap()
            .unwrap()
            .id,
        organizer_entry.id
    );
    let correction_count: i64 = schema_store
        .client()
        .query_one(
            "select count(*) from drive_corrections where id=$1",
            &[&correction_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(correction_count, 1);
    let retried = DriveOperations::apply_organizer_proposals(&drive, &[proposal.id])
        .await
        .unwrap();
    assert_eq!(retried[0].status, ProposalStatus::Applied);
    let correction_count_after_retry: i64 = schema_store
        .client()
        .query_one(
            "select count(*) from drive_corrections where id=$1",
            &[&correction_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(correction_count_after_retry, 1);
}

use chrono::Utc;
use tm_artifacts::ArtifactStore;
use tm_host::FsMode;

use super::*;
use crate::{DriveOperations, DrivePutOptions, DriveService, ProposalStatus, drive_link_plan};

#[tokio::test]
async fn snapshot_restart_preserves_mutable_metadata_and_revoked_links() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path(), "drive").unwrap();
    let drive = DriveService::new(artifacts.clone());
    let filed = DriveOperations::put_bytes(
        &drive,
        b"# Durable note\nmetadata survives restart",
        DrivePutOptions {
            auto: true,
            suggested_path: Some("inbox/durable.md".to_string()),
            tags: vec!["initial".to_string()],
            ..DrivePutOptions::default()
        },
    )
    .await
    .unwrap();
    DriveOperations::tag_entry(&drive, &filed.uri, vec!["review".to_string()])
        .await
        .unwrap();
    let linked = tempfile::tempdir().unwrap();
    let plan = drive_link_plan(linked.path(), FsMode::Ro, Some("durable-project")).unwrap();
    DriveOperations::record_link(&drive, &plan).await.unwrap();
    let invalidated = DriveOperations::invalidate_link(
        &drive,
        &plan.alias,
        "upstream returned sk-testsecret123456",
    )
    .await
    .unwrap();
    assert!(
        !invalidated.metadata["invalidReason"]
            .as_str()
            .unwrap()
            .contains("sk-testsecret123456")
    );
    DriveOperations::revoke_link(&drive, &plan.alias)
        .await
        .unwrap();

    let snapshot = drive.metadata_snapshot().await.unwrap();
    let restarted = DriveService::from_snapshot(artifacts, snapshot.clone()).unwrap();
    assert_eq!(restarted.metadata_snapshot().await.unwrap(), snapshot);
    let proposals = DriveOperations::proposals(&restarted).await.unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].status, ProposalStatus::Pending);
    let links = DriveOperations::links(&restarted).await.unwrap();
    assert_eq!(links[0].status, crate::DriveLinkStatus::Revoked);
    assert!(links[0].version > initial_record_version());
}

#[tokio::test]
async fn compare_and_swap_rejects_a_concurrent_stale_entry_update() {
    let root = tempfile::tempdir().unwrap();
    let drive = DriveService::new(ArtifactStore::open(root.path(), "drive").unwrap());
    let filed = DriveOperations::put_bytes(
        &drive,
        b"concurrency",
        DrivePutOptions {
            suggested_path: Some("notes/concurrency.txt".to_string()),
            ..DrivePutOptions::default()
        },
    )
    .await
    .unwrap();
    let metadata = drive.metadata_store();
    let mut first = filed.entry.clone();
    first.tags.push("first".to_string());
    let mut second = filed.entry.clone();
    second.tags.push("second".to_string());

    let (left, right) = tokio::join!(
        metadata.compare_and_swap_entry(filed.entry.id, filed.entry.version, first),
        metadata.compare_and_swap_entry(filed.entry.id, filed.entry.version, second),
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let error = left.err().or_else(|| right.err()).unwrap();
    assert!(matches!(error, DriveError::Conflict { .. }));
    assert_eq!(
        metadata
            .entry(filed.entry.id)
            .await
            .unwrap()
            .unwrap()
            .version,
        filed.entry.version + 1
    );
}

#[tokio::test]
async fn stale_overwrite_move_preserves_the_target_and_records_no_correction() {
    let root = tempfile::tempdir().unwrap();
    let drive = DriveService::new(ArtifactStore::open(root.path(), "drive").unwrap());
    let source = DriveOperations::put_bytes(
        &drive,
        b"source",
        DrivePutOptions {
            suggested_path: Some("notes/source.txt".to_string()),
            ..DrivePutOptions::default()
        },
    )
    .await
    .unwrap()
    .entry;
    let target = DriveOperations::put_bytes(
        &drive,
        b"target",
        DrivePutOptions {
            suggested_path: Some("notes/target.txt".to_string()),
            ..DrivePutOptions::default()
        },
    )
    .await
    .unwrap()
    .entry;
    let metadata = drive.metadata_store();
    let mut concurrent = source.clone();
    concurrent.tags.push("concurrent".to_string());
    metadata
        .compare_and_swap_entry(source.id, source.version, concurrent)
        .await
        .unwrap();

    let mut replacement = source.clone();
    replacement.path = target.path.clone();
    replacement.uri = DriveEntry::drive_uri(&target.path);
    let error = metadata
        .commit_move(DriveMoveCommit {
            source: DriveEntryUpdate {
                expected_version: source.version,
                replacement,
            },
            overwrite: Some(DriveOverwriteTarget {
                id: target.id,
                expected_version: target.version,
            }),
            correction: DriveCorrectionRecord {
                id: Uuid::new_v4(),
                version: initial_record_version(),
                from: source.path.clone(),
                to: target.path.clone(),
                created_at: Utc::now(),
            },
        })
        .await
        .unwrap_err();
    assert!(matches!(error, DriveError::Conflict { .. }));
    assert_eq!(
        metadata
            .entry_by_path(&target.path)
            .await
            .unwrap()
            .unwrap()
            .id,
        target.id
    );
    assert_eq!(
        metadata
            .entry_by_path(&source.path)
            .await
            .unwrap()
            .unwrap()
            .id,
        source.id
    );
    assert!(metadata.snapshot().await.unwrap().corrections.is_empty());
}

#[tokio::test]
async fn organizer_commit_is_atomic_under_conflict_and_retry() {
    let root = tempfile::tempdir().unwrap();
    let drive = DriveService::new(ArtifactStore::open(root.path(), "drive").unwrap());
    let entry = DriveOperations::put_bytes(
        &drive,
        b"# Raw\norganizer should move this into project notes",
        DrivePutOptions {
            suggested_path: Some("inbox/atomic.md".to_string()),
            project: Some("TempestMiku".to_string()),
            doc_kind: Some("note".to_string()),
            ..DrivePutOptions::default()
        },
    )
    .await
    .unwrap()
    .entry;
    let proposal = DriveOperations::organize_scoped(&drive, Some("TempestMiku"))
        .await
        .unwrap()
        .into_iter()
        .find(|proposal| proposal.entry_id == entry.id)
        .expect("move proposal");
    let target = proposal.proposed_path.clone().expect("proposed path");
    let now = Utc::now();
    let mut replacement_entry = entry.clone();
    replacement_entry.path = target.clone();
    replacement_entry.uri = DriveEntry::drive_uri(&target);
    replacement_entry.updated_at = now;
    let mut replacement_proposal = proposal.clone();
    replacement_proposal.status = ProposalStatus::Applied;
    replacement_proposal.updated_at = now;
    let commit = OrganizerProposalCommit {
        expected_proposal_version: proposal.version,
        replacement: replacement_proposal,
        entry_update: Some(DriveEntryUpdate {
            expected_version: entry.version,
            replacement: replacement_entry,
        }),
        correction: Some(DriveCorrectionRecord {
            id: Uuid::new_v4(),
            version: initial_record_version(),
            from: entry.path.clone(),
            to: target.clone(),
            created_at: now,
        }),
    };
    let metadata = drive.metadata_store();
    let (left, right) = tokio::join!(
        metadata.commit_organizer_proposal(commit.clone()),
        metadata.commit_organizer_proposal(commit),
    );
    assert_eq!(left.is_ok() as usize + right.is_ok() as usize, 1);
    assert!(matches!(
        left.err().or_else(|| right.err()).unwrap(),
        DriveError::Conflict { .. }
    ));

    let snapshot = metadata.snapshot().await.unwrap();
    assert_eq!(snapshot.corrections.len(), 1);
    assert_eq!(
        metadata.entry_by_path(&target).await.unwrap().unwrap().id,
        entry.id
    );
    assert_eq!(
        metadata.entry(entry.id).await.unwrap().unwrap().version,
        entry.version + 1
    );
    assert_eq!(
        metadata
            .proposals()
            .await
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.id == proposal.id)
            .unwrap()
            .status,
        ProposalStatus::Applied
    );

    let retried = DriveOperations::apply_organizer_proposals(&drive, &[proposal.id])
        .await
        .unwrap();
    assert_eq!(retried[0].status, ProposalStatus::Applied);
    assert_eq!(metadata.snapshot().await.unwrap().corrections.len(), 1);
}

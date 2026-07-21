use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use super::*;

#[test]
fn drive_payloads_use_stable_camel_case_wire_shapes() {
    let now = Utc::now();
    let entry = DriveEntry {
        id: Uuid::nil(),
        version: initial_record_version(),
        path: "projects/tempestmiku/notes/p5.txt".to_string(),
        uri: "drive://projects/tempestmiku/notes/p5.txt".to_string(),
        blob_uri: "blob:sha256:abc".to_string(),
        content_hash: "abc".to_string(),
        mime: "text/plain".to_string(),
        size_bytes: 3,
        title: Some("P5".to_string()),
        doc_kind: Some("note".to_string()),
        project: Some("TempestMiku".to_string()),
        entities: vec!["Tempest Miku".to_string()],
        dates: vec!["2026-07-08".to_string()],
        amounts: Vec::new(),
        tags: vec!["planning".to_string()],
        embedding: None,
        source_uri: Some("fixture://p5".to_string()),
        provenance: vec![DriveProvenance {
            source_uri: Some("fixture://p5".to_string()),
            session_id: Some("session".to_string()),
            event_seq: Some(7),
            actor_id: None,
            source_run_id: None,
            content_hash: "abc".to_string(),
            extractor: "test".to_string(),
            created_at: now,
        }],
        created_at: now,
        updated_at: now,
        status: DriveEntryStatus::Active,
        attributes: vec![DriveAttribute {
            key: "doc_kind".to_string(),
            value: "note".to_string(),
            confidence: 1.0,
            evidence: Some(DriveEvidence {
                snippet: "P5".to_string(),
                selector: Some("1-1".to_string()),
            }),
            extractor: "test".to_string(),
            source_uri: Some("fixture://p5".to_string()),
            session_id: Some("session".to_string()),
            event_seq: Some(7),
            content_hash: Some("abc".to_string()),
        }],
        summary: Some("P5".to_string()),
    };
    let proposal = OrganizerProposal {
        id: Uuid::nil(),
        version: initial_record_version(),
        action: OrganizerActionKind::Move,
        entry_id: entry.id,
        source_path: "inbox/p5.txt".to_string(),
        proposed_path: Some(entry.path.clone()),
        proposed_tags: vec!["planning".to_string()],
        proposed_doc_kind: Some("note".to_string()),
        proposed_project: Some("TempestMiku".to_string()),
        evidence: vec![DriveEvidence {
            snippet: "P5".to_string(),
            selector: Some("1-1".to_string()),
        }],
        confidence: 0.9,
        policy_decision: PolicyDecision::ApprovalRequired,
        approval_id: Some("approval-1".to_string()),
        status: ProposalStatus::Pending,
        source_run_id: Uuid::nil(),
        replay_metadata: [("contentHash".to_string(), json!("abc"))].into(),
        created_at: now,
        updated_at: now,
    };
    let put = DrivePutResult {
        entry: entry.clone(),
        uri: entry.uri.clone(),
        proposed_path: entry.path.clone(),
        filed: true,
        proposal: Some(proposal.clone()),
    };

    let value = serde_json::to_value(&put).unwrap();
    assert_eq!(
        value["proposedPath"],
        json!("projects/tempestmiku/notes/p5.txt")
    );
    assert_eq!(value["entry"]["blobUri"], json!("blob:sha256:abc"));
    assert_eq!(value["entry"]["contentHash"], json!("abc"));
    assert!(value["entry"]["createdAt"].is_string());
    assert_eq!(
        value["proposal"]["policyDecision"],
        json!("approval_required")
    );
    assert!(value["proposal"]["sourceRunId"].is_string());

    let run = OrganizerRun {
        id: Uuid::nil(),
        version: initial_record_version(),
        trigger: "manual".to_string(),
        status: OrganizerRunStatus::Running,
        attempts: 2,
        proposal_ids: vec![proposal.id],
        created_at: now,
        available_at: now,
        locked_at: Some(now),
        completed_at: None,
        last_error: None,
    };
    let value = serde_json::to_value(run).unwrap();
    assert_eq!(value["proposalIds"][0], json!(Uuid::nil()));
    assert!(value["lockedAt"].is_string());
    assert_eq!(value["completedAt"], json!(null));

    let search = DriveSearchResult {
        uri: entry.uri,
        path: entry.path,
        title: entry.title,
        doc_kind: entry.doc_kind,
        project: entry.project,
        tags: entry.tags,
        content_hash: entry.content_hash,
        score: 1.0,
        snippet: Some("P5".to_string()),
        selector: Some("1-1".to_string()),
    };
    let value = serde_json::to_value(search).unwrap();
    assert_eq!(value["docKind"], json!("note"));
    assert_eq!(value["contentHash"], json!("abc"));
}

#[test]
fn drive_event_names_are_reserved_in_order() {
    assert_eq!(
        DRIVE_EVENT_NAMES,
        [
            "drive_put",
            "drive_transduced",
            "drive_path_proposed",
            "drive_write_proposed",
            "drive_filed",
            "drive_moved",
            "drive_tagged",
            "project_linked",
            "project_unlinked",
            "drive_organizer_started",
            "drive_organizer_completed",
            "drive_organizer_failed",
        ]
    );
}

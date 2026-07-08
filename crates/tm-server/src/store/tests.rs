use chrono::{Duration, Utc};
use serde_json::json;
use tm_memory::{DreamReason, DreamStatus, NewDreamQueueRecord, RecallChunkRecord};
use tm_modes::{AssetStatus, ModeId};
use uuid::Uuid;

use super::*;

fn postgres_test_dsn() -> Option<String> {
    (std::env::var("TM_POSTGRES_TESTS").ok().as_deref() == Some("1"))
        .then(|| {
            std::env::var("TM_TEST_DATABASE_URL")
                .or_else(|_| std::env::var("TM_DATABASE_URL"))
                .ok()
        })
        .flatten()
}

#[tokio::test]
async fn in_memory_session_events_expose_actor_output_refs() {
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

    let non_actor = store
        .append_event(
            session.id,
            "text",
            json!({"delta": "artifact://not-an-actor-output"}),
        )
        .await
        .unwrap();
    assert_eq!(non_actor.actor_id, None);
    assert_eq!(non_actor.artifact_uri, None);
    assert_eq!(non_actor.history_uri, None);

    let completed = store
        .append_event(
            session.id,
            "actor_completed",
            json!({
                "kind": "completed",
                "actor_id": "Worker",
                "artifact_uri": "artifact://0",
                "history_uri": "history://Worker",
            }),
        )
        .await
        .unwrap();
    assert_eq!(completed.actor_id.as_deref(), Some("Worker"));
    assert_eq!(completed.artifact_uri.as_deref(), Some("artifact://0"));
    assert_eq!(completed.history_uri.as_deref(), Some("history://Worker"));

    let linked = store
        .append_event(
            session.id,
            "actor_resources_linked",
            json!({
                "kind": "resources_linked",
                "actor_id": "Worker",
                "source_event_seq": completed.seq,
                "artifact_uri": "artifact://0",
                "history_uri": "history://Worker",
            }),
        )
        .await
        .unwrap();
    assert_eq!(linked.actor_id.as_deref(), Some("Worker"));
    assert_eq!(linked.artifact_uri.as_deref(), Some("artifact://0"));
    assert_eq!(linked.history_uri.as_deref(), Some("history://Worker"));

    let replay = store
        .events_after(session.id, Some(completed.seq - 1))
        .await
        .unwrap();
    assert_eq!(
        replay
            .iter()
            .map(|event| (
                event.event_type.as_str(),
                event.actor_id.as_deref(),
                event.artifact_uri.as_deref(),
                event.history_uri.as_deref(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                "actor_completed",
                Some("Worker"),
                Some("artifact://0"),
                Some("history://Worker")
            ),
            (
                "actor_resources_linked",
                Some("Worker"),
                Some("artifact://0"),
                Some("history://Worker")
            ),
        ]
    );
}

#[tokio::test]
async fn in_memory_end_session_enqueue_dream_is_idempotent() {
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

    let ended = store.end_session(session.id).await.unwrap();
    assert_eq!(ended.status, "ended");

    let first = store
        .enqueue_dream(NewDreamQueueRecord::session_end(
            session.id, "brian", "global", None,
        ))
        .await
        .unwrap();
    let duplicate = store
        .enqueue_dream(NewDreamQueueRecord::session_end(
            session.id,
            "brian",
            "project:tempestmiku",
            Some(9),
        ))
        .await
        .unwrap();

    assert_eq!(duplicate.id, first.id);
    assert_eq!(duplicate.reason, DreamReason::SessionEnded);
    assert_eq!(duplicate.status, DreamStatus::Queued);
    assert_eq!(duplicate.source_event_seq, Some(9));
    let queued = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].id, first.id);
}

#[tokio::test]
async fn in_memory_dream_claim_heartbeat_complete_and_stale_reclaim() {
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
    let now = Utc::now();
    let queued = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:lifecycle:{}", session.id),
            source_event_seq: None,
            available_at: now - Duration::seconds(1),
        })
        .await
        .unwrap();

    let claimed = store
        .claim_ready_dream(now, Duration::seconds(30))
        .await
        .unwrap()
        .expect("ready dream");
    assert_eq!(claimed.id, queued.id);
    assert_eq!(claimed.status, DreamStatus::Running);
    assert_eq!(claimed.attempts, 1);
    assert_eq!(claimed.locked_at, Some(now));
    assert!(
        store
            .claim_ready_dream(now + Duration::seconds(10), Duration::seconds(30))
            .await
            .unwrap()
            .is_none()
    );

    let reclaimed = store
        .claim_ready_dream(now + Duration::seconds(31), Duration::seconds(30))
        .await
        .unwrap()
        .expect("stale running dream");
    assert_eq!(reclaimed.id, queued.id);
    assert_eq!(reclaimed.status, DreamStatus::Running);
    assert_eq!(reclaimed.attempts, 2);

    let heartbeat_at = now + Duration::seconds(35);
    let heartbeated = store
        .heartbeat_dream(queued.id, heartbeat_at)
        .await
        .unwrap();
    assert_eq!(heartbeated.locked_at, Some(heartbeat_at));

    let completed_at = now + Duration::seconds(40);
    let completed = store.complete_dream(queued.id, completed_at).await.unwrap();
    assert_eq!(completed.status, DreamStatus::Completed);
    assert_eq!(completed.locked_at, None);
    assert_eq!(completed.available_at, completed_at);
    assert!(
        store
            .claim_ready_dream(now + Duration::seconds(90), Duration::seconds(30))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn in_memory_dream_failure_records_error_and_bounded_retry() {
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
    let now = Utc::now();
    let queued = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:retry:{}", session.id),
            source_event_seq: None,
            available_at: now,
        })
        .await
        .unwrap();

    let first = store
        .claim_ready_dream(now, Duration::seconds(30))
        .await
        .unwrap()
        .expect("first attempt");
    assert_eq!(first.attempts, 1);

    let retry_at = now + Duration::seconds(60);
    let retryable = store
        .fail_dream(queued.id, "model timeout".to_string(), retry_at, 2)
        .await
        .unwrap();
    assert_eq!(retryable.status, DreamStatus::Queued);
    assert_eq!(retryable.attempts, 1);
    assert_eq!(retryable.last_error.as_deref(), Some("model timeout"));
    assert_eq!(retryable.available_at, retry_at);
    assert!(
        store
            .claim_ready_dream(now + Duration::seconds(30), Duration::seconds(30))
            .await
            .unwrap()
            .is_none()
    );

    let second = store
        .claim_ready_dream(retry_at, Duration::seconds(30))
        .await
        .unwrap()
        .expect("second attempt");
    assert_eq!(second.attempts, 2);

    let terminal = store
        .fail_dream(
            queued.id,
            "redaction failed".to_string(),
            retry_at + Duration::seconds(60),
            2,
        )
        .await
        .unwrap();
    assert_eq!(terminal.status, DreamStatus::Failed);
    assert_eq!(terminal.locked_at, None);
    assert_eq!(terminal.last_error.as_deref(), Some("redaction failed"));
    assert!(
        store
            .claim_ready_dream(retry_at + Duration::seconds(120), Duration::seconds(30))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn gated_postgres_covers_replay_memory_approvals_and_project_refs() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();

    let ended = store.end_session(session.id).await.unwrap();
    assert_eq!(ended.status, "ended");
    let dream = store
        .enqueue_dream(NewDreamQueueRecord::session_end(
            session.id,
            "brian",
            "global",
            Some(1),
        ))
        .await
        .unwrap();
    let duplicate = store
        .enqueue_dream(NewDreamQueueRecord::session_end(
            session.id,
            "brian",
            "global",
            Some(2),
        ))
        .await
        .unwrap();
    assert_eq!(duplicate.id, dream.id);
    assert_eq!(duplicate.status, DreamStatus::Queued);
    assert_eq!(
        store
            .dream_queue_for_session(session.id)
            .await
            .unwrap()
            .len(),
        1
    );
    let session_end_claim = store
        .claim_ready_dream(Utc::now(), Duration::seconds(30))
        .await
        .unwrap()
        .expect("postgres session-end dream");
    assert_eq!(session_end_claim.id, dream.id);
    let completed_session_end = store
        .complete_dream(session_end_claim.id, Utc::now())
        .await
        .unwrap();
    assert_eq!(completed_session_end.status, DreamStatus::Completed);
    let duplicate_after_complete = store
        .enqueue_dream(NewDreamQueueRecord::session_end(
            session.id,
            "brian",
            "global",
            Some(3),
        ))
        .await
        .unwrap();
    assert_eq!(duplicate_after_complete.id, dream.id);
    assert_eq!(duplicate_after_complete.status, DreamStatus::Completed);

    let lifecycle = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:postgres-lifecycle:{}", Uuid::new_v4()),
            source_event_seq: None,
            available_at: Utc::now() - Duration::days(3650),
        })
        .await
        .unwrap();
    let claimed = store
        .claim_ready_dream(Utc::now(), Duration::seconds(30))
        .await
        .unwrap()
        .expect("postgres ready dream");
    assert_eq!(claimed.id, lifecycle.id);
    assert_eq!(claimed.status, DreamStatus::Running);
    assert_eq!(claimed.attempts, 1);
    let heartbeat_at = Utc::now() + Duration::seconds(1);
    let heartbeated = store
        .heartbeat_dream(claimed.id, heartbeat_at)
        .await
        .unwrap();
    assert_eq!(heartbeated.locked_at, Some(heartbeat_at));
    let retry_at = Utc::now() + Duration::seconds(60);
    let retryable = store
        .fail_dream(
            claimed.id,
            "postgres transient failure".to_string(),
            retry_at,
            2,
        )
        .await
        .unwrap();
    assert_eq!(retryable.status, DreamStatus::Queued);
    assert_eq!(
        retryable.last_error.as_deref(),
        Some("postgres transient failure")
    );
    let second = store
        .claim_ready_dream(retry_at, Duration::seconds(30))
        .await
        .unwrap()
        .expect("postgres retry dream");
    assert_eq!(second.id, lifecycle.id);
    assert_eq!(second.attempts, 2);
    let completed = store.complete_dream(second.id, Utc::now()).await.unwrap();
    assert_eq!(completed.status, DreamStatus::Completed);
    assert_eq!(completed.locked_at, None);

    let stale = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:postgres-stale:{}", Uuid::new_v4()),
            source_event_seq: None,
            available_at: Utc::now() - Duration::days(3650),
        })
        .await
        .unwrap();
    let stale_now = Utc::now();
    let first_stale_claim = store
        .claim_ready_dream(stale_now, Duration::seconds(30))
        .await
        .unwrap()
        .expect("postgres stale test dream");
    assert_eq!(first_stale_claim.id, stale.id);
    assert_eq!(first_stale_claim.attempts, 1);
    assert!(
        store
            .claim_ready_dream(stale_now + Duration::seconds(10), Duration::seconds(30))
            .await
            .unwrap()
            .is_none(),
        "fresh running lock must not be reclaimed"
    );
    let reclaimed = store
        .claim_ready_dream(stale_now + Duration::seconds(31), Duration::seconds(30))
        .await
        .unwrap()
        .expect("postgres stale dream reclaimed");
    assert_eq!(reclaimed.id, stale.id);
    assert_eq!(reclaimed.attempts, 2);
    let terminal = store
        .fail_dream(
            reclaimed.id,
            "postgres terminal failure".to_string(),
            Utc::now(),
            2,
        )
        .await
        .unwrap();
    assert_eq!(terminal.status, DreamStatus::Failed);
    assert_eq!(
        terminal.last_error.as_deref(),
        Some("postgres terminal failure")
    );

    let mut mode_state = session.mode_state.clone();
    mode_state.mode = ModeId::from("serious_engineer");
    mode_state.router_reason = Some("postgres replay test".to_string());
    mode_state.updated_at = Utc::now();
    let updated = store
        .set_mode_state(session.id, mode_state.clone())
        .await
        .unwrap();
    assert_eq!(updated.mode_state.mode, ModeId::from("serious_engineer"));

    store
        .append_event(
            session.id,
            "mode",
            serde_json::to_value(StoreEvent::ModeChanged {
                from: Some(ModeId::from("general")),
                mode: ModeId::from("serious_engineer"),
                label: "Serious Engineer".to_string(),
                voice_cap: "off".to_string(),
                capabilities: vec![
                    "fs.*".to_string(),
                    "code.*".to_string(),
                    "proc.*".to_string(),
                    "backend.coding".to_string(),
                ],
                active_skills: Vec::new(),
                router_reason: mode_state.router_reason,
                lock_source: None,
                override_source: None,
                updated_at: Utc::now(),
                persona_status: updated.persona_status,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_event(
            session.id,
            "approval",
            json!({"approvalId": Uuid::new_v4(), "backend": "postgres-test"}),
        )
        .await
        .unwrap();
    store
        .append_event(
            session.id,
            "approval_resolved",
            json!({"backend": "postgres-test", "outcome": "selected"}),
        )
        .await
        .unwrap();
    let replay = store.events_after(session.id, Some(1)).await.unwrap();
    assert_eq!(
        replay
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["approval", "approval_resolved"]
    );

    let completed = store
        .append_event(
            session.id,
            "actor_completed",
            json!({
                "kind": "completed",
                "actor_id": "Worker",
                "artifact_uri": "artifact://0",
                "history_uri": "history://Worker",
            }),
        )
        .await
        .unwrap();
    assert_eq!(completed.actor_id.as_deref(), Some("Worker"));
    assert_eq!(completed.artifact_uri.as_deref(), Some("artifact://0"));
    assert_eq!(completed.history_uri.as_deref(), Some("history://Worker"));
    let replayed_completed = store
        .events_after(session.id, Some(completed.seq - 1))
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.event_type == "actor_completed")
        .expect("actor_completed replay row");
    assert_eq!(replayed_completed.actor_id.as_deref(), Some("Worker"));
    assert_eq!(
        replayed_completed.artifact_uri.as_deref(),
        Some("artifact://0")
    );
    assert_eq!(
        replayed_completed.history_uri.as_deref(),
        Some("history://Worker")
    );

    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "project:postgres-test".to_string(),
            text: "memory row for P1 project view".to_string(),
            source: format!("session:{}", session.id),
            importance: 0.65,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    let memory = store
        .recall_chunks("project:postgres-test", "P1 project", 5)
        .await
        .unwrap();
    assert_eq!(memory.len(), 1);

    let project_id = format!("postgres-test-{}", Uuid::new_v4());
    let item = store
        .upsert_project_item(NewProjectItem {
            project_id: project_id.clone(),
            kind: ProjectItemKind::Artifact,
            text: "artifact://0".to_string(),
            target_uri: format!("project://{project_id}/artifacts/0"),
            source_session_id: session.id,
            source_event_seq: Some(2),
            source_uri: Some("artifact://0".to_string()),
            dedupe_key: "artifact://0".to_string(),
            provenance_json: json!({
                "sourceSession": session.id,
                "sourceEventId": 2,
                "sourceUri": "artifact://0",
            }),
        })
        .await
        .unwrap();
    let duplicate = store
        .upsert_project_item(NewProjectItem {
            project_id: project_id.clone(),
            kind: ProjectItemKind::Artifact,
            text: "artifact://0".to_string(),
            target_uri: format!("project://{project_id}/artifacts/0"),
            source_session_id: session.id,
            source_event_seq: Some(2),
            source_uri: Some("artifact://0".to_string()),
            dedupe_key: "artifact://0".to_string(),
            provenance_json: json!({"sourceSession": session.id}),
        })
        .await
        .unwrap();
    assert_eq!(item.id, duplicate.id);
    assert_eq!(
        store
            .project_items(&project_id, Some(ProjectItemKind::Artifact))
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn gated_postgres_recall_chunks_use_fts_with_substring_fallback() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    let scope = format!("postgres-fts-{}", Uuid::new_v4());
    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: scope.clone(),
            text: "Ship ledger keeps approvals deferred for review.".to_string(),
            source: "postgres-fts".to_string(),
            importance: 0.7,
            created_at: Utc::now() - Duration::minutes(5),
        })
        .await
        .unwrap();
    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: scope.clone(),
            text: "Ship approvals are contiguous here.".to_string(),
            source: "postgres-ilike".to_string(),
            importance: 0.4,
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let fts = store
        .recall_chunks(&scope, "ship review", 10)
        .await
        .unwrap();
    assert_eq!(fts.len(), 1);
    assert_eq!(fts[0].source, "postgres-fts");
    let substring = store.recall_chunks(&scope, "contigu", 10).await.unwrap();
    assert_eq!(substring.len(), 1);
    assert_eq!(substring[0].source, "postgres-ilike");
}

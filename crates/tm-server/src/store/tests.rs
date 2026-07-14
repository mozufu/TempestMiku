use chrono::{Duration, Utc};
use serde_json::json;
use tm_memory::{
    DreamReason, DreamStatus, MemoryEvidenceRef, MemorySummaryKind, NewDreamQueueRecord,
    NewMemorySummaryRecord, NewSkillProposalRecord, RecallChunkRecord, SkillVerification,
};
use tm_modes::{AssetStatus, ModeId};
use uuid::Uuid;

use super::*;
use crate::{
    AuthDeviceStore, FakePushProvider, PostgresPushStore, PushCipher, PushMessageKind, PushService,
    ServerError,
};
use std::sync::Arc;

fn postgres_test_dsn() -> Option<String> {
    (std::env::var("TM_POSTGRES_TESTS").ok().as_deref() == Some("1"))
        .then(|| {
            std::env::var("TM_TEST_DATABASE_URL")
                .or_else(|_| std::env::var("TM_DATABASE_URL"))
                .ok()
        })
        .flatten()
}

fn assert_postgres_timestamp_eq(
    actual: Option<chrono::DateTime<Utc>>,
    expected: Option<chrono::DateTime<Utc>>,
) {
    assert_eq!(
        actual.map(|value| value.timestamp_micros()),
        expected.map(|value| value.timestamp_micros())
    );
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

    let first = store
        .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string())
        .await
        .unwrap();
    assert_eq!(first.session.status, "ended");
    assert!(first.newly_ended);
    assert_eq!(
        first
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["dream_queued", "session_end"]
    );
    assert_eq!(first.dream.source_event_seq, Some(first.events[1].seq));
    assert!(first.events[0].seq < first.events[1].seq);

    let duplicate = store
        .end_session_and_enqueue_dream(
            session.id,
            "brian".to_string(),
            "project:tempestmiku".to_string(),
        )
        .await
        .unwrap();

    assert!(!duplicate.newly_ended);
    assert!(duplicate.events.is_empty());
    assert_eq!(duplicate.dream.id, first.dream.id);
    assert_eq!(duplicate.dream.reason, DreamReason::SessionEnded);
    assert_eq!(duplicate.dream.status, DreamStatus::Queued);
    assert_eq!(
        duplicate.dream.source_event_seq,
        first.dream.source_event_seq
    );
    let queued = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].id, first.dream.id);
    assert_eq!(store.events_after(session.id, None).await.unwrap().len(), 2);
}

#[tokio::test]
async fn in_memory_session_authority_is_backfilled_and_scope_is_explicit() {
    let store = InMemoryStore::default();
    let first = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    assert_eq!(first.owner_subject, "brian");
    assert_eq!(first.memory_scope, "global");

    assert_eq!(store.configure_owner_subject("brian").await.unwrap(), 0);
    assert_eq!(
        store.get_session(first.id).await.unwrap().owner_subject,
        "brian"
    );
    assert_eq!(store.configure_owner_subject("brian").await.unwrap(), 0);
    assert!(matches!(
        store.configure_owner_subject("someone-else").await,
        Err(ServerError::Conflict(_))
    ));

    let scoped = store
        .set_session_memory_scope(first.id, "project:tempestmiku")
        .await
        .unwrap();
    assert_eq!(scoped.memory_scope, "project:tempestmiku");
    let second = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    assert_eq!(second.owner_subject, "brian");
    assert_eq!(second.memory_scope, "global");
}

#[tokio::test]
async fn in_memory_scope_and_session_end_use_one_authoritative_lock() {
    let store = InMemoryStore::default();
    store.configure_owner_subject("owner").await.unwrap();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .set_session_memory_scope(session.id, "project:authoritative")
        .await
        .unwrap();

    let ended = store
        .end_session_and_enqueue_dream(session.id, "stale-caller".to_string(), "global".to_string())
        .await
        .unwrap();
    assert_eq!(ended.dream.subject, "owner");
    assert_eq!(ended.dream.scope, "project:authoritative");
    assert_eq!(ended.events[0].payload_json["subject"], json!("owner"));
    assert_eq!(
        ended.events[0].payload_json["scope"],
        json!("project:authoritative")
    );
    assert!(matches!(
        store.set_session_memory_scope(session.id, "global").await,
        Err(ServerError::Conflict(_))
    ));
    assert_eq!(
        store.get_session(session.id).await.unwrap().memory_scope,
        "project:authoritative"
    );
}

#[tokio::test]
async fn in_memory_session_end_refuses_queued_and_running_turns_atomically() {
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
    let turn = store
        .enqueue_turn(session.id, "active-before-end", "finish me first")
        .await
        .unwrap();
    let event_count = store.events_after(session.id, None).await.unwrap().len();

    assert!(matches!(
        store.end_session(session.id).await,
        Err(ServerError::Conflict(_))
    ));
    assert!(matches!(
        store
            .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string(),)
            .await,
        Err(ServerError::Conflict(_))
    ));
    assert_eq!(store.get_session(session.id).await.unwrap().status, "open");
    assert!(
        store
            .dream_queue_for_session(session.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.events_after(session.id, None).await.unwrap().len(),
        event_count
    );

    let worker_id = Uuid::new_v4();
    store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .expect("queued turn becomes running");
    assert!(matches!(
        store.end_session(session.id).await,
        Err(ServerError::Conflict(_))
    ));
    assert!(matches!(
        store
            .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string(),)
            .await,
        Err(ServerError::Conflict(_))
    ));
    assert_eq!(store.get_session(session.id).await.unwrap().status, "open");
    assert!(
        store
            .dream_queue_for_session(session.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.events_after(session.id, None).await.unwrap().len(),
        event_count
    );

    store
        .fail_turn(turn.id, worker_id, "finished for test", Utc::now())
        .await
        .unwrap();
    let ended = store
        .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string())
        .await
        .unwrap();
    assert_eq!(ended.session.status, "ended");
}

#[tokio::test]
async fn in_memory_turn_enqueue_is_idempotent_and_rejects_ended_sessions() {
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

    let first = store
        .enqueue_turn(session.id, "message-1", "hello, miku")
        .await
        .unwrap();
    let duplicate = store
        .enqueue_turn(session.id, "message-1", "hello, miku")
        .await
        .unwrap();
    assert_eq!(duplicate.id, first.id);
    assert_eq!(first.status, "queued");
    assert_eq!(first.content_hash.len(), 64);
    assert!(
        first
            .content_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert!(matches!(
        store
            .enqueue_turn(session.id, "message-1", "different")
            .await,
        Err(ServerError::Conflict(_))
    ));
    let messages = store.session_messages(session.id).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].turn_id, Some(first.id));

    assert!(matches!(
        store.end_session(session.id).await,
        Err(ServerError::Conflict(_))
    ));
    let worker_id = Uuid::new_v4();
    store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .expect("queued turn");
    store
        .fail_turn(first.id, worker_id, "test cleanup", Utc::now())
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    assert!(matches!(
        store
            .enqueue_turn(session.id, "message-2", "too late")
            .await,
        Err(ServerError::Conflict(_))
    ));
}

#[tokio::test]
async fn in_memory_persistence_redacts_turn_messages_and_recursive_events_without_breaking_hash_idempotency()
 {
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
    let raw = "use sk-testsecret123456 for this request";
    let turn = store
        .enqueue_turn(session.id, "redacted-message", raw)
        .await
        .unwrap();
    assert_eq!(turn.content_hash, super::models::turn_content_hash(raw));
    assert!(!turn.content.contains("sk-testsecret123456"));
    assert_eq!(
        store
            .enqueue_turn(session.id, "redacted-message", raw)
            .await
            .unwrap()
            .id,
        turn.id
    );
    assert!(matches!(
        store
            .enqueue_turn(
                session.id,
                "redacted-message",
                "use sk-anothersecret123456 for this request",
            )
            .await,
        Err(ServerError::Conflict(_))
    ));
    let messages = store.session_messages(session.id).await.unwrap();
    assert!(!messages[0].content.contains("sk-testsecret123456"));

    let event = store
        .append_event(
            session.id,
            "diagnostic",
            json!({
                "nested": [{
                    "authorization": "short-secret",
                    "message": "Authorization: Bearer opaque-token-123456",
                }],
            }),
        )
        .await
        .unwrap();
    let serialized = serde_json::to_string(&event.payload_json).unwrap();
    assert!(!serialized.contains("short-secret"));
    assert!(!serialized.contains("opaque-token-123456"));
    assert!(serialized.contains("REDACTED"));

    let assistant = store
        .append_message(
            session.id,
            "assistant",
            "provider returned sk-assistantsecret123456",
        )
        .await
        .unwrap();
    assert!(!assistant.content.contains("sk-assistantsecret123456"));
}

#[tokio::test]
async fn in_memory_memory_and_project_store_boundaries_cannot_persist_credentials() {
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
    let secret = "sk-testsecret123456";
    let now = Utc::now();

    assert!(
        store
            .add_profile_fact(ProfileFactRecord {
                id: Uuid::new_v4(),
                subject: "brian".to_string(),
                predicate: "uses".to_string(),
                object: secret.to_string(),
                confidence: 1.0,
                importance: 1.0,
                provenance: "test".to_string(),
                valid_from: now,
                valid_to: None,
            })
            .await
            .is_err()
    );
    assert!(
        store
            .add_recall_chunk(RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: "global".to_string(),
                text: format!("remember {secret}"),
                source: "test".to_string(),
                importance: 1.0,
                created_at: now,
            })
            .await
            .is_err()
    );

    let project = store
        .upsert_project_item(NewProjectItem {
            project_id: "tempestmiku".to_string(),
            kind: ProjectItemKind::Summary,
            text: format!("provider returned {secret}"),
            target_uri: "project://tempestmiku/summary".to_string(),
            source_session_id: session.id,
            source_event_seq: None,
            source_uri: None,
            dedupe_key: "summary:test".to_string(),
            provenance_json: json!({"accessToken": "short-secret"}),
        })
        .await
        .unwrap();
    let project_json = serde_json::to_string(&project).unwrap();
    assert!(!project_json.contains(secret));
    assert!(!project_json.contains("short-secret"));

    let evidence = vec![MemoryEvidenceRef {
        session_id: session.id,
        event_seq: None,
        message_seq: None,
        uri: Some("history://Worker".to_string()),
        label: format!("evidence {secret}"),
    }];
    let summary = store
        .upsert_memory_summary(NewMemorySummaryRecord {
            kind: MemorySummaryKind::Session,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            title: format!("summary {secret}"),
            body: format!("body {secret}"),
            evidence: evidence.clone(),
            source_dream_id: Uuid::new_v4(),
            source_session_id: Some(session.id),
            dedupe_key: "summary:credential-test".to_string(),
        })
        .await
        .unwrap();
    assert!(!serde_json::to_string(&summary).unwrap().contains(secret));

    let skill = store
        .upsert_skill_proposal(NewSkillProposalRecord {
            name: "safe-workflow".to_string(),
            description: format!("description {secret}"),
            body: format!("body {secret}"),
            trigger: format!("trigger {secret}"),
            use_criteria: "when requested".to_string(),
            evidence,
            self_critique: format!("critique {secret}"),
            verification: SkillVerification {
                passed: true,
                checks: vec![format!("checked {secret}")],
            },
            dedupe_key: "skill:credential-test".to_string(),
            source_dream_id: Uuid::new_v4(),
            source_session_id: session.id,
        })
        .await
        .unwrap();
    assert!(!serde_json::to_string(&skill).unwrap().contains(secret));

    assert!(
        store
            .upsert_project_item(NewProjectItem {
                project_id: secret.to_string(),
                kind: ProjectItemKind::Summary,
                text: "safe".to_string(),
                target_uri: "project://safe".to_string(),
                source_session_id: session.id,
                source_event_seq: None,
                source_uri: None,
                dedupe_key: "reject:credential".to_string(),
                provenance_json: json!({}),
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn in_memory_turn_claims_serialize_per_session_and_completion_is_atomic() {
    let store = InMemoryStore::default();
    let make_session = || NewSession {
        mode: ModeId::from("general"),
        persona_status: AssetStatus::Degraded {
            warning: "test".to_string(),
        },
    };
    let first_session = store.create_session(make_session()).await.unwrap();
    let first = store
        .enqueue_turn(first_session.id, "first", "one")
        .await
        .unwrap();
    let second = store
        .enqueue_turn(first_session.id, "second", "two")
        .await
        .unwrap();
    let now = Utc::now();
    let first_worker = Uuid::new_v4();
    let first_claim = store
        .claim_next_turn(first_worker, now)
        .await
        .unwrap()
        .expect("first queued turn");
    assert_eq!(first_claim.id, first.id);
    assert!(
        store
            .claim_next_turn(Uuid::new_v4(), now)
            .await
            .unwrap()
            .is_none(),
        "a session may have only one running turn"
    );

    let other_session = store.create_session(make_session()).await.unwrap();
    let other = store
        .enqueue_turn(other_session.id, "other", "parallel session")
        .await
        .unwrap();
    let other_worker = Uuid::new_v4();
    let other_claim = store
        .claim_next_turn(other_worker, now)
        .await
        .unwrap()
        .expect("other session may run concurrently");
    assert_eq!(other_claim.id, other.id);

    let event = store
        .append_event_for_turn(
            first_session.id,
            "turn_progress",
            json!({"phase": "running"}),
            Some(first.id),
        )
        .await
        .unwrap();
    assert_eq!(event.turn_id, Some(first.id));
    let completed_at = now + Duration::seconds(1);
    let completed = store
        .complete_turn(first.id, first_worker, "done", completed_at)
        .await
        .unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.completed_at, Some(completed_at));
    let repeated = store
        .complete_turn(first.id, first_worker, "ignored retry", completed_at)
        .await
        .unwrap();
    assert_eq!(repeated.id, completed.id);
    let messages = store.session_messages(first_session.id).await.unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.turn_id == Some(first.id) && message.role == "assistant")
            .count(),
        1
    );

    let second_worker = Uuid::new_v4();
    let second_claim = store
        .claim_next_turn(second_worker, completed_at)
        .await
        .unwrap()
        .expect("next turn in completed session");
    assert_eq!(second_claim.id, second.id);
    assert!(matches!(
        store
            .fail_turn(second.id, Uuid::new_v4(), "wrong worker", completed_at)
            .await,
        Err(ServerError::Conflict(_))
    ));
    let failed = store
        .fail_turn(second.id, second_worker, "worker failed", completed_at)
        .await
        .unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.error.as_deref(), Some("worker failed"));

    store
        .fail_turn(other.id, other_worker, "cleanup", completed_at)
        .await
        .unwrap();
}

#[tokio::test]
async fn in_memory_turn_heartbeat_protects_live_work_and_stale_cleanup_uses_freshness() {
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
    let turn = store
        .enqueue_turn(session.id, "stale", "recover me")
        .await
        .unwrap();
    let started_at = Utc::now() - Duration::minutes(10);
    let worker_id = Uuid::new_v4();
    store
        .claim_next_turn(worker_id, started_at)
        .await
        .unwrap()
        .unwrap();
    let heartbeat_at = Utc::now();
    let heartbeat = store
        .heartbeat_turn(turn.id, worker_id, heartbeat_at)
        .await
        .unwrap();
    assert_eq!(heartbeat.started_at, Some(started_at));
    assert_eq!(heartbeat.updated_at, heartbeat_at);
    assert!(matches!(
        store
            .heartbeat_turn(turn.id, Uuid::new_v4(), heartbeat_at)
            .await,
        Err(ServerError::Conflict(_))
    ));
    assert_eq!(
        store
            .fail_stale_running_turns(
                heartbeat_at - Duration::seconds(1),
                Utc::now(),
                "server restarted",
            )
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.turn(turn.id).await.unwrap().status, "running");
    assert_eq!(
        store
            .fail_stale_running_turns(
                heartbeat_at + Duration::seconds(1),
                Utc::now(),
                "server restarted",
            )
            .await
            .unwrap(),
        1
    );
    let recovered = store.turn(turn.id).await.unwrap();
    assert_eq!(recovered.status, "failed");
    assert_eq!(recovered.error.as_deref(), Some("server restarted"));
}

#[tokio::test]
async fn in_memory_approval_resolution_is_cas_and_effect_application_is_fenced() {
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
    let approval_id = Uuid::new_v4();
    let requester_id = Uuid::new_v4();
    let request = NewApprovalRequest {
        id: approval_id,
        session_id: session.id,
        turn_id: None,
        requester_id,
        origin: "test".to_string(),
        action: "memory.write".to_string(),
        scope_json: json!({"subject": "brian"}),
        options_json: json!([
            {"id": "allow", "label": "Allow"},
            {"id": "deny", "label": "Deny"}
        ]),
        effect_type: "memory_write".to_string(),
        effect_payload_json: json!({"proposalId": "proposal-1"}),
        resumable: true,
        created_at: now,
        expires_at: now + Duration::minutes(5),
    };
    let created = store
        .create_approval_request(request.clone())
        .await
        .unwrap();
    assert_eq!(created.status, "pending");
    assert_eq!(
        store.create_approval_request(request).await.unwrap().id,
        approval_id
    );
    assert!(
        store
            .claim_approval_effect(approval_id, Uuid::new_v4(), now, Duration::seconds(30),)
            .await
            .unwrap()
            .is_none(),
        "effects remain blocked until the approval reaches a terminal state"
    );

    let request_event = store
        .append_event(session.id, "approval", json!({"approvalId": approval_id}))
        .await
        .unwrap();
    let linked = store
        .link_approval_event(session.id, approval_id, "approval", request_event.seq)
        .await
        .unwrap();
    assert_eq!(linked.request_event_seq, Some(request_event.seq));
    assert!(matches!(
        store
            .heartbeat_approval_request(approval_id, Uuid::new_v4(), now)
            .await,
        Err(ServerError::Conflict(_))
    ));
    assert_eq!(
        store
            .heartbeat_approval_request(approval_id, requester_id, now + Duration::seconds(1))
            .await
            .unwrap()
            .heartbeat_at,
        now + Duration::seconds(1)
    );

    let resolved_at = now + Duration::seconds(2);
    let (resolved, resolution_event) = store
        .resolve_approval_request_with_event(
            session.id,
            approval_id,
            NewApprovalResolution {
                status: "approved".to_string(),
                selected_option_id: Some("allow".to_string()),
                resolution_json: json!({
                    "approvalId": approval_id,
                    "status": "approved",
                    "outcome": "approved"
                }),
                resolved_at,
            },
        )
        .await
        .unwrap();
    assert_eq!(resolved.status, "approved");
    assert_eq!(resolved.resolution_version, 1);
    assert_eq!(resolved.resolution_event_seq, Some(resolution_event.seq));
    assert_eq!(resolution_event.event_type, "approval_resolved");
    assert!(matches!(
        store
            .resolve_approval_request(
                session.id,
                approval_id,
                NewApprovalResolution {
                    status: "denied".to_string(),
                    selected_option_id: Some("deny".to_string()),
                    resolution_json: json!({"status": "denied", "outcome": "denied"}),
                    resolved_at,
                },
            )
            .await,
        Err(ServerError::Conflict(_))
    ));

    let first_owner = Uuid::new_v4();
    let first = store
        .claim_approval_effect(approval_id, first_owner, resolved_at, Duration::seconds(30))
        .await
        .unwrap()
        .expect("resolved approval effect");
    assert_eq!(first.effect.effect_type, "memory_write");
    assert_eq!(first.effect.payload_json["proposalId"], "proposal-1");
    assert_eq!(
        first.effect.payload_json["resolution"]["outcome"],
        "approved"
    );
    assert!(
        store
            .claim_approval_effect(
                approval_id,
                Uuid::new_v4(),
                resolved_at + Duration::seconds(10),
                Duration::seconds(30),
            )
            .await
            .unwrap()
            .is_none()
    );

    let second = store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            resolved_at + Duration::seconds(31),
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("stale effect lease is reclaimed");
    assert!(
        store
            .complete_approval_effect_with_event(
                &first,
                json!({"proposalId": "proposal-1", "status": "approved"}),
                None,
                resolved_at + Duration::seconds(31),
            )
            .await
            .is_err(),
        "the reclaimed lease fences the old owner"
    );
    let (applied, terminal_event) = store
        .complete_approval_effect_with_event(
            &second,
            json!({"proposalId": "proposal-1", "status": "approved"}),
            None,
            resolved_at + Duration::seconds(32),
        )
        .await
        .unwrap();
    assert_eq!(applied.status, "applied");
    assert_eq!(terminal_event.event_type, "write_proposal");
    assert!(
        store
            .claim_approval_effect(
                approval_id,
                Uuid::new_v4(),
                resolved_at + Duration::seconds(60),
                Duration::seconds(30),
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn in_memory_approval_effect_retry_commits_one_mutation_and_terminal_event() {
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
    let approval_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    store
        .create_approval_request(NewApprovalRequest {
            id: approval_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "fault-test".to_string(),
            action: "memory.write".to_string(),
            scope_json: json!({"scope": "global"}),
            options_json: json!([]),
            effect_type: "memory_write".to_string(),
            effect_payload_json: json!({"proposalId": proposal_id}),
            resumable: true,
            created_at: now,
            expires_at: now + Duration::minutes(5),
        })
        .await
        .unwrap();
    let resolved_at = now + Duration::seconds(1);
    store
        .resolve_approval_request_with_event(
            session.id,
            approval_id,
            NewApprovalResolution {
                status: "approved".to_string(),
                selected_option_id: Some("allow".to_string()),
                resolution_json: json!({
                    "approvalId": approval_id,
                    "status": "approved",
                    "outcome": "approved",
                }),
                resolved_at,
            },
        )
        .await
        .unwrap();
    let first = store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            resolved_at,
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("first effect lease");
    let record = RecallChunkRecord {
        id: proposal_id,
        scope: "global".to_string(),
        text: "one durable reminder".to_string(),
        source: "fault-test".to_string(),
        importance: 1.0,
        created_at: resolved_at,
    };

    // Simulate a process dying after the idempotent mutation but before finalization.
    store.upsert_recall_chunk(record.clone()).await.unwrap();
    let retry_at = resolved_at + Duration::seconds(31);
    let retry = store
        .claim_approval_effect(approval_id, Uuid::new_v4(), retry_at, Duration::seconds(30))
        .await
        .unwrap()
        .expect("stale effect is reclaimed");
    store.upsert_recall_chunk(record).await.unwrap();

    let terminal_payload = json!({
        "kind": "memory",
        "proposalId": proposal_id,
        "status": "approved",
    });
    assert!(
        store
            .complete_approval_effect_with_event(&first, terminal_payload.clone(), None, retry_at,)
            .await
            .is_err(),
        "the stale owner must not append an event"
    );
    let (applied, event) = store
        .complete_approval_effect_with_event(&retry, terminal_payload.clone(), None, retry_at)
        .await
        .unwrap();
    assert_eq!(applied.status, "applied");
    assert_eq!(event.event_type, "write_proposal");
    assert_eq!(event.payload_json, terminal_payload);
    assert!(
        store
            .complete_approval_effect_with_event(
                &retry,
                json!({"status": "approved"}),
                None,
                retry_at,
            )
            .await
            .is_err(),
        "an applied effect cannot append a second event"
    );
    assert_eq!(
        store
            .recall_chunks("global", "", 10)
            .await
            .unwrap()
            .iter()
            .filter(|record| record.id == proposal_id)
            .count(),
        1
    );
    assert_eq!(
        store
            .events_after(session.id, None)
            .await
            .unwrap()
            .iter()
            .filter(|event| {
                event.event_type == "write_proposal"
                    && event.payload_json["proposalId"] == json!(proposal_id)
                    && event.payload_json["status"] == json!("approved")
            })
            .count(),
        1
    );
}

#[tokio::test]
async fn in_memory_stale_approval_recovery_cancels_only_non_resumable_requests() {
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
    let created_at = now - Duration::minutes(2);
    let non_resumable_id = Uuid::new_v4();
    let resumable_id = Uuid::new_v4();
    let expired_id = Uuid::new_v4();
    for (id, resumable) in [(non_resumable_id, false), (resumable_id, true)] {
        store
            .create_approval_request(NewApprovalRequest {
                id,
                session_id: session.id,
                turn_id: None,
                requester_id: Uuid::new_v4(),
                origin: "test".to_string(),
                action: "code.run".to_string(),
                scope_json: json!({}),
                options_json: json!([]),
                effect_type: "approval_continuation".to_string(),
                effect_payload_json: json!({}),
                resumable,
                created_at,
                expires_at: now + Duration::minutes(5),
            })
            .await
            .unwrap();
    }
    store
        .create_approval_request(NewApprovalRequest {
            id: expired_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "test".to_string(),
            action: "memory.write".to_string(),
            scope_json: json!({}),
            options_json: json!([]),
            effect_type: "approval_continuation".to_string(),
            effect_payload_json: json!({"origin": "test"}),
            resumable: true,
            created_at,
            expires_at: now - Duration::minutes(1),
        })
        .await
        .unwrap();

    let expiry_events = store.expire_pending_approvals(now).await.unwrap();
    assert_eq!(expiry_events.len(), 1);
    assert_eq!(expiry_events[0].event_type, "approval_resolved");
    assert_eq!(
        expiry_events[0].payload_json["approvalId"],
        json!(expired_id)
    );
    assert_eq!(
        store
            .approval_request(session.id, expired_id)
            .await
            .unwrap()
            .status,
        "timed_out"
    );
    assert!(
        store
            .expire_pending_approvals(now)
            .await
            .unwrap()
            .is_empty()
    );
    let expiry_worker = Uuid::new_v4();
    let expired_effect = store
        .claim_next_approval_effect(expiry_worker, now, Duration::seconds(30))
        .await
        .unwrap()
        .expect("expiry unblocks a worker-visible effect");
    assert_eq!(expired_effect.effect.approval_id, expired_id);
    store
        .complete_approval_effect(&expired_effect, now)
        .await
        .unwrap();

    let cancellation_events = store
        .cancel_stale_non_resumable_approvals(now - Duration::minutes(1), now)
        .await
        .unwrap();
    assert_eq!(cancellation_events.len(), 1);
    assert_eq!(
        cancellation_events[0].payload_json["approvalId"],
        json!(non_resumable_id)
    );
    assert_eq!(
        store
            .approval_request(session.id, non_resumable_id)
            .await
            .unwrap()
            .status,
        "cancelled"
    );
    assert_eq!(
        store
            .approval_request(session.id, resumable_id)
            .await
            .unwrap()
            .status,
        "pending"
    );
    assert!(
        store
            .claim_approval_effect(non_resumable_id, Uuid::new_v4(), now, Duration::seconds(30),)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .claim_approval_effect(resumable_id, Uuid::new_v4(), now, Duration::seconds(30),)
            .await
            .unwrap()
            .is_none()
    );
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

    let first_owner = Uuid::new_v4();
    let claimed = store
        .claim_ready_dream(now, Duration::seconds(30), first_owner)
        .await
        .unwrap()
        .expect("ready dream");
    assert_eq!(claimed.dream.id, queued.id);
    assert_eq!(claimed.dream.status, DreamStatus::Running);
    assert_eq!(claimed.dream.attempts, 1);
    assert_eq!(claimed.dream.locked_at, Some(now));
    assert_eq!(claimed.dream.heartbeat_at, Some(now));
    assert!(
        store
            .claim_ready_dream(
                now + Duration::seconds(10),
                Duration::seconds(30),
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .is_none()
    );

    let reclaimed = store
        .claim_ready_dream(
            now + Duration::seconds(31),
            Duration::seconds(30),
            Uuid::new_v4(),
        )
        .await
        .unwrap()
        .expect("stale running dream");
    assert_eq!(reclaimed.dream.id, queued.id);
    assert_eq!(reclaimed.dream.status, DreamStatus::Running);
    assert_eq!(reclaimed.dream.attempts, 2);
    assert!(store.complete_dream(&claimed, Utc::now()).await.is_err());

    let heartbeat_at = now + Duration::seconds(35);
    let heartbeated = store
        .heartbeat_dream(&reclaimed, heartbeat_at)
        .await
        .unwrap();
    assert_eq!(heartbeated.dream.locked_at, Some(heartbeat_at));
    assert_eq!(heartbeated.dream.heartbeat_at, Some(heartbeat_at));

    let completed_at = now + Duration::seconds(40);
    let completed = store
        .complete_dream(&reclaimed, completed_at)
        .await
        .unwrap();
    assert_eq!(completed.status, DreamStatus::Completed);
    assert_eq!(completed.locked_at, None);
    assert_eq!(completed.available_at, completed_at);
    assert_eq!(completed.completed_at, Some(completed_at));
    assert_eq!(completed.error_at, None);
    assert!(
        store
            .claim_ready_dream(
                now + Duration::seconds(90),
                Duration::seconds(30),
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn in_memory_dream_crash_on_final_attempt_becomes_terminal() {
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
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:final-crash:{}", session.id),
            source_event_seq: None,
            available_at: now,
        })
        .await
        .unwrap();

    for attempt in 1..=3 {
        let lease = store
            .claim_ready_dream_bounded(
                now + Duration::seconds(i64::from((attempt - 1) * 31)),
                Duration::seconds(30),
                Uuid::new_v4(),
                3,
            )
            .await
            .unwrap()
            .expect("attempt should be leased");
        assert_eq!(lease.dream.attempts, attempt);
    }
    assert!(
        store
            .claim_ready_dream_bounded(
                now + Duration::seconds(93),
                Duration::seconds(30),
                Uuid::new_v4(),
                3,
            )
            .await
            .unwrap()
            .is_none()
    );
    let terminal = store.dream(dream.id).await.unwrap();
    assert_eq!(terminal.status, DreamStatus::Failed);
    assert_eq!(terminal.attempts, 3);
    assert_eq!(terminal.lease_owner, None);
    assert!(terminal.error_at.is_some());
    assert!(
        terminal
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("final permitted attempt"))
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
    let _queued = store
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
        .claim_ready_dream(now, Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("first attempt");
    assert_eq!(first.dream.attempts, 1);

    let retry_at = now + Duration::seconds(60);
    let retryable = store
        .fail_dream(&first, "model timeout".to_string(), retry_at, 2)
        .await
        .unwrap();
    assert_eq!(retryable.status, DreamStatus::Queued);
    assert_eq!(retryable.attempts, 1);
    assert_eq!(retryable.last_error.as_deref(), Some("model timeout"));
    assert_eq!(retryable.available_at, retry_at);
    assert!(retryable.error_at.is_some());
    assert_eq!(retryable.completed_at, None);
    assert!(
        store
            .claim_ready_dream(
                now + Duration::seconds(30),
                Duration::seconds(30),
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .is_none()
    );

    let second = store
        .claim_ready_dream(retry_at, Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("second attempt");
    assert_eq!(second.dream.attempts, 2);

    let terminal = store
        .fail_dream(
            &second,
            "redaction failed".to_string(),
            retry_at + Duration::seconds(60),
            2,
        )
        .await
        .unwrap();
    assert_eq!(terminal.status, DreamStatus::Failed);
    assert_eq!(terminal.locked_at, None);
    assert_eq!(terminal.last_error.as_deref(), Some("redaction failed"));
    assert!(terminal.error_at.is_some());
    assert!(
        store
            .claim_ready_dream(
                retry_at + Duration::seconds(120),
                Duration::seconds(30),
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn in_memory_cron_claim_is_unique_heartbeat_aware_and_fenced() {
    let store = InMemoryStore::default();
    let now = Utc::now();
    let job = store
        .upsert_cron_job(NewCronJobRecord {
            id: "lease-test".to_string(),
            name: "Lease test sk-testsecret123456".to_string(),
            schedule: "* * * * *".to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 1,
            script_timeout_seconds: 30,
            next_run_at: Some(now),
        })
        .await
        .unwrap();
    assert!(!job.name.contains("sk-testsecret123456"));
    let new_run = NewCronRunRecord {
        job_id: "lease-test".to_string(),
        scheduled_for: now,
        status: "queued".to_string(),
        session_id: None,
        result_json: json!({"accessToken": "short-secret"}),
    };
    let (first, claimed) = store
        .claim_cron_run(new_run.clone(), Uuid::new_v4(), now, Duration::seconds(30))
        .await
        .unwrap();
    assert!(claimed);
    assert_eq!(first.epoch, 1);
    assert_eq!(first.run.attempts, 1);
    assert_eq!(first.run.result_json["accessToken"], "[REDACTED_SECRET]");

    let heartbeat = store
        .heartbeat_cron_run(&first, now + Duration::seconds(20))
        .await
        .unwrap();
    let (still_owned, claimed) = store
        .claim_cron_run(
            new_run.clone(),
            Uuid::new_v4(),
            now + Duration::seconds(31),
            Duration::seconds(30),
        )
        .await
        .unwrap();
    assert!(!claimed);
    assert_eq!(still_owned.owner_id, heartbeat.owner_id);

    let (reclaimed, claimed) = store
        .claim_cron_run(
            new_run.clone(),
            Uuid::new_v4(),
            now + Duration::seconds(51),
            Duration::seconds(30),
        )
        .await
        .unwrap();
    assert!(claimed);
    assert_eq!(reclaimed.run.id, first.run.id);
    assert_eq!(reclaimed.epoch, 2);
    assert_eq!(reclaimed.run.attempts, 2);
    assert!(
        store
            .complete_cron_run(&first, "completed", None, json!({}))
            .await
            .is_err()
    );
    let completed = store
        .complete_cron_run(
            &reclaimed,
            "completed",
            None,
            json!({"message": "Authorization: Bearer opaque-token-123456"}),
        )
        .await
        .unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.lease_owner, None);
    assert!(
        !completed
            .result_json
            .to_string()
            .contains("opaque-token-123456")
    );

    let recorded = store.record_cron_run(new_run).await.unwrap();
    assert_eq!(recorded.id, completed.id);
    assert_eq!(store.cron_runs("lease-test", 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn in_memory_cron_crash_on_final_attempt_becomes_terminal() {
    let store = InMemoryStore::default();
    let now = Utc::now();
    store
        .upsert_cron_job(NewCronJobRecord {
            id: "final-crash".to_string(),
            name: "Final crash".to_string(),
            schedule: "* * * * *".to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 1,
            script_timeout_seconds: 30,
            next_run_at: Some(now),
        })
        .await
        .unwrap();
    let materialized = store
        .materialize_cron_run(
            NewCronRunRecord {
                job_id: "final-crash".to_string(),
                scheduled_for: now,
                status: "queued".to_string(),
                session_id: None,
                result_json: json!({}),
            },
            now,
            now + Duration::minutes(1),
        )
        .await
        .unwrap()
        .unwrap();
    let claim_base = materialized.available_at;

    let mut run_id = None;
    for attempt in 1..=3 {
        let lease = store
            .claim_ready_cron_run(
                Uuid::new_v4(),
                claim_base + Duration::seconds(i64::from((attempt - 1) * 31)),
                Duration::seconds(30),
                3,
            )
            .await
            .unwrap()
            .expect("attempt should be leased");
        run_id = Some(lease.run.id);
        assert_eq!(lease.run.attempts, attempt);
    }
    assert!(
        store
            .claim_ready_cron_run(
                Uuid::new_v4(),
                claim_base + Duration::seconds(93),
                Duration::seconds(30),
                3,
            )
            .await
            .unwrap()
            .is_none()
    );
    let terminal = store
        .cron_runs("final-crash", 10)
        .await
        .unwrap()
        .into_iter()
        .find(|run| Some(run.id) == run_id)
        .unwrap();
    assert_eq!(terminal.status, "failed");
    assert_eq!(terminal.attempts, 3);
    assert_eq!(terminal.lease_owner, None);
    assert!(terminal.completed_at.is_some());
    assert!(
        terminal
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("final permitted attempt"))
    );
}

#[tokio::test]
async fn in_memory_runtime_metrics_report_queue_age_and_scheduler_lag() {
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
    store
        .enqueue_turn(session.id, "metrics-turn", "queued")
        .await
        .unwrap();
    let now = Utc::now();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:metrics:{}", session.id),
            source_event_seq: None,
            available_at: now,
        })
        .await
        .unwrap();
    store
        .upsert_cron_job(NewCronJobRecord {
            id: "metrics".to_string(),
            name: "Metrics".to_string(),
            schedule: "* * * * *".to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 1,
            script_timeout_seconds: 30,
            next_run_at: Some(now - Duration::seconds(45)),
        })
        .await
        .unwrap();

    let metrics = store
        .runtime_metrics(now + Duration::seconds(5))
        .await
        .unwrap();
    assert_eq!(metrics.turn_queue_depth, 1);
    assert_eq!(metrics.dream_queue_depth, 1);
    assert_eq!(metrics.cron_queue_depth, 0);
    assert!(metrics.scheduler_lag_seconds >= 50);
    assert_eq!(metrics.pending_approvals, 0);
    assert_eq!(metrics.lease_reclaims, 0);
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

    let ended = store
        .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string())
        .await
        .unwrap();
    assert_eq!(ended.session.status, "ended");
    assert!(ended.newly_ended);
    assert_eq!(
        ended
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["dream_queued", "session_end"]
    );
    assert_eq!(ended.dream.source_event_seq, Some(ended.events[1].seq));
    assert!(ended.events[0].seq < ended.events[1].seq);
    let dream = ended.dream;
    let duplicate = store
        .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string())
        .await
        .unwrap();
    assert!(!duplicate.newly_ended);
    assert!(duplicate.events.is_empty());
    assert_eq!(duplicate.dream.id, dream.id);
    assert_eq!(duplicate.dream.status, DreamStatus::Queued);
    assert_eq!(
        store
            .dream_queue_for_session(session.id)
            .await
            .unwrap()
            .len(),
        1
    );
    let session_end_claim = store
        .claim_ready_dream(Utc::now(), Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("postgres session-end dream");
    assert_eq!(session_end_claim.dream.id, dream.id);
    let completed_session_end = store
        .complete_dream(&session_end_claim, Utc::now())
        .await
        .unwrap();
    assert_eq!(completed_session_end.status, DreamStatus::Completed);
    assert!(completed_session_end.completed_at.is_some());
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
        .claim_ready_dream(Utc::now(), Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("postgres ready dream");
    assert_eq!(claimed.dream.id, lifecycle.id);
    assert_eq!(claimed.dream.status, DreamStatus::Running);
    assert_eq!(claimed.dream.attempts, 1);
    let heartbeat_at = Utc::now() + Duration::seconds(1);
    let heartbeated = store.heartbeat_dream(&claimed, heartbeat_at).await.unwrap();
    assert_postgres_timestamp_eq(heartbeated.dream.locked_at, Some(heartbeat_at));
    assert_postgres_timestamp_eq(heartbeated.dream.heartbeat_at, Some(heartbeat_at));
    let retry_at = Utc::now() + Duration::seconds(60);
    let retryable = store
        .fail_dream(
            &claimed,
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
    assert!(retryable.error_at.is_some());
    let second = store
        .claim_ready_dream(retry_at, Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("postgres retry dream");
    assert_eq!(second.dream.id, lifecycle.id);
    assert_eq!(second.dream.attempts, 2);
    let completed = store.complete_dream(&second, Utc::now()).await.unwrap();
    assert_eq!(completed.status, DreamStatus::Completed);
    assert_eq!(completed.locked_at, None);
    assert!(completed.completed_at.is_some());
    assert_eq!(completed.error_at, None);

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
        .claim_ready_dream(stale_now, Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("postgres stale test dream");
    assert_eq!(first_stale_claim.dream.id, stale.id);
    assert_eq!(first_stale_claim.dream.attempts, 1);
    assert!(
        store
            .claim_ready_dream(
                stale_now + Duration::seconds(10),
                Duration::seconds(30),
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .is_none(),
        "fresh running lock must not be reclaimed"
    );
    let reclaimed = store
        .claim_ready_dream(
            stale_now + Duration::seconds(31),
            Duration::seconds(30),
            Uuid::new_v4(),
        )
        .await
        .unwrap()
        .expect("postgres stale dream reclaimed");
    assert_eq!(reclaimed.dream.id, stale.id);
    assert_eq!(reclaimed.dream.attempts, 2);
    assert!(
        store
            .complete_dream(&first_stale_claim, Utc::now())
            .await
            .is_err()
    );
    let terminal = store
        .fail_dream(
            &reclaimed,
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
    assert!(terminal.error_at.is_some());

    let cron_job_id = format!("postgres-lease-{}", Uuid::new_v4());
    let cron_now = Utc::now();
    store
        .upsert_cron_job(NewCronJobRecord {
            id: cron_job_id.clone(),
            name: "Postgres lease test".to_string(),
            schedule: "* * * * *".to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 1,
            script_timeout_seconds: 30,
            next_run_at: Some(cron_now),
        })
        .await
        .unwrap();
    let new_cron_run = NewCronRunRecord {
        job_id: cron_job_id.clone(),
        scheduled_for: cron_now,
        status: "queued".to_string(),
        session_id: None,
        result_json: json!({}),
    };
    let (first_cron, claimed) = store
        .claim_cron_run(
            new_cron_run.clone(),
            Uuid::new_v4(),
            cron_now,
            Duration::seconds(30),
        )
        .await
        .unwrap();
    assert!(claimed);
    let (reclaimed_cron, claimed) = store
        .claim_cron_run(
            new_cron_run.clone(),
            Uuid::new_v4(),
            cron_now + Duration::seconds(31),
            Duration::seconds(30),
        )
        .await
        .unwrap();
    assert!(claimed);
    assert_eq!(reclaimed_cron.run.id, first_cron.run.id);
    assert_eq!(reclaimed_cron.epoch, first_cron.epoch + 1);
    assert!(
        store
            .complete_cron_run(&first_cron, "completed", None, json!({}))
            .await
            .is_err()
    );
    let completed_cron = store
        .complete_cron_run(&reclaimed_cron, "completed", None, json!({}))
        .await
        .unwrap();
    assert_eq!(completed_cron.status, "completed");
    assert_eq!(
        store.record_cron_run(new_cron_run).await.unwrap().id,
        completed_cron.id
    );
    assert_eq!(store.cron_runs(&cron_job_id, 10).await.unwrap().len(), 1);

    let mut mode_state = session.mode_state.clone();
    mode_state.mode = ModeId::from("serious_engineer");
    mode_state.router_reason = Some("postgres replay test".to_string());
    mode_state.updated_at = Utc::now();
    let updated = store
        .set_mode_state(session.id, mode_state.clone())
        .await
        .unwrap();
    assert_eq!(updated.mode_state.mode, ModeId::from("serious_engineer"));

    let mode_event = store
        .append_event(
            session.id,
            "mode",
            serde_json::to_value(StoreEvent::ModeChanged(Box::new(ModeChangedStoreEvent {
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
            })))
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
    let replay = store
        .events_after(session.id, Some(mode_event.seq))
        .await
        .unwrap();
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
async fn gated_postgres_cron_materialization_and_leasing_are_cross_instance_safe() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let first = PostgresStore::connect(&dsn).await.unwrap();
    let second = PostgresStore::connect(&dsn).await.unwrap();
    let job_id = format!("postgres-cron-cas-{}", Uuid::new_v4());
    let due = Utc::now() - Duration::minutes(1);
    let next = due + Duration::minutes(1);
    first
        .upsert_cron_job(NewCronJobRecord {
            id: job_id.clone(),
            name: "cross instance cron CAS".to_string(),
            schedule: "* * * * *".to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 1,
            script_timeout_seconds: 30,
            next_run_at: Some(due),
        })
        .await
        .unwrap();
    let new_run = NewCronRunRecord {
        job_id: job_id.clone(),
        scheduled_for: due,
        status: "queued".to_string(),
        session_id: None,
        result_json: json!({"source": "cross-instance-test"}),
    };
    let (left, right) = tokio::join!(
        first.materialize_cron_run(new_run.clone(), due, next),
        second.materialize_cron_run(new_run, due, next),
    );
    assert_ne!(left.unwrap().is_some(), right.unwrap().is_some());
    assert_eq!(first.cron_runs(&job_id, 10).await.unwrap().len(), 1);
    assert_postgres_timestamp_eq(
        first.cron_job(&job_id).await.unwrap().next_run_at,
        Some(next),
    );

    let claimed_at = Utc::now();
    let first_owner = Uuid::new_v4();
    let second_owner = Uuid::new_v4();
    let (left, right) = tokio::join!(
        first.claim_ready_cron_run(first_owner, claimed_at, Duration::seconds(60), 3),
        second.claim_ready_cron_run(second_owner, claimed_at, Duration::seconds(60), 3),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_ne!(left.is_some(), right.is_some());
    let original = left.or(right).unwrap();
    let reclaiming_store = if original.owner_id == first_owner {
        &second
    } else {
        &first
    };
    let reclaimed = reclaiming_store
        .claim_ready_cron_run(
            Uuid::new_v4(),
            claimed_at + Duration::seconds(61),
            Duration::seconds(60),
            3,
        )
        .await
        .unwrap()
        .expect("stale cross-instance lease is reclaimed");
    assert_eq!(reclaimed.run.id, original.run.id);
    assert_eq!(reclaimed.epoch, original.epoch + 1);
    assert!(
        first
            .complete_cron_run(&original, "completed", None, json!({}))
            .await
            .is_err(),
        "the stale owner must be fenced"
    );
    let third = first
        .claim_ready_cron_run(
            Uuid::new_v4(),
            claimed_at + Duration::seconds(122),
            Duration::seconds(60),
            3,
        )
        .await
        .unwrap()
        .expect("third and final attempt is leased");
    assert_eq!(third.run.attempts, 3);
    assert!(
        second
            .claim_ready_cron_run(
                Uuid::new_v4(),
                claimed_at + Duration::seconds(183),
                Duration::seconds(60),
                3,
            )
            .await
            .unwrap()
            .is_none()
    );
    let terminal = first
        .cron_runs(&job_id, 10)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(terminal.status, "failed");
    assert_eq!(terminal.attempts, 3);
    assert_eq!(terminal.lease_owner, None);
    assert!(terminal.completed_at.is_some());
}

#[tokio::test]
async fn gated_postgres_dream_crash_on_final_attempt_is_fenced_and_terminal() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let first = PostgresStore::connect(&dsn).await.unwrap();
    let second = PostgresStore::connect(&dsn).await.unwrap();
    let session = first
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let now = Utc::now();
    let dream = first
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:postgres-final-crash:{}", Uuid::new_v4()),
            source_event_seq: None,
            available_at: now,
        })
        .await
        .unwrap();

    let first_lease = first
        .claim_ready_dream_bounded(now, Duration::seconds(30), Uuid::new_v4(), 3)
        .await
        .unwrap()
        .unwrap();
    let second_lease = second
        .claim_ready_dream_bounded(
            now + Duration::seconds(31),
            Duration::seconds(30),
            Uuid::new_v4(),
            3,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_lease.epoch, first_lease.epoch + 1);
    let third_lease = first
        .claim_ready_dream_bounded(
            now + Duration::seconds(62),
            Duration::seconds(30),
            Uuid::new_v4(),
            3,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(third_lease.dream.attempts, 3);
    if let Some(unrelated) = second
        .claim_ready_dream_bounded(
            now + Duration::seconds(93),
            Duration::seconds(30),
            Uuid::new_v4(),
            3,
        )
        .await
        .unwrap()
    {
        assert_ne!(unrelated.dream.id, dream.id);
        second
            .complete_dream(&unrelated, now + Duration::seconds(93))
            .await
            .unwrap();
    }
    let terminal = first.dream(dream.id).await.unwrap();
    assert_eq!(terminal.status, DreamStatus::Failed);
    assert_eq!(terminal.attempts, 3);
    assert_eq!(terminal.lease_owner, None);
    assert!(terminal.error_at.is_some());
}

#[tokio::test]
async fn gated_postgres_session_end_lifecycle_is_all_or_none_on_event_failure() {
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
    let suffix = Uuid::new_v4().simple().to_string();
    let function_name = format!("tm_fail_session_end_{suffix}");
    let trigger_name = format!("tm_fail_session_end_trigger_{suffix}");
    store
        .client()
        .batch_execute(&format!(
            "create function {function_name}() returns trigger language plpgsql as $$
             begin
               if new.session_id = '{}'::uuid and new.event_type = 'session_end' then
                 raise exception 'injected session_end failure';
               end if;
               return new;
             end
             $$;
             create trigger {trigger_name}
               before insert on session_events
               for each row execute function {function_name}();",
            session.id
        ))
        .await
        .unwrap();

    let result = store
        .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string())
        .await;
    store
        .client()
        .batch_execute(&format!(
            "drop trigger {trigger_name} on session_events;
             drop function {function_name}();"
        ))
        .await
        .unwrap();

    assert!(matches!(result, Err(ServerError::Store(_))));
    assert_eq!(store.get_session(session.id).await.unwrap().status, "open");
    assert!(
        store
            .events_after(session.id, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .dream_queue_for_session(session.id)
            .await
            .unwrap()
            .is_empty()
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

#[tokio::test]
async fn gated_postgres_session_authority_and_durable_turns() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    let other_store = PostgresStore::connect(&dsn).await.unwrap();
    store.configure_owner_subject("brian").await.unwrap();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    assert_eq!(session.owner_subject, "brian");
    assert_eq!(session.memory_scope, "global");
    assert_eq!(
        store
            .set_session_memory_scope(session.id, "project:postgres-turns")
            .await
            .unwrap()
            .memory_scope,
        "project:postgres-turns"
    );

    let first = store
        .enqueue_turn(session.id, "postgres-message-1", "hello")
        .await
        .unwrap();
    let duplicate = store
        .enqueue_turn(session.id, "postgres-message-1", "hello")
        .await
        .unwrap();
    assert_eq!(duplicate.id, first.id);
    assert!(matches!(
        store
            .enqueue_turn(session.id, "postgres-message-1", "changed")
            .await,
        Err(ServerError::Conflict(_))
    ));
    let second = store
        .enqueue_turn(session.id, "postgres-message-2", "next")
        .await
        .unwrap();
    assert_eq!(store.session_messages(session.id).await.unwrap().len(), 2);
    let lifecycle_event_count = store.events_after(session.id, None).await.unwrap().len();
    assert!(matches!(
        other_store.end_session(session.id).await,
        Err(ServerError::Conflict(_))
    ));
    assert!(matches!(
        store
            .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string(),)
            .await,
        Err(ServerError::Conflict(_))
    ));
    assert_eq!(store.get_session(session.id).await.unwrap().status, "open");
    assert!(
        store
            .dream_queue_for_session(session.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.events_after(session.id, None).await.unwrap().len(),
        lifecycle_event_count
    );

    let first_worker = Uuid::new_v4();
    let second_worker = Uuid::new_v4();
    let now = Utc::now();
    let (left, right) = tokio::join!(
        store.claim_next_turn(first_worker, now),
        other_store.claim_next_turn(second_worker, now),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    let mut claims = Vec::new();
    claims.extend(left.map(|turn| (turn, first_worker)));
    claims.extend(right.map(|turn| (turn, second_worker)));
    assert_eq!(
        claims
            .iter()
            .filter(|(turn, _)| turn.session_id == session.id)
            .count(),
        1
    );
    let target_index = claims
        .iter()
        .position(|(turn, _)| turn.session_id == session.id)
        .unwrap();
    let (claimed, worker_id) = claims.swap_remove(target_index);
    for (unrelated, unrelated_worker) in claims {
        store
            .complete_turn(
                unrelated.id,
                unrelated_worker,
                "unrelated queue cleanup",
                now,
            )
            .await
            .unwrap();
    }
    assert_eq!(claimed.id, first.id);
    assert!(matches!(
        other_store.end_session(session.id).await,
        Err(ServerError::Conflict(_))
    ));
    assert!(matches!(
        other_store
            .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string(),)
            .await,
        Err(ServerError::Conflict(_))
    ));
    assert!(
        store
            .claim_next_turn(Uuid::new_v4(), now)
            .await
            .unwrap()
            .is_none()
    );
    let event = store
        .append_event_for_turn(
            session.id,
            "turn_progress",
            json!({"phase": "postgres"}),
            Some(first.id),
        )
        .await
        .unwrap();
    assert_eq!(event.turn_id, Some(first.id));
    let completed = store
        .complete_turn(first.id, worker_id, "done", now)
        .await
        .unwrap();
    assert_eq!(completed.status, "completed");
    store
        .complete_turn(first.id, worker_id, "retry ignored", now)
        .await
        .unwrap();
    assert!(matches!(
        store
            .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string(),)
            .await,
        Err(ServerError::Conflict(_))
    ));
    assert_eq!(
        store
            .session_messages(session.id)
            .await
            .unwrap()
            .iter()
            .filter(|message| { message.turn_id == Some(first.id) && message.role == "assistant" })
            .count(),
        1
    );

    let stale_worker = Uuid::new_v4();
    let stale_started = Utc::now() - Duration::minutes(10);
    let stale = store
        .claim_next_turn(stale_worker, stale_started)
        .await
        .unwrap()
        .expect("second turn");
    assert_eq!(stale.id, second.id);
    let heartbeat_at = Utc::now();
    let heartbeat = other_store
        .heartbeat_turn(second.id, stale_worker, heartbeat_at)
        .await
        .unwrap();
    assert_postgres_timestamp_eq(heartbeat.started_at, Some(stale_started));
    assert_eq!(
        heartbeat.updated_at.timestamp_micros(),
        heartbeat_at.timestamp_micros()
    );
    assert!(matches!(
        store
            .heartbeat_turn(second.id, Uuid::new_v4(), heartbeat_at)
            .await,
        Err(ServerError::Conflict(_))
    ));
    assert_eq!(
        store
            .fail_stale_running_turns(
                heartbeat_at - Duration::seconds(1),
                Utc::now(),
                "server restarted",
            )
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.turn(second.id).await.unwrap().status, "running");
    assert!(matches!(
        other_store
            .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string(),)
            .await,
        Err(ServerError::Conflict(_))
    ));
    assert_eq!(
        store
            .fail_stale_running_turns(
                heartbeat_at + Duration::seconds(1),
                Utc::now(),
                "server restarted",
            )
            .await
            .unwrap(),
        1
    );
    assert_eq!(store.turn(second.id).await.unwrap().status, "failed");

    let ended = store
        .end_session_and_enqueue_dream(session.id, "stale-caller".to_string(), "global".to_string())
        .await
        .unwrap();
    assert_eq!(ended.dream.subject, "brian");
    assert_eq!(ended.dream.scope, "project:postgres-turns");
    assert!(matches!(
        other_store
            .set_session_memory_scope(session.id, "global")
            .await,
        Err(ServerError::Conflict(_))
    ));
    assert!(matches!(
        store
            .enqueue_turn(session.id, "postgres-message-3", "too late")
            .await,
        Err(ServerError::Conflict(_))
    ));
}

#[tokio::test]
async fn gated_postgres_approval_resolution_and_effect_claims_are_cross_instance_cas() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    let other_store = PostgresStore::connect(&dsn).await.unwrap();
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
    let approval_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let evolution = crate::evolution::evolution_effect_metadata(
        tm_host::SelfEvolutionTier::Conservative,
        tm_host::EvolutionTargetClass::ScopedMemory,
        proposal_id.to_string(),
        "postgres-test",
        session.id,
        None,
        &json!({"candidate": "cross-instance durable reminder"}),
    )
    .unwrap();
    store
        .create_approval_request(NewApprovalRequest {
            id: approval_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "postgres-test".to_string(),
            action: "memory.write".to_string(),
            scope_json: json!({"scope": "global"}),
            options_json: json!([
                {"id": "allow", "label": "Allow"},
                {"id": "deny", "label": "Deny"}
            ]),
            effect_type: "memory_write".to_string(),
            effect_payload_json: json!({
                "evolution": evolution,
                "proposalId": proposal_id,
            }),
            resumable: true,
            created_at: now,
            expires_at: now + Duration::minutes(5),
        })
        .await
        .unwrap();
    assert!(
        other_store
            .claim_approval_effect(approval_id, Uuid::new_v4(), now, Duration::seconds(30),)
            .await
            .unwrap()
            .is_none()
    );
    let request_event = store
        .append_event(session.id, "approval", json!({"approvalId": approval_id}))
        .await
        .unwrap();
    other_store
        .link_approval_event(session.id, approval_id, "approval", request_event.seq)
        .await
        .unwrap();

    let resolution = NewApprovalResolution {
        status: "approved".to_string(),
        selected_option_id: Some("allow".to_string()),
        resolution_json: json!({
            "approvalId": approval_id,
            "status": "approved",
            "outcome": "approved"
        }),
        resolved_at: now + Duration::seconds(1),
    };
    let (left, right) = tokio::join!(
        store.resolve_approval_request_with_event(session.id, approval_id, resolution.clone()),
        other_store.resolve_approval_request_with_event(session.id, approval_id, resolution),
    );
    assert_eq!(
        left.is_ok() as usize + right.is_ok() as usize,
        1,
        "cross-instance resolution results: left={left:?}, right={right:?}"
    );
    assert!(matches!(
        (&left, &right),
        (Ok(_), Err(ServerError::Conflict(_))) | (Err(ServerError::Conflict(_)), Ok(_))
    ));
    assert_eq!(
        other_store
            .approval_request(session.id, approval_id)
            .await
            .unwrap()
            .resolution_version,
        1
    );
    assert_eq!(
        store
            .events_after(session.id, None)
            .await
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "approval_resolved")
            .count(),
        1
    );

    let first_owner = Uuid::new_v4();
    let second_owner = Uuid::new_v4();
    let claim_at = now + Duration::seconds(2);
    let (left, right) = tokio::join!(
        store.claim_next_approval_effect(first_owner, claim_at, Duration::seconds(30)),
        other_store.claim_next_approval_effect(second_owner, claim_at, Duration::seconds(30)),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.is_some() as usize + right.is_some() as usize, 1);
    let first = left.or(right).expect("one effect claim");
    assert_eq!(first.effect.approval_id, approval_id);
    let record = RecallChunkRecord {
        id: proposal_id,
        scope: "global".to_string(),
        text: "cross-instance durable reminder".to_string(),
        source: "postgres-test".to_string(),
        importance: 1.0,
        created_at: claim_at,
    };
    store.upsert_recall_chunk(record.clone()).await.unwrap();
    let reclaimed = other_store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            claim_at + Duration::seconds(31),
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("stale claim is reclaimable across instances");
    other_store.upsert_recall_chunk(record).await.unwrap();
    let terminal_payload = json!({
        "kind": "memory",
        "proposalId": proposal_id,
        "status": "approved",
    });
    assert!(
        store
            .complete_approval_effect_with_event(
                &first,
                terminal_payload.clone(),
                None,
                claim_at + Duration::seconds(31),
            )
            .await
            .is_err()
    );
    let (applied, terminal_event) = other_store
        .complete_approval_effect_with_event(
            &reclaimed,
            terminal_payload,
            None,
            claim_at + Duration::seconds(32),
        )
        .await
        .unwrap();
    assert_eq!(applied.status, "applied");
    assert_eq!(terminal_event.event_type, "write_proposal");
    assert_eq!(
        store
            .recall_chunks("global", "", 10)
            .await
            .unwrap()
            .iter()
            .filter(|record| record.id == proposal_id)
            .count(),
        1
    );
    assert_eq!(
        store
            .events_after(session.id, None)
            .await
            .unwrap()
            .iter()
            .filter(|event| {
                event.event_type == "write_proposal"
                    && event.payload_json["proposalId"] == json!(proposal_id)
                    && event.payload_json["status"] == json!("approved")
            })
            .count(),
        1
    );
    let restarted = PostgresStore::connect(&dsn).await.unwrap();
    let audits = restarted.evolution_audits(session.id).await.unwrap();
    assert_eq!(audits.len(), 3);
    assert_eq!(
        audits
            .iter()
            .map(|record| record.status)
            .collect::<Vec<_>>(),
        vec![
            tm_host::EvolutionAuditStatus::AwaitingApproval,
            tm_host::EvolutionAuditStatus::Approved,
            tm_host::EvolutionAuditStatus::Applied,
        ]
    );

    let expired_id = Uuid::new_v4();
    store
        .create_approval_request(NewApprovalRequest {
            id: expired_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "postgres-test".to_string(),
            action: "code.run".to_string(),
            scope_json: json!({}),
            options_json: json!([]),
            effect_type: "approval_continuation".to_string(),
            effect_payload_json: json!({}),
            resumable: false,
            created_at: now - Duration::minutes(2),
            expires_at: now - Duration::minutes(1),
        })
        .await
        .unwrap();
    let expiry_events = other_store.expire_pending_approvals(now).await.unwrap();
    assert!(expiry_events.iter().any(|event| {
        event.payload_json["approvalId"] == json!(expired_id)
            && event.event_type == "approval_resolved"
    }));
    assert_eq!(
        store
            .approval_request(session.id, expired_id)
            .await
            .unwrap()
            .status,
        "timed_out"
    );
    assert!(
        store
            .claim_approval_effect(expired_id, Uuid::new_v4(), now, Duration::seconds(30))
            .await
            .unwrap()
            .is_some()
    );
}

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
    let proposals = in_memory
        .organize_with_config(tm_drive::DriveOrganizerConfig::default())
        .unwrap();
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

#[tokio::test]
async fn gated_postgres_runs_versioned_auth_migrations_and_redeems_once() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    for table in [
        "schema_migrations",
        "auth_devices",
        "pairing_codes",
        "evolution_audits",
        "evolution_review_proposals",
        "push_deliveries",
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
    let migrations = store
        .client()
        .query(
            "select version, name, checksum from schema_migrations order by version",
            &[],
        )
        .await
        .unwrap();
    assert!(migrations.len() >= 2);
    assert_eq!(migrations[0].get::<_, i64>("version"), 1);
    assert_eq!(migrations[1].get::<_, i64>("version"), 2);
    assert_eq!(migrations[0].get::<_, String>("checksum").len(), 64);
    assert_eq!(migrations[1].get::<_, String>("checksum").len(), 64);

    let owner_subject: String = store
        .client()
        .query_one(
            "select owner_subject from server_authority where singleton = true",
            &[],
        )
        .await
        .unwrap()
        .get("owner_subject");

    let now = Utc::now();
    let raw_code = format!("postgres-pair-{}", Uuid::new_v4());
    store
        .create_pairing_code(crate::NewPairingCode {
            id: Uuid::new_v4(),
            code_hash: crate::auth::hash_secret(&raw_code),
            created_at: now,
            expires_at: now + Duration::minutes(5),
            created_by_device_id: None,
        })
        .await
        .unwrap();
    let raw_token = format!("postgres-device-{}", Uuid::new_v4());
    let device = store
        .consume_pairing_code(
            &crate::auth::hash_secret(&raw_code),
            crate::NewAuthDevice {
                id: Uuid::new_v4(),
                owner_subject: owner_subject.clone(),
                name: "Postgres auth test".to_string(),
                platform: "test".to_string(),
                token_hash: crate::auth::hash_secret(&raw_token),
                created_at: now,
            },
            now,
        )
        .await
        .unwrap();
    assert!(
        store
            .authenticate_device(&crate::auth::hash_secret(&raw_token), Utc::now())
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .consume_pairing_code(
                &crate::auth::hash_secret(&raw_code),
                crate::NewAuthDevice {
                    id: Uuid::new_v4(),
                    owner_subject: owner_subject.clone(),
                    name: "Replay".to_string(),
                    platform: "test".to_string(),
                    token_hash: crate::auth::hash_secret("replay-token"),
                    created_at: now,
                },
                now,
            )
            .await
            .is_err()
    );
    store
        .revoke_auth_device(device.id, Utc::now())
        .await
        .unwrap();
    assert!(
        store
            .authenticate_device(&crate::auth::hash_secret(&raw_token), Utc::now())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn gated_postgres_push_outbox_delivers_request_and_resolution_once() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    store.configure_owner_subject("brian").await.unwrap();
    let now = Utc::now();
    let raw_code = format!("push-pair-{}", Uuid::new_v4());
    store
        .create_pairing_code(crate::NewPairingCode {
            id: Uuid::new_v4(),
            code_hash: crate::auth::hash_secret(&raw_code),
            created_at: now,
            expires_at: now + Duration::minutes(5),
            created_by_device_id: None,
        })
        .await
        .unwrap();
    let device = store
        .consume_pairing_code(
            &crate::auth::hash_secret(&raw_code),
            crate::NewAuthDevice {
                id: Uuid::new_v4(),
                owner_subject: "brian".to_string(),
                name: "Push outbox test".to_string(),
                platform: "android".to_string(),
                token_hash: crate::auth::hash_secret(&format!("push-device-{}", Uuid::new_v4())),
                created_at: now,
            },
            now,
        )
        .await
        .unwrap();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let approval_id = Uuid::new_v4();
    store
        .create_approval_request(NewApprovalRequest {
            id: approval_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "native-deno".to_string(),
            action: "proc.run cargo test".to_string(),
            scope_json: json!({"capability": "proc.run"}),
            options_json: json!([
                {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
            ]),
            effect_type: "approval_continuation".to_string(),
            effect_payload_json: json!({}),
            resumable: true,
            created_at: now,
            expires_at: now + Duration::minutes(5),
        })
        .await
        .unwrap();
    let request_event = store
        .append_event(session.id, "approval", json!({"approvalId": approval_id}))
        .await
        .unwrap();
    store
        .link_approval_event(session.id, approval_id, "approval", request_event.seq)
        .await
        .unwrap();

    let provider = Arc::new(FakePushProvider::default());
    let cipher = PushCipher::generate_for_tests();
    let service = PushService::new(
        Arc::new(PostgresPushStore::connect(&dsn).await.unwrap()),
        provider.clone(),
        cipher.clone(),
    );
    let other_service = PushService::new(
        Arc::new(PostgresPushStore::connect(&dsn).await.unwrap()),
        provider.clone(),
        cipher,
    );
    service
        .register(device.id, "fake", "opaque-registration")
        .await
        .unwrap();
    let (first_worker, second_worker) = tokio::join!(
        service.tick(Uuid::new_v4()),
        other_service.tick(Uuid::new_v4())
    );
    assert_eq!(first_worker.unwrap() + second_worker.unwrap(), 1);
    let deliveries = provider.deliveries();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].1.kind, PushMessageKind::ApprovalRequested);
    assert_eq!(deliveries[0].1.session_id, session.id);
    assert_eq!(deliveries[0].1.approval_id, Some(approval_id));

    store
        .resolve_approval_request_with_event(
            session.id,
            approval_id,
            NewApprovalResolution {
                status: "denied".to_string(),
                selected_option_id: Some("reject".to_string()),
                resolution_json: json!({
                    "approvalId": approval_id,
                    "status": "denied"
                }),
                resolved_at: Utc::now(),
            },
        )
        .await
        .unwrap();
    let (first_worker, second_worker) = tokio::join!(
        service.tick(Uuid::new_v4()),
        other_service.tick(Uuid::new_v4())
    );
    assert_eq!(first_worker.unwrap() + second_worker.unwrap(), 1);
    let deliveries = provider.deliveries();
    assert_eq!(deliveries.len(), 2);
    assert_eq!(deliveries[1].1.kind, PushMessageKind::ApprovalResolved);

    let final_event = store
        .append_event(
            session.id,
            "final",
            json!({"text": "private assistant reply"}),
        )
        .await
        .unwrap();
    let (first_worker, second_worker) = tokio::join!(
        service.tick(Uuid::new_v4()),
        other_service.tick(Uuid::new_v4())
    );
    assert_eq!(first_worker.unwrap() + second_worker.unwrap(), 1);
    let deliveries = provider.deliveries();
    assert_eq!(deliveries.len(), 3);
    assert_eq!(deliveries[2].1.kind, PushMessageKind::SessionReady);
    assert_eq!(deliveries[2].1.session_id, session.id);
    assert_eq!(deliveries[2].1.event_seq, Some(final_event.seq));
    assert_eq!(deliveries[2].1.approval_id, None);
    assert!(
        !serde_json::to_string(&deliveries[2].1)
            .unwrap()
            .contains("private assistant reply")
    );
    assert_eq!(service.runtime_metrics().await.unwrap().queue_depth, 0);
}

#[tokio::test]
async fn gated_postgres_upgrades_legacy_base_schema_without_losing_history() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_legacy_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();

    let mut legacy_config = dsn.parse::<tokio_postgres::Config>().unwrap();
    legacy_config.options(format!("-c search_path={schema}"));
    let (legacy_client, legacy_connection) =
        legacy_config.connect(tokio_postgres::NoTls).await.unwrap();
    let legacy_connection_task = tokio::spawn(async move {
        legacy_connection.await.unwrap();
    });
    legacy_client
        .batch_execute(include_str!("../../migrations/0001_base.sql"))
        .await
        .unwrap();
    let session_id = Uuid::new_v4();
    let created_at = Utc::now() - Duration::days(30);
    let mode = serde_json::to_value(ModeId::from("general")).unwrap();
    let persona_status = serde_json::to_value(AssetStatus::Degraded {
        warning: "legacy deployment".to_string(),
    })
    .unwrap();
    legacy_client
        .execute(
            "insert into sessions
                (id, created_at, updated_at, status, mode, mode_state_json, persona_status)
             values ($1, $2, $2, 'open', $3, null, $4)",
            &[&session_id, &created_at, &mode, &persona_status],
        )
        .await
        .unwrap();
    legacy_client
        .execute(
            "insert into messages(session_id, seq, role, content, created_at)
             values ($1, 1, 'user', 'preserve this legacy message', $2)",
            &[&session_id, &created_at],
        )
        .await
        .unwrap();
    drop(legacy_client);
    legacy_connection_task.await.unwrap();

    let upgraded = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let session = upgraded.get_session(session_id).await.unwrap();
    assert_eq!(session.status, "open");
    assert_eq!(session.owner_subject, "owner");
    assert_eq!(session.memory_scope, "global");
    let messages = upgraded.session_messages(session_id).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "preserve this legacy message");
    let migrations = upgraded
        .client()
        .query(
            "select version, checksum from schema_migrations order by version",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(migrations.len(), 13);
    assert!(migrations.iter().enumerate().all(|(index, row)| {
        row.get::<_, i64>("version") == index as i64 + 1
            && row.get::<_, String>("checksum").len() == 64
    }));

    drop(upgraded);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}

#[derive(Debug, PartialEq, Eq)]
struct LogicalDriveSnapshot {
    path: String,
    uri: String,
    project: Option<String>,
    doc_kind: Option<String>,
    tags: Vec<String>,
    content_hash: String,
    source_uri: Option<String>,
    proposal_status: String,
}

impl LogicalDriveSnapshot {
    fn from_memory(entry: &tm_drive::DriveEntry, proposal: &tm_drive::OrganizerProposal) -> Self {
        let mut tags = entry.tags.clone();
        tags.sort();
        Self {
            path: entry.path.clone(),
            uri: entry.uri.clone(),
            project: entry.project.clone(),
            doc_kind: entry.doc_kind.clone(),
            tags,
            content_hash: entry.content_hash.clone(),
            source_uri: entry.source_uri.clone(),
            proposal_status: serde_json::to_value(&proposal.status)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
        }
    }
}

async fn insert_postgres_drive_snapshot(
    store: &PostgresStore,
    entry: &tm_drive::DriveEntry,
    proposal: &tm_drive::OrganizerProposal,
) {
    let size_bytes = entry.size_bytes as i64;
    let provenance = serde_json::to_value(&entry.provenance).unwrap();
    let entry_record = serde_json::to_value(entry).unwrap();
    let entry_version = entry.version as i64;
    store
        .client()
        .execute(
            "insert into drive_entries (id, path, uri, blob_uri, content_hash, mime, size_bytes, title, doc_kind, project, source_uri, provenance_json, summary, status, created_at, updated_at, version, record_json)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)",
            &[
                &entry.id,
                &entry.path,
                &entry.uri,
                &entry.blob_uri,
                &entry.content_hash,
                &entry.mime,
                &size_bytes,
                &entry.title,
                &entry.doc_kind,
                &entry.project,
                &entry.source_uri,
                &provenance,
                &entry.summary,
                &"active",
                &entry.created_at,
                &entry.updated_at,
                &entry_version,
                &entry_record,
            ],
        )
        .await
        .unwrap();
    for (idx, attribute) in entry.attributes.iter().enumerate() {
        let idx = idx as i32;
        let evidence = serde_json::to_value(&attribute.evidence).unwrap();
        store
            .client()
            .execute(
                "insert into drive_attributes (entry_id, idx, key, value, confidence, evidence_json, extractor, source_uri, session_id, event_seq, content_hash)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                &[
                    &entry.id,
                    &idx,
                    &attribute.key,
                    &attribute.value,
                    &attribute.confidence,
                    &evidence,
                    &attribute.extractor,
                    &attribute.source_uri,
                    &attribute.session_id,
                    &attribute.event_seq,
                    &attribute.content_hash,
                ],
            )
            .await
            .unwrap();
    }
    for tag in &entry.tags {
        store
            .client()
            .execute(
                "insert into drive_tags (entry_id, tag) values ($1, $2) on conflict do nothing",
                &[&entry.id, tag],
            )
            .await
            .unwrap();
    }
    let proposed_tags = serde_json::to_value(&proposal.proposed_tags).unwrap();
    let evidence = serde_json::to_value(&proposal.evidence).unwrap();
    let replay_metadata = serde_json::to_value(&proposal.replay_metadata).unwrap();
    let action = serde_json::to_value(&proposal.action)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    let policy_decision = serde_json::to_value(&proposal.policy_decision)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    let status = serde_json::to_value(&proposal.status)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    let proposal_record = serde_json::to_value(proposal).unwrap();
    let proposal_version = proposal.version as i64;
    store
        .client()
        .execute(
            "insert into drive_proposals (id, action, entry_id, source_path, proposed_path, proposed_tags, proposed_doc_kind, proposed_project, evidence_json, confidence, policy_decision, approval_id, status, source_run_id, replay_metadata, created_at, updated_at, version, entry_id_snapshot, record_json)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $3, $19)",
            &[
                &proposal.id,
                &action,
                &entry.id,
                &proposal.source_path,
                &proposal.proposed_path,
                &proposed_tags,
                &proposal.proposed_doc_kind,
                &proposal.proposed_project,
                &evidence,
                &proposal.confidence,
                &policy_decision,
                &proposal.approval_id,
                &status,
                &proposal.source_run_id,
                &replay_metadata,
                &proposal.created_at,
                &proposal.updated_at,
                &proposal_version,
                &proposal_record,
            ],
        )
        .await
        .unwrap();
}

async fn postgres_drive_snapshot(
    store: &PostgresStore,
    uri: &str,
    proposal_id: Uuid,
) -> LogicalDriveSnapshot {
    let row = store
        .client()
        .query_one(
            "select id, path, uri, project, doc_kind, content_hash, source_uri from drive_entries where uri = $1",
            &[&uri],
        )
        .await
        .unwrap();
    let entry_id: Uuid = row.get("id");
    let mut tags = store
        .client()
        .query(
            "select tag from drive_tags where entry_id = $1 order by tag asc",
            &[&entry_id],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<_, String>("tag"))
        .collect::<Vec<_>>();
    tags.sort();
    let status: String = store
        .client()
        .query_one(
            "select status from drive_proposals where id = $1 and entry_id = $2",
            &[&proposal_id, &entry_id],
        )
        .await
        .unwrap()
        .get("status");
    LogicalDriveSnapshot {
        path: row.get("path"),
        uri: row.get("uri"),
        project: row.get("project"),
        doc_kind: row.get("doc_kind"),
        tags,
        content_hash: row.get("content_hash"),
        source_uri: row.get("source_uri"),
        proposal_status: status,
    }
}

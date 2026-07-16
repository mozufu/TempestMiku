use super::*;

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
    let (left, right) = tokio::join!(
        store.append_event_for_turn_once(
            first_session.id,
            "memory_recall",
            json!({"context": "left"}),
            first.id,
        ),
        store.append_event_for_turn_once(
            first_session.id,
            "memory_recall",
            json!({"context": "right"}),
            first.id,
        ),
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(left.1 as usize + right.1 as usize, 1);
    assert_eq!(left.0.seq, right.0.seq);
    assert_eq!(left.0.payload_json, right.0.payload_json);
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

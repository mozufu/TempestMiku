use super::*;

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

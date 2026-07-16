use super::*;

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
    let (left, right) = tokio::join!(
        store.append_event_for_turn_once(
            session.id,
            "memory_recall",
            json!({
                "context": {
                    "subject": "brian",
                    "scope": "project:postgres-turns",
                    "source": "left"
                }
            }),
            first.id,
        ),
        other_store.append_event_for_turn_once(
            session.id,
            "memory_recall",
            json!({
                "context": {
                    "subject": "brian",
                    "scope": "project:postgres-turns",
                    "source": "right"
                }
            }),
            first.id,
        ),
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(left.1 as usize + right.1 as usize, 1);
    assert_eq!(left.0.seq, right.0.seq);
    assert_eq!(left.0.payload_json, right.0.payload_json);
    assert_eq!(
        store
            .events_by_type(session.id, "memory_recall", 10)
            .await
            .unwrap()
            .len(),
        1
    );
    store
        .append_event_for_turn(
            session.id,
            "memory_recall",
            json!({
                "context": {
                    "subject": "brian",
                    "scope": "project:other"
                }
            }),
            Some(second.id),
        )
        .await
        .unwrap();
    let authorized_recalls = store
        .memory_recall_events(session.id, "brian", "project:postgres-turns", 1)
        .await
        .unwrap();
    assert_eq!(authorized_recalls.len(), 1);
    assert_eq!(authorized_recalls[0].seq, left.0.seq);
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

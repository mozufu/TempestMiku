use super::*;

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

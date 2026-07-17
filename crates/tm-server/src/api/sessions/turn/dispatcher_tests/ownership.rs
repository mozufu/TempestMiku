use super::*;

#[tokio::test]
async fn complete_turn_failure_aborts_runtime_before_same_session_retry() {
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    let runner = Arc::new(LinearizedRunner::new(
        Arc::clone(&store),
        OwnershipLossPoint::BeforeComplete,
    ));
    let persona = tm_modes::ModesConfig::default();
    let assets = persona.load_assets();
    let session = store
        .create_session(NewSession {
            mode: assets.modes.default_mode(),
            persona_status: assets.status.clone(),
        })
        .await
        .unwrap();
    let state = AppState::new(
        Arc::clone(&store),
        memory,
        Arc::clone(&runner),
        persona,
        AuthConfig::NoAuth,
    )
    .with_auto_turn_dispatcher(false);
    let worker_id = Uuid::new_v4();

    store
        .enqueue_turn(session.id, "complete-fails", "first")
        .await
        .unwrap();
    let first = store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .unwrap();
    let error = execute_claimed_turn(&state, worker_id, first, Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("is not owned"), "{error}");
    assert_eq!(runner.committed.load(Ordering::SeqCst), 0);
    assert_eq!(runner.aborts.load(Ordering::SeqCst), 1);

    store
        .enqueue_turn(session.id, "complete-retry", "second")
        .await
        .unwrap();
    let second = store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .unwrap();
    execute_claimed_turn(&state, worker_id, second, Duration::from_secs(1))
        .await
        .unwrap();

    assert_eq!(*runner.observed.lock().unwrap(), vec![1, 1]);
    assert_eq!(runner.committed.load(Ordering::SeqCst), 1);
    assert_eq!(runner.promotions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn heartbeat_ownership_loss_cancels_and_evicts_before_same_session_retry() {
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    let runner = Arc::new(LinearizedRunner::new(
        Arc::clone(&store),
        OwnershipLossPoint::DuringHeartbeat,
    ));
    let persona = tm_modes::ModesConfig::default();
    let assets = persona.load_assets();
    let session = store
        .create_session(NewSession {
            mode: assets.modes.default_mode(),
            persona_status: assets.status.clone(),
        })
        .await
        .unwrap();
    let state = AppState::new(
        Arc::clone(&store),
        memory,
        Arc::clone(&runner),
        persona,
        AuthConfig::NoAuth,
    )
    .with_auto_turn_dispatcher(false);
    let worker_id = Uuid::new_v4();

    store
        .enqueue_turn(session.id, "heartbeat-lost", "first")
        .await
        .unwrap();
    let first = store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .unwrap();
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        execute_claimed_turn(&state, worker_id, first, Duration::from_millis(5)),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(error.to_string().contains("is not owned"), "{error}");
    assert!(runner.cancelled.load(Ordering::SeqCst));
    assert_eq!(runner.committed.load(Ordering::SeqCst), 0);
    assert_eq!(runner.aborts.load(Ordering::SeqCst), 1);

    store
        .enqueue_turn(session.id, "heartbeat-retry", "second")
        .await
        .unwrap();
    let second = store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .unwrap();
    execute_claimed_turn(&state, worker_id, second, Duration::from_secs(1))
        .await
        .unwrap();

    assert_eq!(*runner.observed.lock().unwrap(), vec![1, 1]);
    assert_eq!(runner.committed.load(Ordering::SeqCst), 1);
    assert_eq!(runner.promotions.load(Ordering::SeqCst), 1);
}

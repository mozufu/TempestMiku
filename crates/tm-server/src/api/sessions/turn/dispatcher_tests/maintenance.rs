use super::*;

#[tokio::test]
async fn dispatcher_runs_different_sessions_concurrently() {
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    let runner = Arc::new(ConcurrentRunner::default());
    let persona = tm_modes::ModesConfig::default();
    let assets = persona.load_assets();
    let state = AppState::new(
        Arc::clone(&store),
        memory,
        Arc::clone(&runner),
        persona,
        AuthConfig::NoAuth,
    )
    .with_auto_turn_dispatcher(false);

    let mut turn_ids = Vec::new();
    for index in 0..3 {
        let session = store
            .create_session(NewSession {
                mode: assets.modes.default_mode(),
                persona_status: assets.status.clone(),
            })
            .await
            .unwrap();
        let turn = store
            .enqueue_turn(session.id, &format!("client-{index}"), "hello")
            .await
            .unwrap();
        turn_ids.push(turn.id);
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let dispatcher =
        start_supervised_turn_dispatcher(state, shutdown_rx, 3, Duration::from_millis(5));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let mut all_complete = true;
            for turn_id in &turn_ids {
                all_complete &= store.turn(*turn_id).await.unwrap().status == "completed";
            }
            if all_complete {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    shutdown_tx.send(true).unwrap();
    dispatcher.await.unwrap();

    assert!(
        runner.max_active.load(Ordering::SeqCst) >= 2,
        "turns from independent sessions should overlap"
    );
}

#[tokio::test]
async fn dispatcher_heartbeats_live_turns_during_periodic_stale_sweeps() {
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    let persona = tm_modes::ModesConfig::default();
    let assets = persona.load_assets();
    let session = store
        .create_session(NewSession {
            mode: assets.modes.default_mode(),
            persona_status: assets.status.clone(),
        })
        .await
        .unwrap();
    let turn = store
        .enqueue_turn(session.id, "heartbeat-live", "take your time")
        .await
        .unwrap();
    let state = AppState::new(
        Arc::clone(&store),
        memory,
        Arc::new(SlowRunner),
        persona,
        AuthConfig::NoAuth,
    )
    .with_auto_turn_dispatcher(false);
    let notify = Arc::clone(&state.turn_notify);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let dispatcher = tokio::spawn(run_turn_dispatcher(
        state,
        notify,
        shutdown_rx,
        1,
        Duration::from_millis(2),
        TurnMaintenance {
            heartbeat_interval: Duration::from_millis(10),
            stale_timeout: Duration::from_millis(80),
            stale_sweep_interval: Duration::from_millis(10),
        },
    ));

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if store.turn(turn.id).await.unwrap().status == "completed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    shutdown_tx.send(true).unwrap();
    dispatcher.await.unwrap();
    let completed = store.turn(turn.id).await.unwrap();
    assert_eq!(completed.status, "completed");
    assert!(completed.updated_at > completed.started_at.unwrap());
}

#[tokio::test]
async fn dispatcher_periodically_fails_crashed_turns_without_a_restart() {
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    let persona = tm_modes::ModesConfig::default();
    let assets = persona.load_assets();
    let session = store
        .create_session(NewSession {
            mode: assets.modes.default_mode(),
            persona_status: assets.status.clone(),
        })
        .await
        .unwrap();
    let turn = store
        .enqueue_turn(session.id, "heartbeat-crashed", "lose the worker")
        .await
        .unwrap();
    store
        .claim_next_turn(
            Uuid::new_v4(),
            Utc::now() + chrono::Duration::milliseconds(200),
        )
        .await
        .unwrap()
        .expect("external worker claims turn");
    let state = AppState::new(
        Arc::clone(&store),
        memory,
        Arc::new(ConcurrentRunner::default()),
        persona,
        AuthConfig::NoAuth,
    )
    .with_auto_turn_dispatcher(false);
    let notify = Arc::clone(&state.turn_notify);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let dispatcher = tokio::spawn(run_turn_dispatcher(
        state,
        notify,
        shutdown_rx,
        1,
        Duration::from_millis(2),
        TurnMaintenance {
            heartbeat_interval: Duration::from_millis(10),
            stale_timeout: Duration::from_millis(40),
            stale_sweep_interval: Duration::from_millis(10),
        },
    ));

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if store.turn(turn.id).await.unwrap().status == "failed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    shutdown_tx.send(true).unwrap();
    dispatcher.await.unwrap();
    let failed = store.turn(turn.id).await.unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(
        failed.error.as_deref(),
        Some("turn owner heartbeat expired")
    );
}

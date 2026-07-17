use super::*;

#[tokio::test]
async fn expired_session_is_reopened_with_clean_state() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::ZERO), Duration::ZERO);
    let session_id = Uuid::from_u128(9);
    let reset_sink = Arc::new(ResetSink::default());

    run(&runner, chat_turn(session_id)).await;
    assert_eq!(reset_sink.0.load(Ordering::SeqCst), 0);
    let mut resumed_turn = chat_turn(session_id);
    resumed_turn.prior_messages = vec![Message::user("persisted earlier turn")];
    let sink: Arc<dyn EventSink + Send + Sync> = reset_sink.clone();
    assert_eq!(runner.run_turn(resumed_turn, sink).await.unwrap(), "done");

    assert_eq!(
        *harness.evaluations.lock().unwrap(),
        vec![(session_id, 1), (session_id, 1)]
    );
    assert_eq!(harness.opens.lock().unwrap().len(), 2);
    assert_eq!(reset_sink.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nonempty_history_without_live_state_emits_runtime_reset() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let reset_sink = Arc::new(ResetSink::default());
    let mut resumed_turn = chat_turn(Uuid::from_u128(11));
    resumed_turn.prior_messages = vec![
        Message::user("persisted question"),
        Message::assistant("persisted answer"),
    ];
    let sink: Arc<dyn EventSink + Send + Sync> = reset_sink.clone();

    assert_eq!(runner.run_turn(resumed_turn, sink).await.unwrap(), "done");
    assert_eq!(reset_sink.0.load(Ordering::SeqCst), 1);

    run(&runner, chat_turn(Uuid::from_u128(12))).await;
    assert_eq!(
        reset_sink.0.load(Ordering::SeqCst),
        1,
        "brand-new empty-history sessions must not emit runtime_reset"
    );
}

#[tokio::test]
async fn runtime_reset_backpressure_retries_before_the_model_turn() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let mut resumed_turn = chat_turn(Uuid::from_u128(13));
    resumed_turn.prior_messages = vec![Message::user("persisted question")];
    let sink: Arc<dyn EventSink + Send + Sync> = Arc::new(FailingResetSink);

    let error = runner
        .run_turn(resumed_turn.clone(), sink)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("runtime_reset backpressure"));
    assert!(harness.llm.requests.lock().unwrap().is_empty());
    assert!(harness.evaluations.lock().unwrap().is_empty());

    let reset_sink = Arc::new(ResetSink::default());
    let sink: Arc<dyn EventSink + Send + Sync> = reset_sink.clone();
    assert_eq!(
        runner.run_turn(resumed_turn.clone(), sink).await.unwrap(),
        "done"
    );
    assert_eq!(reset_sink.0.load(Ordering::SeqCst), 1);

    let sink: Arc<dyn EventSink + Send + Sync> = reset_sink.clone();
    assert_eq!(runner.run_turn(resumed_turn, sink).await.unwrap(), "done");
    assert_eq!(
        reset_sink.0.load(Ordering::SeqCst),
        1,
        "an acknowledged runtime reset must not be emitted again"
    );
}

#[tokio::test]
async fn runtime_reset_persistence_failure_retries_before_the_model_turn() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: tm_modes::AssetStatus::Degraded {
                warning: "test assets".to_string(),
            },
        })
        .await
        .unwrap();
    let (sender, _receiver) = tokio::sync::broadcast::channel(32);
    let mut resumed_turn = chat_turn(session.id);
    resumed_turn.prior_messages = vec![Message::user("persisted question")];

    let failing_sink = Arc::new(PersistingEventSink::new(
        Uuid::nil(),
        Arc::clone(&store),
        sender.clone(),
    ));
    let sink: Arc<dyn EventSink + Send + Sync> = failing_sink.clone();
    let error = runner
        .run_turn(resumed_turn.clone(), sink)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("session"), "{error}");
    assert!(harness.llm.requests.lock().unwrap().is_empty());
    assert!(harness.evaluations.lock().unwrap().is_empty());
    assert!(failing_sink.flush().await.is_err());

    let persisted_sink = Arc::new(PersistingEventSink::new(
        session.id,
        Arc::clone(&store),
        sender.clone(),
    ));
    let sink: Arc<dyn EventSink + Send + Sync> = persisted_sink.clone();
    assert_eq!(
        runner.run_turn(resumed_turn.clone(), sink).await.unwrap(),
        "done"
    );
    persisted_sink.flush().await.unwrap();

    let later_sink = Arc::new(PersistingEventSink::new(
        session.id,
        Arc::clone(&store),
        sender,
    ));
    let sink: Arc<dyn EventSink + Send + Sync> = later_sink.clone();
    assert_eq!(runner.run_turn(resumed_turn, sink).await.unwrap(), "done");
    later_sink.flush().await.unwrap();

    let event_types = store
        .events_after(session.id, None)
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert_eq!(
        event_types.first().map(String::as_str),
        Some("runtime_reset")
    );
    assert_eq!(
        event_types
            .iter()
            .filter(|event_type| event_type.as_str() == "runtime_reset")
            .count(),
        1,
        "an acknowledged runtime reset must not be persisted again"
    );
}

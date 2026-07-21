use super::*;
use crate::session_shards::shard_index;

#[tokio::test]
async fn same_session_reuses_non_send_state() {
    let harness = Harness::new();
    let runner = harness.runner(options(2, Duration::from_secs(60)), Duration::ZERO);
    let session_id = Uuid::from_u128(1);

    run(&runner, chat_turn(session_id)).await;
    run(&runner, chat_turn(session_id)).await;

    assert_eq!(
        *harness.evaluations.lock().unwrap(),
        vec![(session_id, 1), (session_id, 2)]
    );
    assert_eq!(harness.opens.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn durable_state_is_reused_only_after_matching_promotion() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let session_id = Uuid::from_u128(31);
    let committed_id = Uuid::from_u128(3101);
    let aborted_id = Uuid::from_u128(3102);

    let mut committed = chat_turn(session_id);
    committed.durable_turn_id = Some(committed_id);
    run(&runner, committed).await;
    runner
        .promote_session(session_id, committed_id)
        .await
        .unwrap();

    let mut aborted = chat_turn(session_id);
    aborted.durable_turn_id = Some(aborted_id);
    run(&runner, aborted).await;
    runner.abort_session(session_id, aborted_id).await.unwrap();

    run(&runner, chat_turn(session_id)).await;

    assert_eq!(
        *harness.evaluations.lock().unwrap(),
        vec![(session_id, 1), (session_id, 2), (session_id, 1)],
        "the committed state may advance once, but the aborted mutation must be evicted"
    );
    assert_eq!(harness.opens.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn unpromoted_durable_state_is_never_observed_by_the_next_turn() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let session_id = Uuid::from_u128(32);
    let mut uncommitted = chat_turn(session_id);
    uncommitted.durable_turn_id = Some(Uuid::from_u128(3201));

    run(&runner, uncommitted).await;
    run(&runner, chat_turn(session_id)).await;

    assert_eq!(
        *harness.evaluations.lock().unwrap(),
        vec![(session_id, 1), (session_id, 1)]
    );
    assert_eq!(harness.opens.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn capability_downgrade_reopens_session_without_prior_authority_or_state() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let session_id = Uuid::from_u128(23);
    let mut elevated = chat_turn(session_id);
    elevated.capabilities = vec![
        "http.request".to_string(),
        "resources.read:artifact".to_string(),
    ];

    run(&runner, elevated).await;
    run(&runner, chat_turn(session_id)).await;

    assert_eq!(
        *harness.evaluations.lock().unwrap(),
        vec![(session_id, 1), (session_id, 1)],
        "a downgraded turn must not reuse the elevated interpreter"
    );
    assert_eq!(harness.opens.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn failed_turn_discards_unconfirmed_cached_session_state() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let session_id = Uuid::from_u128(21);
    let failing_sink: Arc<dyn EventSink + Send + Sync> = Arc::new(FailingCellResultSink);

    let error = runner
        .run_turn(chat_turn(session_id), failing_sink)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cell result persistence failed"));

    run(&runner, chat_turn(session_id)).await;

    assert_eq!(
        *harness.evaluations.lock().unwrap(),
        vec![(session_id, 1), (session_id, 1)],
        "the failed turn's mutated live session must not survive a retry"
    );
    assert_eq!(harness.opens.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn failed_persistence_barrier_discards_cached_session_state() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let session_id = Uuid::from_u128(22);
    let failing_sink: Arc<dyn EventSink + Send + Sync> = Arc::new(FailingBarrierSink);

    let error = runner
        .run_turn(chat_turn(session_id), failing_sink)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("event persistence barrier failed")
    );

    run(&runner, chat_turn(session_id)).await;

    assert_eq!(
        *harness.evaluations.lock().unwrap(),
        vec![(session_id, 1), (session_id, 1)],
        "a turn that fails its final event barrier must not retain runtime state"
    );
    assert_eq!(harness.opens.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn different_sessions_keep_isolated_state() {
    let harness = Harness::new();
    let runner = harness.runner(options(2, Duration::from_secs(60)), Duration::ZERO);
    let first = Uuid::from_u128(1);
    let second = Uuid::from_u128(2);

    run(&runner, chat_turn(first)).await;
    run(&runner, chat_turn(second)).await;
    run(&runner, chat_turn(first)).await;

    assert_eq!(
        *harness.evaluations.lock().unwrap(),
        vec![(first, 1), (second, 1), (first, 2)]
    );
}

#[tokio::test]
async fn dropping_runner_joins_workers_and_destroys_cached_sessions() {
    let harness = Harness::new();
    let runner = harness.runner(options(2, Duration::from_secs(60)), Duration::ZERO);

    run(&runner, chat_turn(Uuid::from_u128(1))).await;
    run(&runner, chat_turn(Uuid::from_u128(2))).await;
    assert_eq!(harness.drops.load(Ordering::SeqCst), 0);

    drop(runner);

    assert_eq!(harness.drops.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn deterministic_shards_serialize_same_session_turns() {
    let harness = Harness::new();
    let runner = harness.runner(
        options(3, Duration::from_secs(60)),
        Duration::from_millis(20),
    );
    let session_id = Uuid::from_u128(4);
    assert_eq!(shard_index(session_id, 3), 1);

    let (first, second) = tokio::join!(
        runner.run_turn(chat_turn(session_id), Arc::new(NullSink)),
        runner.run_turn(chat_turn(session_id), Arc::new(NullSink)),
    );
    assert_eq!(first.unwrap(), "done");
    assert_eq!(second.unwrap(), "done");
    assert_eq!(harness.max_active.load(Ordering::SeqCst), 1);
    assert_eq!(
        *harness.evaluations.lock().unwrap(),
        vec![(session_id, 1), (session_id, 2)]
    );
    assert_eq!(harness.opens.lock().unwrap()[0].1, "tm-agent-shard-1");
}

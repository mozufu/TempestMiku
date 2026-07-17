use super::*;

#[tokio::test]
async fn tm_backend_uses_cached_session_and_forwards_structured_events() {
    let temp = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let llm = Arc::new(ReactiveLlm::with_code(
        "[{patch: \"secret\"}] |> par map @fs.patch |> display {kind: \"table\"}",
    ));
    let llm_client: Arc<dyn LlmClient> = llm;
    let mut base_options = TmSandboxOptions {
        artifact_root: temp.path().join("artifacts"),
        ..TmSandboxOptions::default()
    };
    base_options.approval_policy = Arc::new(AlwaysApprove);
    let runner = AgentChatRunner::tm_with_options(
        llm_client,
        AgentConfig {
            model: "fake".into(),
            max_turns: 3,
            ..AgentConfig::default()
        },
        base_options,
        options(1, Duration::from_secs(60)),
    );
    let sink = Arc::new(RuntimeSink::default());
    let event_sink: Arc<dyn EventSink + Send + Sync> = sink.clone();
    let mut turn = chat_turn(Uuid::from_u128(20));
    turn.capabilities = vec!["fs.patch".into()];
    turn.host_functions = vec![Arc::new(ApprovedPatch::new(Arc::clone(&calls)))];
    assert_eq!(runner.run_turn(turn, event_sink).await.unwrap(), "done");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let events = sink.0.lock().unwrap();
    for expected in [
        "scope_start",
        "scope_result",
        "effect_start",
        "effect_suspended",
        "effect_resumed",
        "effect_result",
        "display",
        "binding_committed",
    ] {
        assert!(
            events.iter().any(|(event, _)| event == expected),
            "missing {expected}: {events:?}"
        );
    }
    let effect_start = events
        .iter()
        .find(|(kind, _)| kind == "effect_start")
        .unwrap();
    assert_eq!(effect_start.1["argsPreview"], "[redacted]");
}

#[tokio::test]
async fn host_event_proxy_routes_only_while_a_turn_is_bound() {
    let proxy = SwappableHostEventSink::default();
    let first = Arc::new(RuntimeSink::default());
    let first_sink: Arc<dyn EventSink + Send + Sync> = first.clone();
    proxy.bind(first_sink);
    proxy
        .emit("first", serde_json::json!({"turn": 1}))
        .await
        .unwrap();

    proxy.clear();
    let error = proxy
        .emit("between_turns", serde_json::json!({"ignored": true}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("without an active turn sink"));

    let second = Arc::new(RuntimeSink::default());
    let second_sink: Arc<dyn EventSink + Send + Sync> = second.clone();
    proxy.bind(second_sink);
    proxy
        .emit("second", serde_json::json!({"turn": 2}))
        .await
        .unwrap();

    assert_eq!(
        *first.0.lock().unwrap(),
        vec![("first".to_string(), serde_json::json!({"turn": 1}))]
    );
    assert_eq!(
        *second.0.lock().unwrap(),
        vec![("second".to_string(), serde_json::json!({"turn": 2}))]
    );
}

#[tokio::test]
async fn host_event_proxy_awaits_async_confirmed_delivery() {
    let proxy = SwappableHostEventSink::default();
    let sink: Arc<dyn EventSink + Send + Sync> = Arc::new(AsyncConfirmedFailureSink);
    proxy.bind(sink);

    let error = proxy
        .emit("binding_committed", serde_json::json!({"cellId": "cell-1"}))
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("async runtime persistence failed")
    );
}

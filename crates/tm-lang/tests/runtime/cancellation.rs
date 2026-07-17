use super::*;

#[tokio::test(flavor = "current_thread")]
async fn mid_effect_cancellation_emits_one_terminal_effect_and_cell_result() {
    let events = Arc::new(Events::default());
    let started = Arc::new(tokio::sync::Notify::new());
    let cancellation = Arc::new(TriggerCancellation::default());
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(BlockingEffect::new(Arc::clone(&started))));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("test.block"),
        host_event_sink: events.clone(),
        cancellation: Some(cancellation.clone()),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let cancel = async {
        started.notified().await;
        cancellation.cancel();
    };
    let (output, ()) = tokio::join!(
        session.eval("@test.block null", CellBudget::default()),
        cancel
    );
    let output = output.unwrap();
    assert!(output.error.unwrap().contains("cancelled"));

    let events = events.0.lock().unwrap();
    let effect_results = events
        .iter()
        .filter(|(kind, _)| kind == "effect_result")
        .collect::<Vec<_>>();
    assert_eq!(effect_results.len(), 1, "{events:?}");
    assert_eq!(effect_results[0].1["status"], "cancelled");
    let cell_results = events
        .iter()
        .filter(|(kind, _)| kind == "cell_result")
        .collect::<Vec<_>>();
    assert_eq!(cell_results.len(), 1, "{events:?}");
    assert_eq!(cell_results[0].1["status"], "cancelled");
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_effect_start_sink_still_emits_one_terminal_result() {
    let events = Arc::new(YieldingStartEvents::default());
    let cancellation = Arc::new(TriggerCancellation::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(Patch::new(Arc::clone(&calls))));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("fs.patch"),
        host_event_sink: events.clone(),
        cancellation: Some(cancellation.clone()),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let cancel = async {
        events.effect_start_seen.notified().await;
        cancellation.cancel();
    };
    let (output, ()) = tokio::join!(
        session.eval("@fs.patch {patch: \"x\"}", CellBudget::default()),
        cancel
    );
    let output = output.unwrap();
    assert!(output.error.unwrap().contains("cancelled"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let events = events.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "effect_result")
            .count(),
        1,
        "{events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_result")
            .count(),
        1,
        "{events:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_cell_start_sink_still_emits_one_paired_terminal() {
    let events = Arc::new(YieldingCellStartEvents::default());
    let cancellation = Arc::new(TriggerCancellation::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        cancellation: Some(cancellation.clone()),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let cancel = async {
        events.cell_start_seen.notified().await;
        cancellation.cancel();
    };
    let (output, ()) = tokio::join!(session.eval("1", CellBudget::default()), cancel);
    let output = output.unwrap();
    assert!(output.error.unwrap().contains("cancelled"));

    let events = events.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_start")
            .count(),
        1,
        "{events:?}"
    );
    let cell_results = events
        .iter()
        .filter(|(kind, _)| kind == "cell_result")
        .collect::<Vec<_>>();
    assert_eq!(cell_results.len(), 1, "{events:?}");
    assert_eq!(cell_results[0].1["status"], "cancelled");
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_binding_persistence_finishes_the_selected_commit() {
    let events = Arc::new(YieldingBindingEvents::default());
    let cancellation = Arc::new(TriggerCancellation::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        cancellation: Some(cancellation.clone()),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let cancel = async {
        events.binding_seen.notified().await;
        cancellation.cancel();
        events.release.notify_one();
    };
    let (output, ()) = tokio::join!(
        session.eval("let committed_during_cancel = 7", CellBudget::default()),
        cancel
    );
    let output = output.unwrap();
    assert!(output.error.is_none(), "{output:?}");
    assert_eq!(output.result, Some(json!(7)));

    cancellation.reset();
    let read = session
        .eval("committed_during_cancel", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(read.result, Some(json!(7)), "{read:?}");
    let events = events.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "binding_committed")
            .count(),
        2,
        "one structural binding event is emitted for each successful cell: {events:?}"
    );
    assert!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_result")
            .all(|(_, payload)| payload["status"] == "completed")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_after_durable_binding_event_keeps_the_installed_binding() {
    let events = Arc::new(DurableThenStallingBindingEvents::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    {
        let evaluation = session.eval("let installed_before_await = 7", CellBudget::default());
        tokio::pin!(evaluation);
        tokio::select! {
            _ = events.binding_seen.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                panic!("binding event was not durably recorded")
            }
            result = &mut evaluation => {
                panic!("binding persistence unexpectedly completed: {result:?}")
            }
        }
    }

    let read = session
        .eval("installed_before_await", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(read.result, Some(json!(7)), "{read:?}");
    assert!(
        events
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|(kind, payload)| { kind == "binding_committed" && payload["bindingCount"] == 1 })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn state_writing_batch_cancellation_cannot_split_event_from_base_commit() {
    let events = Arc::new(YieldingBindingEvents::default());
    let cancellation = Arc::new(TriggerCancellation::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        cancellation: Some(cancellation.clone()),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let cancel = async {
        events.binding_seen.notified().await;
        cancellation.cancel();
        events.release.notify_one();
    };
    let cells = vec!["let batch_commit_during_cancel = 9".into()];
    let (outputs, ()) = tokio::join!(session.eval_batch(&cells, CellBudget::default()), cancel);
    let outputs = outputs.unwrap();
    assert!(outputs[0].error.is_none(), "{outputs:?}");

    cancellation.reset();
    let read = session
        .eval("batch_commit_during_cancel", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(read.result, Some(json!(9)), "{read:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn mid_effect_timeout_emits_one_terminal_effect_and_cell_result() {
    let events = Arc::new(Events::default());
    let started = Arc::new(tokio::sync::Notify::new());
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(BlockingEffect::new(started)));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("test.block"),
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let output = session
        .eval(
            "@test.block null",
            CellBudget {
                wall_ms: 10,
                output_bytes: 1024,
            },
        )
        .await
        .unwrap();
    assert!(output.error.unwrap().contains("wall-clock budget"));

    let events = events.0.lock().unwrap();
    let effect_results = events
        .iter()
        .filter(|(kind, _)| kind == "effect_result")
        .collect::<Vec<_>>();
    assert_eq!(effect_results.len(), 1, "{events:?}");
    assert_eq!(effect_results[0].1["status"], "timed_out");
    let cell_results = events
        .iter()
        .filter(|(kind, _)| kind == "cell_result")
        .collect::<Vec<_>>();
    assert_eq!(cell_results.len(), 1, "{events:?}");
    assert_eq!(cell_results[0].1["status"], "timed_out");
}

#[tokio::test(flavor = "current_thread")]
async fn host_spill_envelopes_must_fit_the_cell_budget() {
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(Spill::new()));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("test.spill"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let output = session
        .eval(
            "@test.spill null",
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 64,
            },
        )
        .await
        .unwrap();

    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("output budget")),
        "{output:?}"
    );

    let compact = session
        .eval(
            "@test.spill null",
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 2_048,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        compact.result.as_ref().unwrap()["artifact"]["uri"],
        json!("artifact://7"),
        "{compact:?}"
    );
}

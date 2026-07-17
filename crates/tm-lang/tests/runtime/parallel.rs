use super::*;

#[tokio::test(flavor = "current_thread")]
async fn par_map_owns_effect_children_and_reports_bounded_progress() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = session
        .eval(
            "[{patch: \"a\"}, {patch: \"b\"}] |> par map @fs.patch",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(output.error.is_none(), "{output:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let events = events.0.lock().unwrap();
    let kinds: Vec<_> = events.iter().map(|(kind, _)| kind.as_str()).collect();
    let scope_start = kinds
        .iter()
        .position(|kind| *kind == "scope_start")
        .unwrap();
    let first_effect = kinds
        .iter()
        .position(|kind| *kind == "effect_start")
        .unwrap();
    let scope_result = kinds
        .iter()
        .rposition(|kind| *kind == "scope_result")
        .unwrap();
    assert!(scope_start < first_effect && first_effect < scope_result);
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == "scope_progress")
            .count(),
        2
    );
    let scope_id = events
        .iter()
        .find(|(kind, _)| kind == "scope_start")
        .unwrap()
        .1["nodeId"]
        .clone();
    assert!(
        events
            .iter()
            .filter(|(kind, _)| kind == "effect_start")
            .all(|(_, payload)| payload["parentNodeId"] == scope_id)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn par_record_and_map_overlap_effects_and_preserve_result_order() {
    for source in [
        "par {first: @test.probe {value: 1}, second: @test.probe {value: 2}}",
        "[{value: 1}, {value: 2}] |> par map @test.probe",
    ] {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut registry = HostRegistry::new();
        registry.register(Arc::new(ParallelProbe::new(
            Arc::clone(&active),
            Arc::clone(&peak),
        )));
        let mut session = TmSandbox::new(TmSandboxOptions {
            host_registry: registry,
            grants: CapabilityGrants::default().allow("test.probe"),
            ..TmSandboxOptions::default()
        })
        .open(SessionConfig::default())
        .await
        .unwrap();

        let output = session.eval(source, CellBudget::default()).await.unwrap();
        assert!(output.error.is_none(), "{source}: {output:?}");
        assert_eq!(peak.load(Ordering::SeqCst), 2, "{source}");
        if source.starts_with('[') {
            assert_eq!(
                output.result,
                Some(json!([{"value": 1}, {"value": 2}])),
                "{source}"
            );
        } else {
            assert_eq!(
                output.result,
                Some(json!({"first": {"value": 1}, "second": {"value": 2}})),
                "{source}"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn par_map_first_failure_cancels_unstarted_siblings() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = session
        .eval(
            "[{patch: \"a\"}, {patch: \"fail\"}, {patch: \"never\"}] |> par map @fs.patch",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(output.error.unwrap().contains("scripted failure"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let events = events.0.lock().unwrap();
    let terminal = events
        .iter()
        .find(|(kind, payload)| kind == "scope_result" && payload["status"] == "failed")
        .unwrap();
    assert_eq!(terminal.1["cancelledSiblings"], 1);
}

#[tokio::test(flavor = "current_thread")]
async fn outer_parallel_failure_terminalizes_nested_effects_and_scopes_child_first() {
    let events = Arc::new(Events::default());
    let started = Arc::new(tokio::sync::Notify::new());
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(BlockingEffect::new(Arc::clone(&started))));
    registry.register(Arc::new(FailAfterStart::new(started)));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default()
            .allow("test.block")
            .allow("test.fail_after_start"),
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let output = session
        .eval(
            "par {nested: par {blocked: @test.block null}, failing: @test.fail_after_start null}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(output.error.unwrap().contains("nested sibling failed"));

    let events = events.0.lock().unwrap();
    let scopes = events
        .iter()
        .filter(|(kind, _)| kind == "scope_start")
        .collect::<Vec<_>>();
    assert_eq!(scopes.len(), 2, "{events:?}");
    let outer = scopes[0].1["nodeId"].clone();
    let nested = scopes[1].1["nodeId"].clone();
    assert_eq!(scopes[1].1["parentNodeId"], outer);

    let block_start = events
        .iter()
        .find(|(kind, payload)| kind == "effect_start" && payload["capability"] == "test.block")
        .expect("blocking effect starts inside nested scope");
    assert_eq!(block_start.1["parentNodeId"], nested);
    let block_terminal = events
        .iter()
        .find(|(kind, payload)| {
            kind == "effect_result" && payload["nodeId"] == block_start.1["nodeId"]
        })
        .expect("dropped nested effect receives a terminal result");
    assert_eq!(block_terminal.1["status"], "cancelled");
    assert_eq!(block_terminal.1["parentNodeId"], nested);

    let nested_result = events
        .iter()
        .position(|(kind, payload)| {
            kind == "scope_result"
                && payload["nodeId"] == nested
                && payload["status"] == "cancelled"
        })
        .expect("nested scope receives a cancelled terminal");
    let outer_result = events
        .iter()
        .position(|(kind, payload)| {
            kind == "scope_result" && payload["nodeId"] == outer && payload["status"] == "failed"
        })
        .expect("outer scope receives a failed terminal");
    assert!(nested_result < outer_result, "{events:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn limits_and_structured_scope_fail_closed() {
    let events = Arc::new(Events::default());
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(Patch::new(Arc::new(AtomicUsize::new(0)))));
    let mut options = TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default(),
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    };
    options.limits = RuntimeLimits {
        steps: 50,
        print_bytes: 8,
        preview_bytes: 16,
        ..RuntimeLimits::default()
    };
    let mut session = TmSandbox::new(options)
        .open(SessionConfig::default())
        .await
        .unwrap();
    assert!(
        session
            .eval("print \"0123456789\"", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("budget")
    );
    let output = session
        .eval(
            "par [1, 2]",
            CellBudget {
                wall_ms: 1000,
                output_bytes: 100,
            },
        )
        .await
        .unwrap();
    assert_eq!(output.result, Some(json!([1, 2])));
    let kinds: Vec<_> = events
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|(kind, _)| kind.clone())
        .collect();
    assert!(kinds.contains(&"scope_start".into()));
    assert!(kinds.contains(&"scope_result".into()));
}

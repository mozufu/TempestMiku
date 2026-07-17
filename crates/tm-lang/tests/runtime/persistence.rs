use super::*;

#[tokio::test(flavor = "current_thread")]
async fn failed_function_pattern_restores_the_callers_environment_before_handle() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            steps: 200,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let pattern = (0..128)
        .map(|index| format!("item_{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let values = serde_json::to_string(&"item\n".repeat(128)).unwrap();
    let source = format!(
        "fun fail [{pattern}] = 0; let caller = 41; let values = {values} |> lines; handle fail values with error {{ | ResourceLimitError _ -> caller }}"
    );

    let output = session.eval(&source, CellBudget::default()).await.unwrap();

    assert_eq!(output.result, Some(json!(41)), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_event_persistence_failures_are_fatal_sandbox_errors() {
    for (event_type, source) in [
        ("cell_result", "1"),
        ("cell_start", "("),
        ("binding_committed", "let persisted = 1"),
    ] {
        let mut session = TmSandbox::new(TmSandboxOptions {
            host_event_sink: Arc::new(FailOnceOnEvent::new(event_type)),
            ..TmSandboxOptions::default()
        })
        .open(SessionConfig::default())
        .await
        .unwrap();

        let error = session
            .eval(source, CellBudget::default())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("persistence failure"),
            "{event_type}: {error:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn deep_pure_recursion_returns_a_resource_limit_error() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            runtime_depth: 32,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let output = session
        .eval(
            "fun recurse value = recurse value; recurse 0",
            CellBudget::default(),
        )
        .await
        .unwrap();
    let error = output.error.as_deref().unwrap_or_default();

    assert!(error.contains("ResourceLimitError"), "{output:?}");
    assert!(error.contains("runtime nesting budget"), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn parse_and_check_failures_emit_redacted_paired_cell_events() {
    let events = Arc::new(Events::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    for source in ["let = \"parse-secret\"", "unknown_check_secret"] {
        let output = session.eval(source, CellBudget::default()).await.unwrap();
        assert!(output.error.is_some(), "{source}: {output:?}");
    }
    let events = events.0.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_start")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_result")
            .count(),
        2
    );
    let encoded = serde_json::to_string(&*events).unwrap();
    assert!(!encoded.contains("parse-secret"), "{encoded}");
    assert!(!encoded.contains("unknown_check_secret"), "{encoded}");
}

#[tokio::test(flavor = "current_thread")]
async fn collection_builders_enforce_value_budget_incrementally() {
    let repeated = "x".repeat(50);
    let strings = ["a".repeat(30), "b".repeat(30), "c".repeat(30)];
    let controls = "\\u0001".repeat(40);
    let sources = [
        format!("let key = \"{repeated}\"; [1, 2, 3] |> map (fun value -> key)"),
        format!(
            "[\"{}\", \"{}\", \"{}\"] |> sort_by (fun value -> value) asc",
            strings[0], strings[1], strings[2]
        ),
        format!(
            "[\"{}\", \"{}\", \"{}\"] |> group_by (fun value -> value)",
            strings[0], strings[1], strings[2]
        ),
        format!("[\"{controls}\"] |> group_by (fun value -> value)"),
    ];
    for source in sources {
        let mut session = TmSandbox::new(TmSandboxOptions {
            limits: RuntimeLimits {
                value_bytes: 128,
                ..RuntimeLimits::default()
            },
            ..TmSandboxOptions::default()
        })
        .open(SessionConfig::default())
        .await
        .unwrap();
        let output = session.eval(&source, CellBudget::default()).await.unwrap();
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains("intermediate value budget")),
            "{source}: {output:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fabricated_artifact_envelopes_do_not_bypass_result_budget() {
    let fake = format!(
        "{{truncated: true, artifact: {{uri: \"artifact://7\", id: \"7\", kind: \"text\", mime: \"text/plain\", title: null, size_bytes: 256, preview: \"x\"}}, payload: \"{}\"}}",
        "x".repeat(256)
    );
    let mut session = TmSandbox::new(TmSandboxOptions::default())
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = session
        .eval(
            &fake,
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 64,
            },
        )
        .await
        .unwrap();
    assert!(output.error.unwrap().contains("result/output budget"));
}

#[tokio::test(flavor = "current_thread")]
async fn effect_previews_share_the_cell_output_budget() {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(ParallelProbe::new(active, peak)));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("test.probe"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let output = session
        .eval(
            "@test.probe {value: 1}; @test.probe {value: 2}; @test.probe {value: 3}",
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 50,
            },
        )
        .await
        .unwrap();
    assert!(output.error.unwrap().contains("effect/output budget"));
}

#[tokio::test(flavor = "current_thread")]
async fn sensitive_cells_redact_source_arguments_results_and_errors() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let approval_actions = Arc::new(Mutex::new(Vec::new()));
    let mut session = sandbox_with_policy(
        Arc::clone(&events),
        calls,
        Arc::new(RecordingApprove(Arc::clone(&approval_actions))),
    )
    .open(SessionConfig::default())
    .await
    .unwrap();
    let output = session
        .eval(
            "@fs.patch {patch: \"secret-source-value\"} |> display {kind: \"json\"}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(output.error.is_none(), "{output:?}");
    assert_eq!(&*approval_actions.lock().unwrap(), &["fs.patch"]);

    let events = events.0.lock().unwrap();
    let encoded = serde_json::to_string(&*events).unwrap();
    assert!(!encoded.contains("secret-source-value"), "{encoded}");
    assert!(events.iter().any(|(kind, payload)| {
        kind == "cell_start" && payload["sourcePreview"] == "[redacted]"
    }));
    assert!(events.iter().any(|(kind, payload)| {
        kind == "effect_result" && payload["resultPreview"] == "[redacted]"
    }));
    assert!(events.iter().any(|(kind, payload)| {
        kind == "effect_suspended" && payload["action"] == "[redacted]"
    }));
    assert!(events.iter().any(|(kind, payload)| {
        kind == "display" && payload["spec"] == "[redacted]" && payload["value"] == "[redacted]"
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn durable_previews_stay_content_blind_across_persistent_bindings() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), calls)
        .open(SessionConfig::default())
        .await
        .unwrap();

    let authority = session
        .eval(
            "let persisted = @fs.patch {patch: \"cross-cell-secret\"}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(authority.error.is_none(), "{authority:?}");
    let pure = session
        .eval(
            "persisted |> display {kind: \"json\"}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(pure.error.is_none(), "{pure:?}");

    let events = events.0.lock().unwrap();
    let encoded = serde_json::to_string(&*events).unwrap();
    assert!(!encoded.contains("cross-cell-secret"), "{encoded}");
    assert!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_start")
            .all(|(_, payload)| payload["sourcePreview"] == "[redacted]")
    );
    assert!(events.iter().any(|(kind, payload)| {
        kind == "display" && payload["spec"] == "[redacted]" && payload["value"] == "[redacted]"
    }));
    let binding = events
        .iter()
        .find(|(kind, _)| kind == "binding_committed")
        .expect("authority cell commits its binding");
    assert_eq!(binding.1["bindingCount"], 1);
    assert_eq!(binding.1["namesRedacted"], true);
    assert!(binding.1.get("names").is_none());
    assert!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_result")
            .all(|(_, payload)| {
                payload
                    .get("resultPreview")
                    .is_none_or(|value| value == "[redacted]")
            })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn division_by_zero_and_rethrow_preserve_error_identity() {
    let mut session = TmSandbox::new(TmSandboxOptions::default())
        .open(SessionConfig::default())
        .await
        .unwrap();
    let direct = session
        .eval(
            "handle 1 / 0 with error { | DivisionByZero _ -> 41 }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(direct.result, Some(json!(41)), "{direct:?}");

    let rethrown = session
        .eval(
            "handle do { handle 1 / 0 with error { | DivisionByZero _ -> rethrow null } } with error { | DivisionByZero _ -> 42 }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(rethrown.result, Some(json!(42)), "{rethrown:?}");
}

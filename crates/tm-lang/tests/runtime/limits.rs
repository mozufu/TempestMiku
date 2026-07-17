use super::*;

#[tokio::test(flavor = "current_thread")]
async fn cancellation_and_result_caps_do_not_commit_bindings() {
    let events = Arc::new(Events::default());
    let mut options = TmSandboxOptions {
        grants: CapabilityGrants::default(),
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    };
    options.cancellation = Some(Arc::new(Cancelled));
    let mut cancelled = TmSandbox::new(options.clone())
        .open(SessionConfig::default())
        .await
        .unwrap();
    assert!(
        cancelled
            .eval("let x = 1", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("cancelled")
    );
    let cancelled_batch = cancelled
        .eval_batch(
            &["@artifacts.put {data: \"cancelled-batch-secret\"}".into()],
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(
        cancelled_batch[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cancelled"))
    );

    options.cancellation = None;
    let mut capped = TmSandbox::new(options)
        .open(SessionConfig::default())
        .await
        .unwrap();
    let timed_out = capped
        .eval(
            "@artifacts.put {data: \"preflight-secret\"}",
            CellBudget {
                wall_ms: 0,
                output_bytes: 64,
            },
        )
        .await
        .unwrap();
    assert!(timed_out.error.unwrap().contains("TimeoutError"));
    let timed_out_batch = capped
        .eval_batch(
            &["@artifacts.put {data: \"timed-out-batch-secret\"}".into()],
            CellBudget {
                wall_ms: 0,
                output_bytes: 64,
            },
        )
        .await
        .unwrap();
    assert!(
        timed_out_batch[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("TimeoutError"))
    );
    {
        let events = events.0.lock().unwrap();
        let encoded = serde_json::to_string(&*events).unwrap();
        assert!(!encoded.contains("preflight-secret"), "{encoded}");
        assert!(!encoded.contains("cancelled-batch-secret"), "{encoded}");
        assert!(!encoded.contains("timed-out-batch-secret"), "{encoded}");
        assert_eq!(
            events
                .iter()
                .filter(|(kind, payload)| {
                    kind == "cell_result"
                        && matches!(payload["status"].as_str(), Some("cancelled" | "timed_out"))
                })
                .count(),
            4,
            "{events:#?}"
        );
        assert!(
            events
                .iter()
                .filter(|(kind, _)| kind == "cell_start")
                .all(|(_, payload)| payload["sourcePreview"] == "[redacted]")
        );
    }
    let failed = capped
        .eval(
            "let huge = [\"0123456789\"]",
            CellBudget {
                wall_ms: 1000,
                output_bytes: 4,
            },
        )
        .await
        .unwrap();
    let failed_error = failed.error.unwrap();
    assert!(failed_error.len() <= 4, "{failed_error}");
    assert!(failed_error.starts_with("Reso"), "{failed_error}");
    assert!(
        capped
            .eval("huge", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("unbound name huge")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn model_facing_runtime_errors_respect_the_cell_output_budget() {
    let mut session = TmSandbox::new(TmSandboxOptions::default())
        .open(SessionConfig::default())
        .await
        .unwrap();
    let source = format!("unknown_{}", "x".repeat(4_096));
    let budget = CellBudget {
        wall_ms: 1_000,
        output_bytes: 32,
    };

    let single = session.eval(&source, budget).await.unwrap();
    let single_error = single.error.unwrap();
    assert!(single_error.len() <= budget.output_bytes, "{single_error}");

    let batch = session.eval_batch(&[source], budget).await.unwrap();
    let batch_error = batch[0].error.as_deref().unwrap();
    assert!(batch_error.len() <= budget.output_bytes, "{batch_error}");
}

#[tokio::test(flavor = "current_thread")]
async fn source_parse_value_and_cumulative_output_budgets_fail_closed() {
    let options = TmSandboxOptions {
        limits: RuntimeLimits {
            source_bytes: 1_024,
            syntax_nodes: 64,
            parse_depth: 16,
            value_bytes: 256,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    };
    let mut session = TmSandbox::new(options)
        .open(SessionConfig::default())
        .await
        .unwrap();

    let oversized_source = format!("\"{}\"", "x".repeat(1_025));
    let source_error = session
        .eval(&oversized_source, CellBudget::default())
        .await
        .unwrap();
    assert!(source_error.error.unwrap().contains("source budget"));

    let token_heavy = std::iter::repeat_n("x", 100).collect::<Vec<_>>().join(" ");
    let token_error = session
        .eval(&token_heavy, CellBudget::default())
        .await
        .unwrap();
    assert!(token_error.error.unwrap().contains("syntax budget"));

    let nested = format!("{}1{}", "(".repeat(24), ")".repeat(24));
    let depth_error = session.eval(&nested, CellBudget::default()).await.unwrap();
    assert!(depth_error.error.unwrap().contains("nesting budget"));

    let interpolated = format!("\"#{{{}1{}}}\"", "(".repeat(24), ")".repeat(24));
    let interpolation_error = session
        .eval(&interpolated, CellBudget::default())
        .await
        .unwrap();
    assert!(
        interpolation_error
            .error
            .unwrap()
            .contains("nesting budget")
    );

    let growth = session
        .eval(
            "let x = \"12345678\"; let x = x + x; let x = x + x; let x = x + x; let x = x + x; let x = x + x; let x = x + x",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(growth.error.unwrap().contains("intermediate value budget"));

    let cumulative = session
        .eval(
            "print \"123456\"; \"abcdef\"",
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 10,
            },
        )
        .await
        .unwrap();
    let cumulative_error = cumulative.error.as_deref().unwrap();
    assert!(cumulative_error.len() <= 10, "{cumulative_error}");
    assert!(
        cumulative_error.starts_with("Resource"),
        "{cumulative_error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn retained_environment_and_high_cardinality_values_are_bounded_atomically() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            // Keep one candidate value below the per-value ceiling so this specifically exercises
            // the lower aggregate retained-environment ceiling.
            value_bytes: 256 * 1024,
            environment_bytes: 32 * 1024,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let kept = session
        .eval("let kept = 7", CellBudget::default())
        .await
        .unwrap();
    assert!(kept.error.is_none(), "{kept:?}");
    let oversized = format!(
        "let rejected = {}",
        serde_json::to_string(&"x".repeat(40_000)).unwrap()
    );
    let rejected = session
        .eval(&oversized, CellBudget::default())
        .await
        .unwrap();
    assert!(
        rejected
            .error
            .as_deref()
            .is_some_and(|error| error.contains("environment budget")),
        "{rejected:?}"
    );
    let prior = session.eval("kept", CellBudget::default()).await.unwrap();
    assert_eq!(prior.result, Some(json!(7)), "{prior:?}");
    let absent = session
        .eval("rejected", CellBudget::default())
        .await
        .unwrap();
    assert!(absent.error.unwrap().contains("unbound name rejected"));

    let mut value_session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            value_bytes: 8 * 1024,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let high_cardinality_source = serde_json::to_string(&"\n".repeat(256)).unwrap();
    let high_cardinality = value_session
        .eval(
            &format!("{high_cardinality_source} |> lines"),
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(
        high_cardinality
            .error
            .unwrap()
            .contains("intermediate value budget")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cumulative_closure_capture_budget_rejects_quadratic_environment_growth() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            environment_bytes: 32 * 1024,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let source = (0..128)
        .map(|index| format!("fun f{index} x = x"))
        .collect::<Vec<_>>()
        .join("; ");
    let output = session.eval(&source, CellBudget::default()).await.unwrap();
    assert!(output.error.unwrap().contains("environment budget"));
}

#[tokio::test(flavor = "current_thread")]
async fn repeated_large_lambda_bodies_count_toward_value_budget() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            value_bytes: 256 * 1024,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let body = serde_json::to_string(&"x".repeat(16 * 1024)).unwrap();
    let inputs = (0..24)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!("let make = fun _ -> fun _ -> {body}; [{inputs}] |> map make");

    let output = session.eval(&source, CellBudget::default()).await.unwrap();

    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("intermediate value budget")),
        "{output:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn many_short_bindings_are_checked_once_at_commit_within_the_wall_budget() {
    let mut options = TmSandboxOptions::default();
    options.limits.source_bytes = 512 * 1024;
    let mut session = TmSandbox::new(options)
        .open(SessionConfig::default())
        .await
        .unwrap();
    let mut source = String::new();
    for index in 0..6_000 {
        source.push_str(&format!("let value_{index} = {index};"));
    }
    source.push_str("value_5999");

    let output = session
        .eval(
            &source,
            CellBudget {
                wall_ms: 3_000,
                output_bytes: CellBudget::default().output_bytes,
            },
        )
        .await
        .unwrap();

    assert_eq!(output.result, Some(json!(5_999)), "{output:?}");
}

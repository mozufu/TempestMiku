use super::*;

#[tokio::test(flavor = "current_thread")]
async fn execute_batch_serializes_state_writes_and_merges_bindings_in_response_order() {
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

    let outputs = session
        .eval_batch(
            &[
                "let shared = @test.probe {value: 1}".into(),
                "let second = @test.probe {value: 2};\nlet shared = 2".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(
        outputs.iter().all(|output| output.error.is_none()),
        "{outputs:?}"
    );
    assert_eq!(peak.load(Ordering::SeqCst), 1);
    assert_eq!(
        session
            .eval("shared", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(2))
    );
    assert_eq!(
        session
            .eval("second", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!({"value": 2}))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn execute_batch_honors_configured_parallelism() {
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
        limits: RuntimeLimits {
            parallelism: 1,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "@test.probe {value: 1}".into(),
                "@test.probe {value: 2}".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(outputs.iter().all(|output| output.error.is_none()));
    assert_eq!(peak.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn execute_batch_schedules_forward_declarative_dependencies_in_response_order() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "let first = 1".into(),
                "let second = first + 1".into(),
                "second + 1".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        outputs
            .iter()
            .map(|output| output.result.clone())
            .collect::<Vec<_>>(),
        vec![Some(json!(1)), Some(json!(2)), Some(json!(3))]
    );
    assert!(outputs.iter().all(|output| output.error.is_none()));
    assert_eq!(
        session
            .eval("second", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(2))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn execute_batch_reads_the_latest_earlier_rebinding_and_merges_by_response_order() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    session
        .eval("let shared = 0", CellBudget::default())
        .await
        .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "let shared = 1".into(),
                "let derived = shared + 1".into(),
                "let shared = 3".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert_eq!(outputs[1].result, Some(json!(2)));
    assert_eq!(
        session
            .eval("{shared, derived}", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!({"shared": 3, "derived": 2}))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn execute_batch_schedules_closure_and_interpolation_dependencies() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "let seed = 4".into(),
                "fun add_seed value = value + seed".into(),
                "let rendered = \"value #{add_seed 2}\"".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(outputs.iter().all(|output| output.error.is_none()));
    assert_eq!(outputs[2].result, Some(json!("value 6")));
    assert_eq!(
        session
            .eval("rendered", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!("value 6"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn interpolation_rejects_multiple_forms_before_running_effects() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(events, Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();

    let output = session
        .eval(
            r##""#{1; @fs.patch {patch: \"skipped\"}}""##,
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("interpolation must contain one expression")),
        "{output:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn authorized_linked_alias_uri_is_normalized_to_the_host_argument_schema() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hello.txt"), "hello\n").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".into(),
        path: root.path().to_path_buf(),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let mut session = TmSandbox::new(TmSandboxOptions {
        artifact_root: artifacts.path().to_path_buf(),
        session_id: "direct-fs-read".into(),
        project_id: Some("repo".into()),
        linked_folders: Some(linked),
        grants: CapabilityGrants::default().allow("fs.read"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let output = session
        .eval(
            "let path = repo:hello.txt; @fs.read path",
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(output.error.is_none(), "{output:?}");
    assert_eq!(output.result.as_ref().unwrap()["content"], "hello\n");
}

#[tokio::test(flavor = "current_thread")]
async fn linked_alias_uri_outside_the_authoritative_scope_is_rejected_before_host_call() {
    let root = tempfile::tempdir().unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".into(),
        path: root.path().to_path_buf(),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let mut session = TmSandbox::new(TmSandboxOptions {
        session_id: "scoped-linked-alias".into(),
        project_id: Some("other".into()),
        linked_folders: Some(linked),
        grants: CapabilityGrants::default().allow("fs.read"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let output = session
        .eval("@fs.read repo:hello.txt", CellBudget::default())
        .await
        .unwrap();

    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unknown resource scheme repo")),
        "{output:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_defaults_do_not_grant_http_or_artifact_reads() {
    let artifacts = tempfile::tempdir().unwrap();
    let mut http_allowlist = std::collections::BTreeMap::new();
    http_allowlist.insert("https://example.test/data".into(), "fixture".into());
    let mut denied = TmSandbox::new(TmSandboxOptions {
        artifact_root: artifacts.path().to_path_buf(),
        session_id: "exact-default-grants".into(),
        http_allowlist: http_allowlist.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    for source in [
        "@http.request {method: \"GET\", url: \"https://example.test/data\"}",
        "@resources.read artifact://0",
        "@artifacts.get artifact://0",
        "@artifacts.list null",
    ] {
        let output = denied.eval(source, CellBudget::default()).await.unwrap();
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains("unknown capability")
                    || error.contains("unknown resource scheme")),
            "{source}: {output:?}"
        );
    }

    let mut allowed = TmSandbox::new(TmSandboxOptions {
        artifact_root: artifacts.path().to_path_buf(),
        session_id: "exact-explicit-grants".into(),
        http_allowlist,
        grants: CapabilityGrants::default().allow("http.request"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let output = allowed
        .eval(
            "@http.request {method: \"GET\", url: \"https://example.test/data\"}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(output.result, Some(json!("fixture")), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn failed_batch_producer_blocks_dependent_effect_without_falling_back() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();
    session
        .eval("let payload = 7", CellBudget::default())
        .await
        .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "let payload = 1 / 0".into(),
                "@fs.patch {patch: payload}".into(),
                "let unrelated = 9".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(outputs[0].error.is_some());
    assert_eq!(
        outputs[1].error.as_deref(),
        Some(
            "BatchDependencyError: execute call 2 requires binding(s) [payload] from failed execute call 1"
        )
    );
    assert_eq!(outputs[2].result, Some(json!(9)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        session
            .eval("{payload, unrelated}", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!({"payload": 7, "unrelated": 9}))
    );

    let events = events.0.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(event, _)| event == "cell_start")
            .count(),
        5
    );
    assert_eq!(
        events
            .iter()
            .filter(|(event, payload)| { event == "cell_result" && payload["status"] == "failed" })
            .count(),
        2
    );
    assert!(events.iter().all(|(event, _)| event != "effect_start"));
}

#[tokio::test(flavor = "current_thread")]
async fn stalled_dependency_terminal_persistence_is_bounded() {
    let events = Arc::new(StallingSecondCellResult::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let cells = vec!["let payload = 1 / 0".into(), "payload".into()];

    let error = tokio::time::timeout(
        Duration::from_secs(3),
        session.eval_batch(&cells, CellBudget::default()),
    )
    .await
    .expect("dependency terminal persistence must respect its grace period")
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("terminal event persistence deadline exceeded"),
        "{error:?}"
    );
    assert_eq!(events.seen.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn unparseable_batch_cell_blocks_every_later_cell_fail_closed() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();
    session
        .eval("let payload = 7", CellBudget::default())
        .await
        .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "let payload =".into(),
                "@fs.patch {patch: payload}".into(),
                "let unrelated = 9".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(outputs[0].error.is_some());
    for output in &outputs[1..] {
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains("BatchDependencyError")
                    && error.contains("<unknown bindings>")),
            "{output:?}"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        session
            .eval("payload", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(7))
    );
    assert!(
        session
            .eval("unrelated", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("unbound name unrelated")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn execute_batch_does_not_resolve_backward_declarative_dependencies() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let outputs = session
        .eval_batch(
            &["later".into(), "let later = 1".into()],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(
        outputs[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unbound name later"))
    );
    assert_eq!(outputs[1].result, Some(json!(1)));
}

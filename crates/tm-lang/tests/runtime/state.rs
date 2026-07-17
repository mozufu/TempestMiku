use super::*;

#[tokio::test(flavor = "current_thread")]
async fn persistent_bindings_commit_atomically_and_reset() {
    let events = Arc::new(Events::default());
    let mut session = sandbox(events, Arc::new(AtomicUsize::new(0)))
        .open(SessionConfig::default())
        .await
        .unwrap();
    let first = session
        .eval("let x = 1", CellBudget::default())
        .await
        .unwrap();
    assert!(first.error.is_none(), "{first:?}");
    assert_eq!(first.result, Some(json!(1)));
    let failed = session
        .eval("let y = 2;\n1 / 0", CellBudget::default())
        .await
        .unwrap();
    assert!(failed.error.unwrap().contains("division by zero"));
    assert_eq!(
        session
            .eval("x", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(1))
    );
    assert!(
        session
            .eval("y", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("unbound name y")
    );
    session.reset().await.unwrap();
    assert!(
        session
            .eval("x", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("unbound name x")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn approval_resumes_same_effect_node_and_executes_once() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = session
        .eval("@fs.patch {patch: \"secret\"}", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(output.result.unwrap()["applied"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let events = events.0.lock().unwrap();
    let ordered: Vec<_> = events.iter().map(|(kind, _)| kind.as_str()).collect();
    let start = ordered
        .iter()
        .position(|kind| *kind == "effect_start")
        .unwrap();
    let start_payload = events
        .iter()
        .find(|(kind, _)| kind == "effect_start")
        .map(|(_, payload)| payload)
        .unwrap();
    assert_eq!(start_payload["argsPreview"], "[redacted]");
    let suspended = ordered
        .iter()
        .position(|kind| *kind == "effect_suspended")
        .unwrap();
    let resumed = ordered
        .iter()
        .position(|kind| *kind == "effect_resumed")
        .unwrap();
    let result = ordered
        .iter()
        .position(|kind| *kind == "effect_result")
        .unwrap();
    assert!(start < suspended && suspended < resumed && resumed < result);
    let ids: Vec<_> = events
        .iter()
        .filter(|(kind, _)| {
            [
                "effect_start",
                "effect_suspended",
                "effect_resumed",
                "effect_result",
            ]
            .contains(&kind.as_str())
        })
        .map(|(_, payload)| payload["nodeId"].as_str().unwrap())
        .collect();
    assert!(ids.windows(2).all(|pair| pair[0] == pair[1]));
}

#[tokio::test(flavor = "current_thread")]
async fn denial_and_timeout_are_terminal_and_never_execute_effect() {
    for policy in [
        Arc::new(Deny) as Arc<dyn ApprovalPolicy>,
        Arc::new(tm_host::DefaultDenyApprovalPolicy) as Arc<dyn ApprovalPolicy>,
    ] {
        let events = Arc::new(Events::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut session = sandbox_with_policy(Arc::clone(&events), Arc::clone(&calls), policy)
            .open(SessionConfig::default())
            .await
            .unwrap();
        let output = session
            .eval("@fs.patch {patch: \"x\"}", CellBudget::default())
            .await
            .unwrap();
        assert!(output.error.is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let events = events.0.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|(kind, payload)| kind == "effect_result" && payload["status"] == "failed")
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn approval_backend_errors_terminalize_suspended_effects_once() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox_with_policy(
        Arc::clone(&events),
        Arc::clone(&calls),
        Arc::new(ApprovalFailure),
    )
    .open(SessionConfig::default())
    .await
    .unwrap();
    let output = session
        .eval("@fs.patch {patch: \"x\"}", CellBudget::default())
        .await
        .unwrap();
    assert!(output.error.is_some(), "{output:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let events = events.0.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "effect_result")
            .count(),
        1,
        "{events:?}"
    );
}

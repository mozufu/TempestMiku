use super::support::*;
use super::*;

#[tokio::test]
async fn proc_run_requires_approval_even_for_configured_safe_args_and_spills() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"p0-proc-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("src/lib.rs"),
        "#[test]\nfn prints() { println!(\"sk-testsecret123456 {}\", \"x\".repeat(60000)); }\n",
    )
    .unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(artifact_dir.path(), "proc").unwrap();
    let read_store = store.clone();
    let proc_run = ProcRunFn::with_timeout_ms(temp_linked(root.path(), FsMode::Rw), store, 180_000);
    let denied_safe = proc_run
        .call(
            json!({"cmd":"cargo","args":["test"],"cwd":"tempestmiku:"}),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(denied_safe, HostError::ApprovalTimeout(_)));
    let value = call_fn(
        &proc_run,
        json!({"cmd":"cargo","args":["test"],"cwd":"tempestmiku:"}),
        &approved_ctx(),
    )
    .await;
    assert_eq!(value["exitCode"], json!(0));
    let denied = proc_run
        .call(
            json!({"cmd":"cargo","args":["clean"],"cwd":"tempestmiku:"}),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(denied, HostError::ApprovalTimeout(_)));
    let hostile_suffix = proc_run
        .call(
            json!({"cmd":"cargo","args":["test","--manifest-path","/tmp/outside/Cargo.toml"],"cwd":"tempestmiku:"}),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(hostile_suffix, HostError::ApprovalTimeout(_)));
    let unknown = proc_run
        .call(
            json!({"cmd":"rm","args":["-rf","."],"cwd":"tempestmiku:"}),
            &approved_ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(unknown, HostError::CapabilityDenied(_)));
    let spill = call_fn(
            &proc_run,
            json!({"cmd":"cargo","args":["test","--","--nocapture"],"cwd":"tempestmiku:","outputBytes":1000}),
            &approved_ctx(),
        )
        .await;
    assert_eq!(spill["truncated"], json!(true));
    assert!(
        spill["artifact"]["uri"]
            .as_str()
            .unwrap()
            .starts_with("artifact://")
    );
    let persisted = read_store
        .read(spill["artifact"]["uri"].as_str().unwrap(), None)
        .unwrap();
    assert!(!persisted.content.contains("sk-testsecret123456"));
    assert!(persisted.content.contains("[REDACTED_TOKEN]"));
}

#[tokio::test]
async fn proc_run_approval_is_bounded_redacted_and_identifies_exact_execution() {
    let root = tempfile::tempdir().unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let capture = Arc::new(CaptureThenApprove::default());
    let invocation =
        InvocationCtx::with_approvals(ctx().grants, capture.clone(), Duration::from_secs(1));
    let proc_run = ProcRunFn::with_timeout_ms(
        temp_linked_with_commands(
            root.path(),
            FsMode::Rw,
            vec!["echo".to_string()],
            Vec::new(),
        ),
        ArtifactStore::open(artifact_dir.path(), "proc-approval-action").unwrap(),
        180_000,
    );
    let secret = "sk-testsecret123456";

    let result = proc_run
        .call(
            json!({
                "cmd":"echo",
                "args":[secret, "x".repeat(400)],
                "cwd":"tempestmiku:"
            }),
            &invocation,
        )
        .await
        .unwrap();
    assert_eq!(result["exitCode"], json!(0));
    let actions = capture.actions.lock().unwrap();
    let action = actions.last().unwrap();
    assert!(action.len() <= super::util::MAX_APPROVAL_ACTION_BYTES);
    assert!(!action.contains(secret));
    let action: Value = serde_json::from_str(action).unwrap();
    assert_eq!(action["operation"], json!("proc.run"));
    assert_eq!(action["details"]["argvSha256"].as_str().unwrap().len(), 64);
    assert_eq!(action["details"]["stdinPresent"], json!(false));
    assert_eq!(action["details"]["stdinBytes"], json!(0));
    assert!(action["details"]["stdinSha256"].is_null());
    assert!(action["details"]["stdinPreview"].is_null());
    assert_eq!(action["details"]["stdinPreviewTruncated"], json!(false));
    assert!(Path::new(action["details"]["resolvedExecutable"].as_str().unwrap()).is_absolute());
    assert_eq!(
        action["details"]["cwd"]["linkedPath"],
        json!("tempestmiku:")
    );
}

#[tokio::test]
async fn proc_run_approval_binds_raw_stdin_digest_and_bounded_redacted_preview() {
    let root = tempfile::tempdir().unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let capture = Arc::new(CaptureThenApprove::default());
    let invocation =
        InvocationCtx::with_approvals(ctx().grants, capture.clone(), Duration::from_secs(1));
    let proc_run = ProcRunFn::with_timeout_ms(
        temp_linked_with_commands(root.path(), FsMode::Rw, vec!["cat".to_string()], Vec::new()),
        ArtifactStore::open(artifact_dir.path(), "proc-stdin-approval").unwrap(),
        180_000,
    );
    let secret = "sk-testsecret123456";
    let stdin = format!("{secret}\n{}", "界".repeat(300));

    let result = proc_run
        .call(
            json!({"cmd":"cat","cwd":"tempestmiku:","stdin":stdin}),
            &invocation,
        )
        .await
        .unwrap();
    assert_eq!(result["exitCode"], json!(0));

    let actions = capture.actions.lock().unwrap();
    let action: Value = serde_json::from_str(actions.last().unwrap()).unwrap();
    let details = &action["details"];
    let expected_digest = hex::encode(Sha256::digest(stdin.as_bytes()));
    assert_eq!(details["stdinPresent"], json!(true));
    assert_eq!(details["stdinBytes"], json!(stdin.len()));
    assert_eq!(details["stdinSha256"], json!(expected_digest));
    assert_eq!(details["stdinPreviewTruncated"], json!(true));
    let preview = details["stdinPreview"].as_str().unwrap();
    assert!(preview.len() <= 256);
    assert!(preview.contains("[REDACTED_TOKEN]"));
    assert!(preview.contains("...[truncated:"));
    assert!(!preview.contains(secret));
}

#[tokio::test]
async fn proc_run_rejects_many_args_that_cannot_fit_the_approval_prompt() {
    let root = tempfile::tempdir().unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let capture = Arc::new(CaptureThenApprove::default());
    let invocation =
        InvocationCtx::with_approvals(ctx().grants, capture.clone(), Duration::from_secs(1));
    let proc_run = ProcRunFn::with_timeout_ms(
        temp_linked_with_commands(
            root.path(),
            FsMode::Rw,
            vec!["echo".to_string()],
            Vec::new(),
        ),
        ArtifactStore::open(artifact_dir.path(), "proc-many-args").unwrap(),
        180_000,
    );

    let error = proc_run
        .call(
            json!({
                "cmd":"echo",
                "args": vec!["x"; 33],
                "cwd":"tempestmiku:"
            }),
            &invocation,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::InvalidArgs(_)));
    assert!(error.to_string().contains("approval prompt"));
    assert!(capture.actions.lock().unwrap().is_empty());

    let error = proc_run
        .call(
            json!({
                "cmd":"x".repeat(513),
                "args": [],
                "cwd":"tempestmiku:"
            }),
            &invocation,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::InvalidArgs(_)));
    assert!(error.to_string().contains("approval prompt"));
    assert!(capture.actions.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn proc_run_rejects_cwd_swapped_to_symlink_during_approval() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let work = root.path().join("work");
    let parked = root.path().join("parked-work");
    fs::create_dir(&work).unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let proc_run = ProcRunFn::with_timeout_ms(
        temp_linked_with_commands(root.path(), FsMode::Rw, vec!["pwd".to_string()], Vec::new()),
        ArtifactStore::open(artifact_dir.path(), "proc-cwd-race").unwrap(),
        180_000,
    );
    let approval = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(SwapDirectoryThenApprove {
            path: work,
            parked,
            replacement: outside.path().to_path_buf(),
        }),
        Duration::from_secs(1),
    );

    let error = proc_run
        .call(json!({"cmd":"pwd","cwd":"tempestmiku:work"}), &approval)
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::InvalidPath(_)));
}

#[tokio::test]
async fn proc_run_rejects_command_policy_narrowed_during_approval() {
    let root = tempfile::tempdir().unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let linked = temp_linked_with_commands(
        root.path(),
        FsMode::Rw,
        vec!["echo".to_string()],
        Vec::new(),
    );
    let proc_run = ProcRunFn::with_timeout_ms(
        linked.clone(),
        ArtifactStore::open(artifact_dir.path(), "proc-policy-race").unwrap(),
        180_000,
    );
    let approval = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(NarrowPolicyThenApprove {
            linked,
            root: root.path().canonicalize().unwrap(),
        }),
        Duration::from_secs(1),
    );

    let error = proc_run
        .call(
            json!({"cmd":"echo","args":["blocked"],"cwd":"tempestmiku:"}),
            &approval,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::CapabilityDenied(_)));
}

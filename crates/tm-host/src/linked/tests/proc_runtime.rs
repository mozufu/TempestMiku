use super::support::*;
use super::*;

#[tokio::test]
async fn proc_run_accepts_bounded_utf8_stdin_and_spills_echoed_output() {
    let root = tempfile::tempdir().unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(artifact_dir.path(), "proc-stdin").unwrap();
    let read_store = store.clone();
    let proc_run = ProcRunFn::with_timeout_ms(
        temp_linked_with_commands(
            root.path(),
            FsMode::Rw,
            vec!["cat".to_string(), "sleep".to_string()],
            vec![vec!["cat".to_string()], vec!["sleep".to_string()]],
        ),
        store,
        180_000,
    );

    let echoed = call_fn(
        &proc_run,
        json!({"cmd":"cat","cwd":"tempestmiku:","stdin":"hello, 世界"}),
        &approved_ctx(),
    )
    .await;
    assert_eq!(echoed["exitCode"], json!(0));
    assert_eq!(echoed["stdout"], json!("hello, 世界"));

    let empty = call_fn(
        &proc_run,
        json!({"cmd":"cat","cwd":"tempestmiku:","stdin":""}),
        &approved_ctx(),
    )
    .await;
    assert_eq!(empty["exitCode"], json!(0));
    assert_eq!(empty["stdout"], json!(""));

    let spill = call_fn(
        &proc_run,
        json!({
            "cmd":"cat",
            "cwd":"tempestmiku:",
            "stdin":"x".repeat(60_000),
            "outputBytes":1000
        }),
        &approved_ctx(),
    )
    .await;
    assert_eq!(spill["truncated"], json!(true));
    let persisted = read_store
        .read(spill["artifact"]["uri"].as_str().unwrap(), None)
        .unwrap();
    assert_eq!(persisted.content.len(), 60_000);
}

#[tokio::test]
async fn proc_run_rejects_non_string_or_oversized_stdin_and_bounds_timeout() {
    let root = tempfile::tempdir().unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(artifact_dir.path(), "proc-stdin-errors").unwrap();
    let proc_run = ProcRunFn::with_timeout_ms(
        temp_linked_with_commands(
            root.path(),
            FsMode::Rw,
            vec!["cat".to_string(), "sleep".to_string()],
            vec![vec!["cat".to_string()], vec!["sleep".to_string()]],
        ),
        store,
        180_000,
    );

    for stdin in [json!(["not", "text"]), json!({"not":"text"}), json!(42)] {
        let err = proc_run
            .call(
                json!({"cmd":"cat","cwd":"tempestmiku:","stdin":stdin}),
                &ctx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::InvalidArgs(_)));
        assert!(err.to_string().contains("UTF-8 string"));
    }

    let err = proc_run
        .call(
            json!({
                "cmd":"cat",
                "cwd":"tempestmiku:",
                "stdin":"x".repeat(1024 * 1024 + 1)
            }),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::InvalidArgs(_)));
    assert!(err.to_string().contains("1048576"));

    let timed_out = call_fn(
        &proc_run,
        json!({
            "cmd":"sleep",
            "args":["1"],
            "cwd":"tempestmiku:",
            "stdin":"x",
            "timeoutMs":1
        }),
        &approved_ctx(),
    )
    .await;
    assert_eq!(timed_out["timedOut"], json!(true));
    assert_eq!(timed_out["exitCode"], json!(-1));
}

#[tokio::test]
async fn proc_run_configured_timeout_updates_docs_and_caps_commands() {
    let root = tempfile::tempdir().unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let proc_run = ProcRunFn::with_timeout_ms(
        temp_linked_with_commands(
            root.path(),
            FsMode::Rw,
            vec!["sleep".to_string()],
            vec![vec!["sleep".to_string()]],
        ),
        ArtifactStore::open(artifact_dir.path(), "proc-configured-timeout").unwrap(),
        25,
    );
    assert_eq!(
        proc_run.docs().args_schema["properties"]["timeoutMs"]["maximum"],
        json!(25)
    );
    assert_eq!(
        proc_run.docs().args_schema["properties"]["timeoutMs"]["default"],
        json!(25)
    );

    let timed_out = call_fn(
        &proc_run,
        json!({
            "cmd":"sleep",
            "args":["1"],
            "cwd":"tempestmiku:",
            "timeoutMs":500
        }),
        &approved_ctx(),
    )
    .await;
    assert_eq!(timed_out["timedOut"], json!(true));
    assert!(timed_out["durationMs"].as_u64().unwrap() < 500);
}

#[cfg(unix)]
#[tokio::test]
async fn proc_run_timeout_kills_descendant_processes() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("descendant-survived");
    let artifact_dir = tempfile::tempdir().unwrap();
    let proc_run = ProcRunFn::with_timeout_ms(
        temp_linked_with_commands(
            root.path(),
            FsMode::Rw,
            vec!["python3".to_string()],
            vec![vec!["python3".to_string()]],
        ),
        ArtifactStore::open(artifact_dir.path(), "proc-tree-timeout").unwrap(),
        180_000,
    );
    let descendant = format!(
        "import time; time.sleep(0.3); open({:?}, 'w').write('leaked')",
        marker
    );
    let parent = format!(
        "import subprocess, sys, time; subprocess.Popen([sys.executable, '-c', {:?}]); time.sleep(5)",
        descendant
    );

    let timed_out = call_fn(
        &proc_run,
        json!({
            "cmd":"python3",
            "args":["-c", parent],
            "cwd":"tempestmiku:",
            "timeoutMs":50
        }),
        &approved_ctx(),
    )
    .await;
    assert_eq!(timed_out["timedOut"], json!(true));

    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        !marker.exists(),
        "a proc.run descendant survived the timeout and wrote {}",
        marker.display()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_proc_run_kills_descendant_processes() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("descendant-survived-cancel");
    let artifact_dir = tempfile::tempdir().unwrap();
    let proc_run = ProcRunFn::with_timeout_ms(
        temp_linked_with_commands(
            root.path(),
            FsMode::Rw,
            vec!["python3".to_string()],
            vec![vec!["python3".to_string()]],
        ),
        ArtifactStore::open(artifact_dir.path(), "proc-tree-cancel").unwrap(),
        180_000,
    );
    let descendant = format!(
        "import time; time.sleep(0.3); open({:?}, 'w').write('leaked')",
        marker
    );
    let parent = format!(
        "import subprocess, sys, time; subprocess.Popen([sys.executable, '-c', {:?}]); time.sleep(5)",
        descendant
    );
    let invocation = approved_ctx();

    {
        let call = proc_run.call(
            json!({
                "cmd":"python3",
                "args":["-c", parent],
                "cwd":"tempestmiku:",
                "timeoutMs":5_000
            }),
            &invocation,
        );
        tokio::pin!(call);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut call)
                .await
                .is_err()
        );
    }

    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        !marker.exists(),
        "a proc.run descendant survived cancellation and wrote {}",
        marker.display()
    );
}

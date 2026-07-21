use super::support::*;
use super::*;

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn init_repo(root: &Path) {
    let status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
    fs::write(root.join("tracked.txt"), "before\n").unwrap();
    let add = std::process::Command::new("git")
        .args(["add", "tracked.txt"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(add.success());
    let commit = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=TempestMiku Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(commit.success());
}

#[tokio::test]
async fn git_status_and_diff_are_fixed_read_only_inspection_capabilities() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    init_repo(root.path());
    fs::write(root.path().join("tracked.txt"), "after\n").unwrap();
    fs::write(root.path().join("untracked.txt"), "new\n").unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(artifacts.path(), "git-read").unwrap();
    let linked = temp_linked(root.path(), FsMode::Rw);

    let status = GitReadFn::status(
        linked.clone(),
        store.clone(),
        docs("git.status", "git", "Inspect status", true),
    )
    .call(json!({"cwd":"tempestmiku:"}), &ctx())
    .await
    .unwrap();
    assert_eq!(status["operation"], json!("git.status"));
    assert_eq!(status["exitCode"], json!(0));
    let status_out = status["stdout"].as_str().unwrap();
    assert!(status_out.contains("tracked.txt"));
    assert!(status_out.contains("untracked.txt"));

    let diff = GitReadFn::diff(linked, store, docs("git.diff", "git", "Inspect diff", true))
        .call(json!({"cwd":"tempestmiku:"}), &ctx())
        .await
        .unwrap();
    assert_eq!(diff["operation"], json!("git.diff"));
    assert_eq!(diff["exitCode"], json!(0));
    let diff_out = diff["stdout"].as_str().unwrap();
    assert!(diff_out.contains("diff --git a/tracked.txt b/tracked.txt"));
    assert!(diff_out.contains("-before"));
    assert!(diff_out.contains("+after"));
}

#[tokio::test]
async fn git_capabilities_reject_extra_arguments_and_missing_grants() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    init_repo(root.path());
    let artifacts = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(artifacts.path(), "git-deny").unwrap();
    let status = GitReadFn::status(
        temp_linked(root.path(), FsMode::Ro),
        store,
        docs("git.status", "git", "Inspect status", true),
    );

    let extra = status
        .call(
            json!({"cwd":"tempestmiku:","args":["--porcelain=v2"]}),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(extra, HostError::InvalidArgs(_)));

    let denied = HostRegistry::new();
    let error = denied.docs(
        "git.status",
        &InvocationCtx::new(CapabilityGrants::default()),
    );
    assert!(matches!(error, Err(HostError::NotFound(_))));

    let direct = status
        .call(
            json!({"cwd":"tempestmiku:"}),
            &InvocationCtx::new(CapabilityGrants::default()),
        )
        .await;
    assert!(matches!(direct, Err(HostError::CapabilityDenied(_))));
}

#[tokio::test]
async fn git_log_and_commit_require_approval_and_commit_only_the_index() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    init_repo(root.path());
    fs::write(root.path().join("tracked.txt"), "staged\n").unwrap();
    fs::write(root.path().join("unstaged.txt"), "not committed\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    let artifacts = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(artifacts.path(), "git-mutate").unwrap();
    let linked = temp_linked(root.path(), FsMode::Rw);

    let log = GitReadFn::log(
        linked.clone(),
        store.clone(),
        docs("git.log", "git", "Inspect log", true),
    );
    let denied_log = log
        .call(json!({"cwd":"tempestmiku:"}), &ctx())
        .await
        .unwrap_err();
    assert!(matches!(denied_log, HostError::ApprovalTimeout(_)));
    let log_value = log
        .call(json!({"cwd":"tempestmiku:"}), &approved_ctx())
        .await
        .unwrap();
    assert_eq!(log_value["operation"], json!("git.log"));
    assert!(log_value["stdout"].as_str().unwrap().contains("fixture"));

    let commit = GitReadFn::commit(
        linked,
        store,
        docs("git.commit", "git", "Commit index", true),
    );
    let denied_commit = commit
        .call(
            json!({"cwd":"tempestmiku:","message":"test commit"}),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(denied_commit, HostError::ApprovalTimeout(_)));
    let committed = commit
        .call(
            json!({"cwd":"tempestmiku:","message":"test commit"}),
            &approved_ctx(),
        )
        .await
        .unwrap();
    assert_eq!(committed["operation"], json!("git.commit"));
    assert_eq!(committed["exitCode"], json!(0));

    let committed_files = std::process::Command::new("git")
        .args(["show", "--pretty=", "--name-only", "HEAD"])
        .current_dir(root.path())
        .output()
        .unwrap();
    let committed_files = String::from_utf8(committed_files.stdout).unwrap();
    assert!(committed_files.contains("tracked.txt"));
    assert!(!committed_files.contains("unstaged.txt"));
    assert!(root.path().join("unstaged.txt").exists());
}

#[tokio::test]
async fn git_push_and_pull_reject_non_https_upstreams_before_approval() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    init_repo(root.path());
    for args in [
        vec!["remote", "add", "origin", "ssh://example.invalid/repo.git"],
        vec!["branch", "--set-upstream-to", "origin/main"],
    ] {
        let output = std::process::Command::new("git")
            .args(&args)
            .current_dir(root.path())
            .output()
            .unwrap();
        if args[0] == "branch" && !output.status.success() {
            assert!(
                std::process::Command::new("git")
                    .args(["config", "branch.main.remote", "origin"])
                    .current_dir(root.path())
                    .status()
                    .unwrap()
                    .success()
            );
            assert!(
                std::process::Command::new("git")
                    .args(["config", "branch.main.merge", "refs/heads/main"])
                    .current_dir(root.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
    let artifacts = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(artifacts.path(), "git-network-deny").unwrap();
    let linked = temp_linked(root.path(), FsMode::Rw);
    let capture = Arc::new(CaptureThenApprove::default());
    let invocation =
        InvocationCtx::with_approvals(ctx().grants, capture.clone(), Duration::from_secs(1));
    for function in [
        GitReadFn::push(
            linked.clone(),
            store.clone(),
            docs("git.push", "git", "Push", true),
        ),
        GitReadFn::pull(
            linked.clone(),
            store.clone(),
            docs("git.pull", "git", "Pull", true),
        ),
    ] {
        let error = function
            .call(json!({"cwd":"tempestmiku:"}), &invocation)
            .await
            .unwrap_err();
        assert!(matches!(error, HostError::CapabilityDenied(_)));
    }
    assert!(capture.actions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn approval_payload_binds_git_operation_and_redacts_commit_message() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    init_repo(root.path());
    fs::write(root.path().join("tracked.txt"), "changed\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    let artifacts = tempfile::tempdir().unwrap();
    let capture = Arc::new(CaptureThenApprove::default());
    let invocation =
        InvocationCtx::with_approvals(ctx().grants, capture.clone(), Duration::from_secs(1));
    let secret = "sk-testsecret123456";
    GitReadFn::commit(
        temp_linked(root.path(), FsMode::Rw),
        ArtifactStore::open(artifacts.path(), "git-approval").unwrap(),
        docs("git.commit", "git", "Commit", true),
    )
    .call(
        json!({"cwd":"tempestmiku:","message":format!("commit {secret}")}),
        &invocation,
    )
    .await
    .unwrap();
    let actions = capture.actions.lock().unwrap();
    let raw = actions.last().unwrap();
    assert!(!raw.contains(secret));
    let action: Value = serde_json::from_str(raw).unwrap();
    assert_eq!(action["operation"], json!("git.commit"));
    assert_eq!(
        action["details"]["message"]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        action["details"]["fixedArgv"]
            .as_array()
            .unwrap()
            .last()
            .unwrap(),
        &json!("[commit message; see digest/preview]")
    );
}

#[tokio::test]
async fn git_commit_rejects_index_changes_while_approval_is_pending() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    init_repo(root.path());
    fs::write(root.path().join("tracked.txt"), "first staged\n").unwrap();
    fs::write(root.path().join("second.txt"), "second\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    let artifacts = tempfile::tempdir().unwrap();
    let invocation = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(RunGitThenApprove {
            cwd: root.path().to_path_buf(),
            args: vec!["add".to_string(), "second.txt".to_string()],
        }),
        Duration::from_secs(1),
    );
    let error = GitReadFn::commit(
        temp_linked(root.path(), FsMode::Rw),
        ArtifactStore::open(artifacts.path(), "git-stale-approval").unwrap(),
        docs("git.commit", "git", "Commit", true),
    )
    .call(
        json!({"cwd":"tempestmiku:","message":"must be rejected"}),
        &invocation,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, HostError::InvalidArgs(_)));

    let subject = std::process::Command::new("git")
        .args(["log", "-1", "--format=%s"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(subject.stdout).unwrap().trim(), "fixture");
}

fn git_fn(operation: &str, root: &Path, artifacts: &Path) -> GitReadFn {
    let linked = temp_linked(root, FsMode::Rw);
    let store = ArtifactStore::open(artifacts, operation).unwrap();
    let tool_docs = docs(operation, "git", operation, true);
    match operation {
        "git.grep" => GitReadFn::grep(linked, store, tool_docs),
        "git.show" => GitReadFn::show(linked, store, tool_docs),
        "git.clone" => GitReadFn::clone(linked, store, tool_docs),
        "git.init" => GitReadFn::init(linked, store, tool_docs),
        "git.add" => GitReadFn::add(linked, store, tool_docs),
        "git.mv" => GitReadFn::mv(linked, store, tool_docs),
        "git.restore" => GitReadFn::restore(linked, store, tool_docs),
        "git.rm" => GitReadFn::rm(linked, store, tool_docs),
        "git.bisect" => GitReadFn::bisect(linked, store, tool_docs),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn git_grep_and_show_are_literal_bounded_reads() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    init_repo(root.path());
    let grep = git_fn("git.grep", root.path(), artifacts.path());
    let value = grep
        .call(
            json!({"cwd":"tempestmiku:","pattern":"before","caseSensitive":false}),
            &ctx(),
        )
        .await
        .unwrap();
    assert!(value["stdout"].as_str().unwrap().contains("tracked.txt:1"));
    assert!(
        grep.call(json!({"cwd":"tempestmiku:","pattern":""}), &ctx())
            .await
            .is_err()
    );

    let show = git_fn("git.show", root.path(), artifacts.path());
    assert!(matches!(
        show.call(json!({"cwd":"tempestmiku:"}), &ctx())
            .await
            .unwrap_err(),
        HostError::ApprovalTimeout(_)
    ));
    let value = show
        .call(json!({"cwd":"tempestmiku:"}), &approved_ctx())
        .await
        .unwrap();
    assert!(value["stdout"].as_str().unwrap().contains("fixture"));
    assert!(matches!(
        show.call(
            json!({"cwd":"tempestmiku:","revision":"HEAD~1"}),
            &approved_ctx()
        )
        .await
        .unwrap_err(),
        HostError::InvalidArgs(_)
    ));
}

#[tokio::test]
async fn git_init_and_curated_file_mutations_require_approval() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let init = git_fn("git.init", root.path(), artifacts.path());
    assert!(matches!(
        init.call(json!({"cwd":"tempestmiku:"}), &ctx())
            .await
            .unwrap_err(),
        HostError::ApprovalTimeout(_)
    ));
    init.call(json!({"cwd":"tempestmiku:"}), &approved_ctx())
        .await
        .unwrap();
    assert!(root.path().join(".git").is_dir());
    fs::write(root.path().join("one.txt"), "one\n").unwrap();
    let add = git_fn("git.add", root.path(), artifacts.path());
    add.call(
        json!({"cwd":"tempestmiku:","paths":["one.txt"]}),
        &approved_ctx(),
    )
    .await
    .unwrap();
    let staged = std::process::Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        String::from_utf8(staged.stdout)
            .unwrap()
            .contains("one.txt")
    );
    assert!(matches!(
        add.call(
            json!({"cwd":"tempestmiku:","paths":["../escape"]}),
            &approved_ctx()
        )
        .await
        .unwrap_err(),
        HostError::InvalidArgs(_)
    ));

    assert!(
        std::process::Command::new("git")
            .args(["commit", "--quiet", "-m", "one", "--no-gpg-sign"])
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    let mv = git_fn("git.mv", root.path(), artifacts.path());
    mv.call(
        json!({"cwd":"tempestmiku:","path":"one.txt","dest":"two.txt"}),
        &approved_ctx(),
    )
    .await
    .unwrap();
    assert!(root.path().join("two.txt").exists());
    assert!(matches!(
        mv.call(
            json!({"cwd":"tempestmiku:","path":"dir/two.txt","dest":"x"}),
            &approved_ctx()
        )
        .await
        .unwrap_err(),
        HostError::InvalidArgs(_)
    ));
    let protected = git_fn("git.rm", root.path(), artifacts.path())
        .call(
            json!({"cwd":"tempestmiku:","paths":["two.txt"]}),
            &approved_ctx(),
        )
        .await
        .unwrap();
    assert_eq!(protected["exitCode"], json!(1));
    assert!(root.path().join("two.txt").exists());
    assert!(
        std::process::Command::new("git")
            .args(["commit", "--quiet", "-m", "rename", "--no-gpg-sign"])
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(root.path().join("two.txt"), "changed\n").unwrap();
    git_fn("git.restore", root.path(), artifacts.path())
        .call(
            json!({"cwd":"tempestmiku:","paths":["two.txt"]}),
            &approved_ctx(),
        )
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("two.txt")).unwrap(),
        "one\n"
    );
    let removed = git_fn("git.rm", root.path(), artifacts.path())
        .call(
            json!({"cwd":"tempestmiku:","paths":["two.txt"]}),
            &approved_ctx(),
        )
        .await
        .unwrap();
    assert_eq!(removed["exitCode"], json!(0), "{}", removed["stderr"]);
    assert!(!root.path().join("two.txt").exists());
}

#[tokio::test]
async fn git_add_and_restore_reject_repository_filters() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    init_repo(root.path());
    assert!(
        std::process::Command::new("git")
            .args(["config", "--local", "filter.evil.clean", "echo unsafe"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    for operation in ["git.add", "git.restore"] {
        let error = git_fn(operation, root.path(), artifacts.path())
            .call(
                json!({"cwd":"tempestmiku:","paths":["tracked.txt"]}),
                &approved_ctx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, HostError::CapabilityDenied(_)));
    }
}

#[tokio::test]
async fn git_add_and_restore_reject_included_repository_filters() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    init_repo(root.path());
    let marker = root.path().join("filter-ran");
    let include = root.path().join("included-filter.cfg");
    fs::write(
        &include,
        format!(
            "[filter \"evil\"]\n\tclean = touch {}\n\tsmudge = touch {}\n",
            marker.display(),
            marker.display()
        ),
    )
    .unwrap();
    assert!(
        std::process::Command::new("git")
            .args([
                "config",
                "--local",
                "include.path",
                include.to_str().unwrap(),
            ])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );

    for operation in ["git.add", "git.restore"] {
        let error = git_fn(operation, root.path(), artifacts.path())
            .call(
                json!({"cwd":"tempestmiku:","paths":["tracked.txt"]}),
                &approved_ctx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, HostError::CapabilityDenied(_)));
        assert!(!marker.exists(), "{operation} executed an included filter");
    }
}

#[tokio::test]
async fn git_approval_state_supports_large_indexes() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    init_repo(root.path());
    for index in 0..400 {
        fs::write(root.path().join(format!("tracked-{index:03}.txt")), "x\n").unwrap();
    }
    assert!(
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .args(["commit", "--quiet", "-m", "large index", "--no-gpg-sign"])
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(root.path().join("tracked-000.txt"), "changed\n").unwrap();

    let result = git_fn("git.add", root.path(), artifacts.path())
        .call(
            json!({"cwd":"tempestmiku:","paths":["tracked-000.txt"]}),
            &approved_ctx(),
        )
        .await
        .unwrap();
    assert_eq!(result["exitCode"], json!(0), "{}", result["stderr"]);
}

#[tokio::test]
async fn git_bisect_uses_no_checkout_and_preserves_worktree() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    init_repo(root.path());
    let good = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    fs::write(root.path().join("tracked.txt"), "second\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["commit", "-am", "second", "--quiet", "--no-gpg-sign"])
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    let bad = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let before = fs::read_to_string(root.path().join("tracked.txt")).unwrap();
    let bisect = git_fn("git.bisect", root.path(), artifacts.path());
    bisect
        .call(
            json!({"cwd":"tempestmiku:","action":"start","bad":bad,"good":[good]}),
            &approved_ctx(),
        )
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("tracked.txt")).unwrap(),
        before
    );
    assert!(root.path().join(".git/BISECT_HEAD").exists());
    bisect
        .call(
            json!({"cwd":"tempestmiku:","action":"reset"}),
            &approved_ctx(),
        )
        .await
        .unwrap();
    assert!(!root.path().join(".git/BISECT_HEAD").exists());
}

#[tokio::test]
async fn git_clone_rejects_unsafe_urls_and_nonempty_or_repository_cwds() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let clone = git_fn("git.clone", root.path(), artifacts.path());
    assert!(matches!(
        clone
            .call(
                json!({"cwd":"tempestmiku:","url":"ssh://example.invalid/repo.git"}),
                &approved_ctx()
            )
            .await
            .unwrap_err(),
        HostError::CapabilityDenied(_)
    ));
    fs::write(root.path().join("occupied"), "x").unwrap();
    assert!(matches!(
        clone
            .call(
                json!({"cwd":"tempestmiku:","url":"https://example.invalid/repo.git"}),
                &approved_ctx()
            )
            .await
            .unwrap_err(),
        HostError::InvalidArgs(_)
    ));
    fs::remove_file(root.path().join("occupied")).unwrap();
    init_repo(root.path());
    assert!(matches!(
        clone
            .call(
                json!({"cwd":"tempestmiku:","url":"https://example.invalid/repo.git"}),
                &approved_ctx()
            )
            .await
            .unwrap_err(),
        HostError::InvalidArgs(_)
    ));
}

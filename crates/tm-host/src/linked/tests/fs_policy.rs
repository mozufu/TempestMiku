use super::support::*;
use super::*;

#[tokio::test]
async fn fs_read_write_ls_find_honor_mode_and_gitignore() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(root.path().join("ignored.txt"), "ignored").unwrap();
    let linked = temp_linked(root.path(), FsMode::Rw);
    let write = FsWriteFn::new(linked.clone());
    let value = call_fn(
        &write,
        json!({"path":"tempestmiku:src/lib.rs","data":"pub fn x() {}\n","createParents":true}),
        &ctx(),
    )
    .await;
    assert_eq!(value["bytesWritten"], json!(14));
    let read = FsReadFn::new(linked.clone());
    let content = call_fn(&read, json!({"path":"tempestmiku:src/lib.rs"}), &ctx()).await;
    assert_eq!(content["content"], json!("pub fn x() {}\n"));
    let ls = FsLsFn::new(linked.clone());
    let entries = call_fn(&ls, json!({"path":"tempestmiku:","recursive":true}), &ctx()).await;
    assert!(entries.to_string().contains("sizeBytes"));
    let find = FsFindFn::new(linked);
    let omitted = call_fn(
        &find,
        json!({"patterns":"ignored.txt","cwd":"tempestmiku:"}),
        &ctx(),
    )
    .await;
    assert_eq!(omitted.as_array().unwrap().len(), 0);
    let included = call_fn(
        &find,
        json!({"patterns":"ignored.txt","cwd":"tempestmiku:","respectGitignore":false}),
        &ctx(),
    )
    .await;
    assert_eq!(included.as_array().unwrap().len(), 1);
}

#[test]
fn hostile_gitignore_patterns_are_bounded_before_compilation() {
    let root = tempfile::tempdir().unwrap();
    let linked = temp_linked(root.path(), FsMode::Ro);
    let resolved = linked.resolve_spec(Some("tempestmiku:")).unwrap();
    let gitignore = root.path().join(".gitignore");

    let too_many = (0..=MAX_GLOB_PATTERNS)
        .map(|index| format!("ignored-{index}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&gitignore, too_many).unwrap();
    let error = load_simple_gitignore(&resolved.policy, resolved.root_identity).unwrap_err();
    assert!(matches!(error, HostError::InvalidArgs(_)));
    assert!(error.to_string().contains("pattern count"));

    fs::write(&gitignore, "x".repeat(MAX_GLOB_PATTERN_BYTES + 1)).unwrap();
    let error = load_simple_gitignore(&resolved.policy, resolved.root_identity).unwrap_err();
    assert!(matches!(error, HostError::InvalidArgs(_)));
    assert!(error.to_string().contains("UTF-8 bytes"));

    let aggregate_patterns =
        vec!["x".repeat(MAX_GLOB_PATTERN_BYTES); MAX_GLOB_TOTAL_BYTES / MAX_GLOB_PATTERN_BYTES + 1]
            .join("\n");
    fs::write(&gitignore, aggregate_patterns).unwrap();
    let error = load_simple_gitignore(&resolved.policy, resolved.root_identity).unwrap_err();
    assert!(matches!(error, HostError::InvalidArgs(_)));
    assert!(error.to_string().contains("aggregate"));
}

#[tokio::test]
async fn dynamic_link_narrowing_revokes_write_access() {
    let root = tempfile::tempdir().unwrap();
    let linked = temp_linked(root.path(), FsMode::Rw);
    let write = FsWriteFn::new(linked.clone());
    call_fn(
        &write,
        json!({"path":"tempestmiku:src/lib.rs","data":"pub fn x() {}\n","createParents":true}),
        &ctx(),
    )
    .await;

    linked
        .insert_policy(FsPolicy {
            alias: "tempestmiku".to_string(),
            root: root.path().canonicalize().unwrap(),
            mode: FsMode::Ro,
            commands: std::collections::BTreeSet::new(),
            safe_args: Vec::new(),
        })
        .unwrap();
    let err = write
        .call(
            json!({"path":"tempestmiku:src/blocked.rs","data":"nope","createParents":true}),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        HostError::CapabilityDenied("tempestmiku:src/blocked.rs is read-only".to_string())
    );
    assert!(!root.path().join("src/blocked.rs").exists());
}

#[test]
fn policy_narrowing_cannot_slip_between_stable_check_and_commit() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.txt");
    fs::write(&path, "old").unwrap();
    let linked = temp_linked(root.path(), FsMode::Rw);
    let revision = linked.revision();
    let (checked_tx, checked_rx) = mpsc::channel();
    let (commit_tx, commit_rx) = mpsc::channel();
    let commit_linked = linked.clone();
    let commit_path = path.clone();
    let commit = std::thread::spawn(move || {
        commit_linked.with_stable_policy_snapshot(revision, |linked| {
            let resolved = linked.resolve_spec(Some("tempestmiku:state.txt"))?;
            if resolved.policy.mode != FsMode::Rw {
                return Err(HostError::CapabilityDenied(
                    "stable policy unexpectedly narrowed".to_string(),
                ));
            }
            checked_tx.send(()).unwrap();
            commit_rx.recv().unwrap();
            fs::write(&commit_path, "committed")
                .map_err(|error| HostError::HostCall(error.to_string()))
        })
    });
    checked_rx.recv().unwrap();

    let (attempt_tx, attempt_rx) = mpsc::channel();
    let (updated_tx, updated_rx) = mpsc::channel();
    let update_linked = linked.clone();
    let update_root = root.path().canonicalize().unwrap();
    let update = std::thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        let result = update_linked.insert_policy(FsPolicy {
            alias: "tempestmiku".to_string(),
            root: update_root,
            mode: FsMode::Ro,
            commands: std::collections::BTreeSet::new(),
            safe_args: Vec::new(),
        });
        updated_tx.send(result).unwrap();
    });
    attempt_rx.recv().unwrap();
    let update_was_blocked = updated_rx.recv_timeout(Duration::from_millis(50)).is_err();

    commit_tx.send(()).unwrap();
    commit.join().unwrap().unwrap();
    updated_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    update.join().unwrap();
    assert!(update_was_blocked);
    assert_eq!(fs::read_to_string(path).unwrap(), "committed");
    assert_eq!(linked.policy("tempestmiku").unwrap().mode, FsMode::Ro);
}

#[test]
fn policy_removal_waits_for_a_stable_read_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let linked = temp_linked(root.path(), FsMode::Ro);
    let revision = linked.revision();
    let (checked_tx, checked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let read_linked = linked.clone();
    let read = std::thread::spawn(move || {
        read_linked.with_stable_policy_snapshot(revision, |linked| {
            linked.resolve_spec(Some("tempestmiku:"))?;
            checked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        })
    });
    checked_rx.recv().unwrap();

    let (attempt_tx, attempt_rx) = mpsc::channel();
    let (removed_tx, removed_rx) = mpsc::channel();
    let remove_linked = linked.clone();
    let remove = std::thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        removed_tx
            .send(remove_linked.remove_policy("tempestmiku"))
            .unwrap();
    });
    attempt_rx.recv().unwrap();
    let removal_was_blocked = removed_rx.recv_timeout(Duration::from_millis(50)).is_err();

    release_tx.send(()).unwrap();
    read.join().unwrap().unwrap();
    removed_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    remove.join().unwrap();
    assert!(removal_was_blocked);
    assert!(matches!(
        linked.policy("tempestmiku"),
        Err(HostError::InvalidPath(_))
    ));
}

#[tokio::test]
async fn fs_write_gates_overwrites_only() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("note.txt");
    let write = FsWriteFn::new(temp_linked(root.path(), FsMode::Rw));
    let denied_ctx = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(StaticApproval(ApprovalDecision::Denied)),
        Duration::from_secs(1),
    );
    write
        .call(
            json!({"path":"tempestmiku:new.txt","data":"new"}),
            &denied_ctx,
        )
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("new.txt")).unwrap(),
        "new"
    );

    fs::write(&path, "old").unwrap();
    let denied = write
        .call(
            json!({"path":"tempestmiku:note.txt","data":"denied","overwrite":true}),
            &denied_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(denied, HostError::ApprovalDenied(_)));
    assert_eq!(fs::read_to_string(&path).unwrap(), "old");

    let timed_out = write
        .call(
            json!({"path":"tempestmiku:note.txt","data":"timeout","overwrite":true}),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(timed_out, HostError::ApprovalTimeout(_)));
    assert_eq!(fs::read_to_string(&path).unwrap(), "old");

    let approved_ctx = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        Duration::from_secs(1),
    );
    write
        .call(
            json!({"path":"tempestmiku:note.txt","data":"approved","overwrite":true}),
            &approved_ctx,
        )
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "approved");
}

#[tokio::test]
async fn fs_write_rejects_content_changed_during_approval() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("note.txt");
    fs::write(&path, "old").unwrap();
    let write = FsWriteFn::new(temp_linked(root.path(), FsMode::Rw));
    let approval = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(RewriteThenApprove {
            path: path.clone(),
            data: b"concurrent".to_vec(),
        }),
        Duration::from_secs(1),
    );

    let error = write
        .call(
            json!({"path":"tempestmiku:note.txt","data":"agent","overwrite":true}),
            &approval,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::InvalidArgs(_)));
    assert_eq!(fs::read_to_string(path).unwrap(), "concurrent");
}

#[tokio::test]
async fn fs_write_rejects_policy_narrowed_during_approval() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("note.txt");
    fs::write(&path, "old").unwrap();
    let linked = temp_linked(root.path(), FsMode::Rw);
    let write = FsWriteFn::new(linked.clone());
    let approval = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(NarrowPolicyThenApprove {
            linked,
            root: root.path().canonicalize().unwrap(),
        }),
        Duration::from_secs(1),
    );

    let error = write
        .call(
            json!({"path":"tempestmiku:note.txt","data":"agent","overwrite":true}),
            &approval,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::CapabilityDenied(_)));
    assert_eq!(fs::read_to_string(path).unwrap(), "old");
}

#[cfg(unix)]
#[tokio::test]
async fn fs_write_rejects_parent_swapped_to_symlink_during_approval() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let directory = root.path().join("work");
    let parked = root.path().join("parked-work");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("note.txt"), "old").unwrap();
    fs::write(outside.path().join("note.txt"), "outside").unwrap();
    let write = FsWriteFn::new(temp_linked(root.path(), FsMode::Rw));
    let approval = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(SwapDirectoryThenApprove {
            path: directory.clone(),
            parked: parked.clone(),
            replacement: outside.path().to_path_buf(),
        }),
        Duration::from_secs(1),
    );

    let error = write
        .call(
            json!({"path":"tempestmiku:work/note.txt","data":"agent","overwrite":true}),
            &approval,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::InvalidPath(_)));
    assert_eq!(fs::read_to_string(parked.join("note.txt")).unwrap(), "old");
    assert_eq!(
        fs::read_to_string(outside.path().join("note.txt")).unwrap(),
        "outside"
    );
}

#[tokio::test]
async fn mutations_reject_files_above_the_bounded_read_limit() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large.txt");
    fs::write(&path, vec![b'x'; MAX_MUTATION_FILE_BYTES + 1]).unwrap();
    let remove = FsRemoveFn::new(temp_linked(root.path(), FsMode::Rw));
    let error = remove
        .call(
            json!({"path":"tempestmiku:large.txt","tag":"not-read"}),
            &approved_ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::InvalidArgs(_)));
    assert!(
        error
            .to_string()
            .contains(&MAX_MUTATION_FILE_BYTES.to_string())
    );
    assert!(path.exists());
}

#[tokio::test]
async fn fs_grep_returns_tag_and_context() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("lib.rs"), "before\nneedle here\nafter\n").unwrap();
    let search = FsGrepFn::new(temp_linked(root.path(), FsMode::Rw));
    let value = call_fn(
        &search,
        json!({"pattern":"needle","paths":["tempestmiku:lib.rs"],"regex":false,"contextLines":1}),
        &ctx(),
    )
    .await;
    let hit = &value.as_array().unwrap()[0];
    assert_eq!(hit["line"], json!(2));
    assert_eq!(hit["column"], json!(1));
    assert_eq!(hit["before"], json!(["before"]));
    assert_eq!(hit["after"], json!(["after"]));
    let tag = hit["tag"].as_str().unwrap();
    assert_eq!(tag.len(), 16);
    assert!(
        tag.chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
    );
}

#[tokio::test]
async fn linked_walk_and_search_arguments_are_hard_bounded() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("large.txt"),
        vec![b'x'; 4 * 1024 * 1024 + 1],
    )
    .unwrap();
    let linked = temp_linked(root.path(), FsMode::Ro);
    let ls = FsLsFn::new(linked.clone());
    let find = FsFindFn::new(linked.clone());
    let search = FsGrepFn::new(linked);

    let ls_error = ls
        .call(json!({"path":"tempestmiku:","limit":10_001}), &ctx())
        .await
        .unwrap_err();
    assert!(matches!(ls_error, HostError::InvalidArgs(_)));

    let find_error = find
        .call(json!({"patterns":[],"cwd":"tempestmiku:"}), &ctx())
        .await
        .unwrap_err();
    assert!(matches!(find_error, HostError::InvalidArgs(_)));

    for query in [
        json!({"pattern":"x","paths":["tempestmiku:large.txt"],"contextLines":21}),
        json!({"pattern":"x","paths":["tempestmiku:large.txt"],"limit":10_001}),
        json!({"pattern":"x","paths":[]}),
        json!({"pattern":"x","paths":["tempestmiku:large.txt"]}),
    ] {
        let error = search.call(query, &ctx()).await.unwrap_err();
        assert!(matches!(error, HostError::InvalidArgs(_)));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn recursive_linked_walk_rejects_trees_beyond_the_depth_budget() {
    let root = tempfile::tempdir().unwrap();
    let mut current = root.path().to_path_buf();
    for _ in 0..=MAX_SECURE_WALK_DEPTH {
        current.push("d");
        fs::create_dir(&current).unwrap();
    }
    let ls = FsLsFn::new(temp_linked(root.path(), FsMode::Ro));

    let error = ls
        .call(json!({"path":"tempestmiku:","recursive":true}), &ctx())
        .await
        .unwrap_err();

    assert!(matches!(error, HostError::InvalidArgs(_)));
    assert!(error.to_string().contains("directory levels"));
}

#[test]
fn linked_list_and_find_results_share_a_cumulative_byte_cap() {
    let mut entries = Vec::new();
    let mut bytes = 2_usize;
    for index in 0..10_000 {
        let long_name = format!("{index:05}-{}", "x".repeat(700));
        let entry = FsEntry {
            path: format!("tempestmiku:{long_name}"),
            uri: format!("linked://tempestmiku/{long_name}"),
            name: long_name,
            kind: "file".to_string(),
            size_bytes: Some(1),
            modified_at: None,
        };
        if !push_bounded_fs_entry(&mut entries, entry, 10_000, &mut bytes).unwrap() {
            break;
        }
    }
    assert!(entries.len() < 10_000);
    assert!(serde_json::to_vec(&entries).unwrap().len() <= MAX_FS_RESULT_BYTES);
}

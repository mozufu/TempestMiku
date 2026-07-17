use super::support::*;
use super::*;

#[tokio::test]
async fn fs_patch_applies_explicit_hunks_and_rejects_stale_tags() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lib.rs");
    fs::write(&path, "one\ntwo\nthree\n").unwrap();
    let tag = file_tag(&fs::read(&path).unwrap());
    let patch = FsPatchFn::new(
        temp_linked(root.path(), FsMode::Rw),
        ArtifactStore::open(root.path().join("artifacts"), "patch").unwrap(),
    );
    let value = call_fn(
        &patch,
        json!({
            "path":"tempestmiku:lib.rs",
            "tag":tag,
            "hunks":[
                {"op":"replace","startLine":2,"endLine":2,"expectedLines":["two"],"lines":["TWO"]},
                {"op":"prepend","lines":["zero"]},
                {"op":"insertAfter","line":1,"expectedLine":"one","lines":["middle"]},
                {"op":"delete","startLine":3,"endLine":3,"expectedLines":["three"]}
            ]
        }),
        &ctx(),
    )
    .await;
    assert_ne!(value["newTag"].as_str().unwrap(), tag);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "zero\none\nmiddle\nTWO\n"
    );
    let err = patch
            .call(
                json!({"path":"tempestmiku:lib.rs","tag":"deadbeefdeadbeef","hunks":[{"op":"append","lines":["x"]}]}),
                &ctx(),
            )
            .await
            .unwrap_err();
    assert!(matches!(err, HostError::InvalidArgs(_)));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "zero\none\nmiddle\nTWO\n"
    );

    let legacy_insert = patch
        .call(
            json!({
                "path":"tempestmiku:lib.rs",
                "tag":file_tag(&fs::read(&path).unwrap()),
                "hunks":[{"op":"insert","at":"tail","lines":["x"]}]
            }),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(legacy_insert, HostError::InvalidArgs(_)));

    let missing = patch
        .call(
            json!({
                "path":"tempestmiku:new.rs",
                "tag":"deadbeefdeadbeef",
                "hunks":[{"op":"append","lines":["x"]}]
            }),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(missing, HostError::InvalidPath(_)));
    assert!(!root.path().join("new.rs").exists());
}

#[tokio::test]
async fn fs_patch_rejects_missing_or_mismatched_expected_context_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lib.rs");
    fs::write(&path, "one\ntwo\nthree\n").unwrap();
    let patch = FsPatchFn::new(
        temp_linked(root.path(), FsMode::Rw),
        ArtifactStore::open(root.path().join("artifacts"), "patch-context").unwrap(),
    );

    let missing = patch
        .call(
            json!({
                "path":"tempestmiku:lib.rs",
                "tag":file_tag(&fs::read(&path).unwrap()),
                "hunks":[{"op":"replace","startLine":2,"endLine":2,"lines":["TWO"]}]
            }),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(missing, HostError::InvalidArgs(_)));

    for hunk in [
        json!({"op":"replace","startLine":2,"endLine":2,"expectedLines":["wrong"],"lines":["TWO"]}),
        json!({"op":"delete","startLine":2,"endLine":2,"expectedLines":["wrong"]}),
        json!({"op":"insertAfter","line":2,"expectedLine":"wrong","lines":["new"]}),
    ] {
        let err = patch
            .call(
                json!({
                    "path":"tempestmiku:lib.rs",
                    "tag":file_tag(&fs::read(&path).unwrap()),
                    "hunks":[hunk]
                }),
                &ctx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::InvalidArgs(_)));
        assert!(err.to_string().contains("context mismatch"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\ntwo\nthree\n");
    }
}

#[tokio::test]
async fn fs_patch_rejects_inserts_anchored_inside_replaced_ranges() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lib.rs");
    fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
    let patch = FsPatchFn::new(
        temp_linked(root.path(), FsMode::Rw),
        ArtifactStore::open(root.path().join("artifacts"), "patch-overlap").unwrap(),
    );

    for insert in [
        json!({"op":"insertAfter","line":2,"expectedLine":"two","lines":["hidden"]}),
        json!({"op":"insertBefore","line":3,"expectedLine":"three","lines":["hidden"]}),
    ] {
        let err = patch
            .call(
                json!({
                    "path":"tempestmiku:lib.rs",
                    "tag":file_tag(&fs::read(&path).unwrap()),
                    "hunks":[
                        {"op":"delete","startLine":2,"endLine":3,"expectedLines":["two","three"]},
                        insert
                    ]
                }),
                &ctx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::InvalidArgs(_)));
        assert!(err.to_string().contains("overlaps"));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "one\ntwo\nthree\nfour\n"
        );
    }
}

#[tokio::test]
async fn fs_remove_requires_approval_and_a_fresh_tag() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lib.rs");
    fs::write(&path, "bye\n").unwrap();
    let tag = file_tag(&fs::read(&path).unwrap());
    let remove = FsRemoveFn::new(temp_linked(root.path(), FsMode::Rw));
    let denied_ctx = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(StaticApproval(ApprovalDecision::Denied)),
        Duration::from_secs(1),
    );
    let err = remove
        .call(json!({"path":"tempestmiku:lib.rs","tag":tag}), &denied_ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalDenied(_)));
    assert!(path.exists());
    let approved_ctx = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        Duration::from_secs(1),
    );
    remove
        .call(
            json!({"path":"tempestmiku:lib.rs","tag":file_tag(&fs::read(&path).unwrap())}),
            &approved_ctx,
        )
        .await
        .unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn fs_remove_rejects_content_changed_during_approval() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lib.rs");
    fs::write(&path, "old\n").unwrap();
    let tag = file_tag(&fs::read(&path).unwrap());
    let remove = FsRemoveFn::new(temp_linked(root.path(), FsMode::Rw));
    let approval = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(RewriteThenApprove {
            path: path.clone(),
            data: b"concurrent\n".to_vec(),
        }),
        Duration::from_secs(1),
    );

    let error = remove
        .call(json!({"path":"tempestmiku:lib.rs","tag":tag}), &approval)
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::InvalidArgs(_)));
    assert_eq!(fs::read_to_string(path).unwrap(), "concurrent\n");
}

#[cfg(unix)]
#[tokio::test]
async fn fs_remove_deletes_an_in_repo_symlink_without_deleting_its_target() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target.rs");
    let link = root.path().join("link.rs");
    fs::write(&target, "keep\n").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let remove = FsRemoveFn::new(temp_linked(root.path(), FsMode::Rw));
    let approved_ctx = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        Duration::from_secs(1),
    );

    remove
        .call(
            json!({
                "path": "tempestmiku:link.rs",
                "tag": file_tag(&fs::read(&link).unwrap())
            }),
            &approved_ctx,
        )
        .await
        .unwrap();

    assert!(fs::symlink_metadata(&link).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep\n");
}

#[tokio::test]
async fn fs_move_requires_approval_only_for_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.rs");
    fs::write(&source, "source\n").unwrap();
    let move_file = FsMoveFn::new(temp_linked(root.path(), FsMode::Rw));
    let source_tag = file_tag(&fs::read(&source).unwrap());
    let same_path = move_file
        .call(
            json!({"path":"tempestmiku:source.rs","dest":"tempestmiku:source.rs","tag":source_tag.clone()}),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(same_path, HostError::InvalidArgs(_)));
    call_fn(
        &move_file,
        json!({"path":"tempestmiku:source.rs","dest":"tempestmiku:moved.rs","tag":source_tag}),
        &ctx(),
    )
    .await;
    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(root.path().join("moved.rs")).unwrap(),
        "source\n"
    );

    fs::write(&source, "replacement\n").unwrap();
    let denied_ctx = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(StaticApproval(ApprovalDecision::Denied)),
        Duration::from_secs(1),
    );
    let err = move_file
        .call(
            json!({
                "path":"tempestmiku:source.rs",
                "dest":"tempestmiku:moved.rs",
                "tag":file_tag(&fs::read(&source).unwrap()),
                "overwrite":true
            }),
            &denied_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalDenied(_)));
    assert!(source.exists());

    let other = tempfile::tempdir().unwrap();
    let linked = LinkedFolders::from_configs(vec![
        LinkedFolderConfig {
            name: "tempestmiku".to_string(),
            path: root.path().to_path_buf(),
            mode: FsMode::Rw,
            commands: Vec::new(),
            safe_args: Vec::new(),
        },
        LinkedFolderConfig {
            name: "other".to_string(),
            path: other.path().to_path_buf(),
            mode: FsMode::Rw,
            commands: Vec::new(),
            safe_args: Vec::new(),
        },
    ])
    .unwrap();
    let cross_alias = FsMoveFn::new(linked)
        .call(
            json!({
                "path":"tempestmiku:source.rs",
                "dest":"other:new/path.rs",
                "tag":file_tag(&fs::read(&source).unwrap()),
                "createParents":true
            }),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(cross_alias, HostError::InvalidPath(_)));
    assert!(!other.path().join("new").exists());
}

#[tokio::test]
async fn fs_move_rejects_destination_changed_during_approval() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.rs");
    let dest = root.path().join("dest.rs");
    fs::write(&source, "source\n").unwrap();
    fs::write(&dest, "old dest\n").unwrap();
    let source_tag = file_tag(&fs::read(&source).unwrap());
    let move_file = FsMoveFn::new(temp_linked(root.path(), FsMode::Rw));
    let approval = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(RewriteThenApprove {
            path: dest.clone(),
            data: b"concurrent dest\n".to_vec(),
        }),
        Duration::from_secs(1),
    );

    let error = move_file
        .call(
            json!({
                "path":"tempestmiku:source.rs",
                "dest":"tempestmiku:dest.rs",
                "tag":source_tag,
                "overwrite":true
            }),
            &approval,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::InvalidArgs(_)));
    assert_eq!(fs::read_to_string(source).unwrap(), "source\n");
    assert_eq!(fs::read_to_string(dest).unwrap(), "concurrent dest\n");
}

#[cfg(unix)]
#[tokio::test]
async fn fs_move_moves_an_in_repo_symlink_without_moving_its_target() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target.rs");
    let link = root.path().join("link.rs");
    let moved = root.path().join("moved-link.rs");
    fs::write(&target, "keep\n").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let move_file = FsMoveFn::new(temp_linked(root.path(), FsMode::Rw));

    move_file
        .call(
            json!({
                "path": "tempestmiku:link.rs",
                "dest": "tempestmiku:moved-link.rs",
                "tag": file_tag(&fs::read(&link).unwrap())
            }),
            &ctx(),
        )
        .await
        .unwrap();

    assert!(fs::symlink_metadata(&link).is_err());
    assert!(
        fs::symlink_metadata(&moved)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep\n");
}

#[tokio::test]
async fn fs_patch_spills_large_diffs_without_failing_the_mutation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large.txt");
    fs::write(&path, "old\n").unwrap();
    let patch = FsPatchFn::new(
        temp_linked(root.path(), FsMode::Rw),
        ArtifactStore::open(root.path().join("artifacts"), "patch").unwrap(),
    );
    let value = call_fn(
        &patch,
        json!({
            "path":"tempestmiku:large.txt",
            "tag":file_tag(&fs::read(&path).unwrap()),
            "hunks":[{"op":"replace","startLine":1,"endLine":1,"expectedLines":["old"],"lines":["x".repeat(20_000)]}]
        }),
        &ctx(),
    )
    .await;
    assert_eq!(value["changed"], json!(true));
    assert_eq!(value["truncated"], json!(true));
    assert!(value["diffPreview"].as_str().unwrap().len() <= 12 * 1024 + 64);
    assert!(value["diffArtifact"]["uri"].as_str().is_some());
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        format!("{}\n", "x".repeat(20_000))
    );
}

#[tokio::test]
async fn fs_patch_preserves_crlf_and_keeps_diff_context_bounded() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("windows.txt");
    let old = (1..=2_000)
        .map(|line| format!("line {line}\r\n"))
        .collect::<String>();
    fs::write(&path, old.as_bytes()).unwrap();
    let patch = FsPatchFn::new(
        temp_linked(root.path(), FsMode::Rw),
        ArtifactStore::open(root.path().join("artifacts"), "patch-crlf").unwrap(),
    );

    let value = call_fn(
        &patch,
        json!({
            "path":"tempestmiku:windows.txt",
            "tag":file_tag(&fs::read(&path).unwrap()),
            "hunks":[{"op":"replace","startLine":1000,"endLine":1000,"expectedLines":["line 1000"],"lines":["changed"]}]
        }),
        &ctx(),
    )
    .await;

    let written = fs::read(&path).unwrap();
    assert!(written.windows(2).any(|pair| pair == b"\r\n"));
    assert!(
        !written
            .iter()
            .enumerate()
            .any(|(index, byte)| *byte == b'\n' && (index == 0 || written[index - 1] != b'\r'))
    );
    let preview = value["diffPreview"].as_str().unwrap();
    assert!(preview.contains("-line 1000"));
    assert!(preview.contains("+changed"));
    assert!(!preview.contains('\r'));
    assert!(
        preview.len() < 1_000,
        "contextual preview was {} bytes",
        preview.len()
    );
    assert_eq!(value["truncated"], json!(false));
    assert!(value["diffArtifact"].is_null());
}

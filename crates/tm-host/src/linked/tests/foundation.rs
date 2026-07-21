use super::support::*;
use super::*;

#[tokio::test]
async fn unknown_capability_fails_closed() {
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(EchoFn {
        docs: docs("echo", "test", "Echo args", false),
    }));
    let ctx = InvocationCtx::new(CapabilityGrants::default());
    let err = registry
        .invoke("echo", Value::String("x".into()), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, HostError::CapabilityDenied("echo".into()));
}

#[tokio::test]
async fn unknown_scheme_fails_closed() {
    let registry = ResourceRegistry::new();
    let ctx = InvocationCtx::new(CapabilityGrants::default());
    let err = registry.read("memory://x", None, &ctx).await.unwrap_err();
    assert!(matches!(err, HostError::UnknownScheme { .. }));
}

#[test]
fn host_config_defaults_and_bounds_proc_run_timeout() {
    let root = tempfile::tempdir().unwrap();
    let default_path = root.path().join("default.json");
    fs::write(&default_path, "{}").unwrap();
    let default = P0HostConfig::from_json_file(default_path).unwrap();
    assert_eq!(default.proc_run_timeout_ms, 180_000);
    assert_eq!(default.proc_isolation, ProcIsolationConfig::Disabled {});

    let invalid_path = root.path().join("invalid.json");
    fs::write(&invalid_path, r#"{"proc_run_timeout_ms":900001}"#).unwrap();
    let err = P0HostConfig::from_json_file(invalid_path).unwrap_err();
    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[test]
fn host_config_rejects_unknown_authority_fields_instead_of_defaulting() {
    let root = tempfile::tempdir().unwrap();
    for (name, config) in [
        ("top-level", r#"{"proc_isolaton":{"provider":"disabled"}}"#),
        (
            "linked-folder",
            r#"{"linked_folders":[{"name":"repo","path":".","mode":"ro","commands":[],"commandz":[]}]}"#,
        ),
        ("approval", r#"{"approvals":{"mod":"manual"}}"#),
        (
            "self-evolution",
            r#"{"self_evolution":{"teir":"conservative"}}"#,
        ),
    ] {
        let path = root.path().join(format!("{name}.json"));
        fs::write(&path, config).unwrap();
        let error = P0HostConfig::from_json_file(path).unwrap_err();
        assert!(
            matches!(error, HostError::InvalidArgs(_)),
            "{name} unknown field did not fail closed: {error}"
        );
        assert!(
            error.to_string().contains("unknown field"),
            "{name} error was not explicit: {error}"
        );
    }
}

#[tokio::test]
async fn p0_tool_docs_include_tm_contract_metadata() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("lib.rs"), "pub fn x() {}\n").unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(artifact_dir.path(), "docs").unwrap();
    let mut host_registry = HostRegistry::new();
    let mut resource_registry = ResourceRegistry::new();
    register_p0_linked_folder_functions(
        &mut host_registry,
        &mut resource_registry,
        temp_linked(root.path(), FsMode::Rw),
        store,
        Duration::from_millis(180_000),
    );

    let docs = host_registry.docs("fs.read", &ctx()).unwrap();
    assert_eq!(docs.signature, "@fs.read FsReadArgs -> ResourceContent");
    assert_eq!(docs.args_schema["required"], json!(["path"]));
    assert_eq!(
        docs.result_schema.as_ref().unwrap()["properties"]["content"]["type"],
        json!("string")
    );
    assert!(
        docs.examples
            .iter()
            .any(|example| example.code.contains("@fs.read"))
    );
    assert!(docs.errors.iter().any(|err| err.name == "InvalidPathError"));
    assert!(
        docs.grants
            .iter()
            .any(|grant| grant.kind == "linked-folder")
    );
    assert_eq!(docs.approval, "none");
    assert!(docs.sensitive);
    assert_eq!(docs.stability, "experimental");

    for name in ["fs.read", "fs.ls", "fs.find", "fs.grep"] {
        let docs = host_registry.docs(name, &ctx()).unwrap();
        assert!(docs.sensitive, "{name} results must be trace-sensitive");
        assert_eq!(
            docs.approval, "none",
            "{name} sensitivity must not add mutation approval"
        );
    }

    for name in [
        "fs.write",
        "fs.ls",
        "fs.find",
        "fs.move",
        "fs.grep",
        "fs.patch",
        "fs.remove",
        "proc.run",
    ] {
        let docs = host_registry.docs(name, &ctx()).unwrap();
        assert!(
            docs.signature.starts_with(&format!("@{name} ")),
            "{name} docs should expose a tm effect signature: {}",
            docs.signature
        );
        assert!(
            docs.args_schema.is_object(),
            "{name} docs should expose an args schema"
        );
        assert!(
            !docs.errors.is_empty(),
            "{name} docs should document fail-closed errors"
        );
    }
    for retired in ["code.search", "code.edit", "code.ast", "code.lsp"] {
        assert!(
            matches!(
                host_registry.docs(retired, &ctx()),
                Err(HostError::NotFound(_))
            ),
            "retired {retired} must fail closed as NotFound"
        );
    }
    let patch_docs = host_registry.docs("fs.patch", &ctx()).unwrap();
    let replace_required =
        &patch_docs.args_schema["properties"]["hunks"]["items"]["oneOf"][0]["required"];
    assert!(
        replace_required
            .as_array()
            .unwrap()
            .contains(&json!("expectedLines"))
    );
}

#[tokio::test]
async fn approval_default_denies_on_timeout() {
    let policy = DefaultDenyApprovalPolicy;
    let err = policy
        .request("write-prod", Duration::from_millis(1))
        .await
        .unwrap_err();
    assert_eq!(err, HostError::ApprovalTimeout("write-prod".into()));
}

#[tokio::test]
async fn artifact_handler_resolves_through_registry() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(dir.path(), "default").unwrap();
    let artifact = store.put_text("hello", None, "text/plain").unwrap();
    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(ArtifactResourceHandler::new(store)));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:artifact"));
    let content = registry.read(&artifact.uri, None, &ctx).await.unwrap();
    assert_eq!(content.content, "hello");
}

#[tokio::test]
async fn linked_path_rejects_traversal_and_symlink_escape() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(root.path().join("inside.txt"), "inside").unwrap();
    fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    let linked = temp_linked(root.path(), FsMode::Rw);
    assert!(matches!(
        linked.read_resource("tempestmiku:../secret.txt", None),
        Err(HostError::InvalidPath(_))
    ));
    #[cfg(unix)]
    {
        let err = linked
            .read_resource("tempestmiku:escape/secret.txt", None)
            .unwrap_err();
        assert!(matches!(err, HostError::InvalidPath(_)));
    }
}

#[cfg(unix)]
#[test]
fn linked_root_identity_is_pinned_against_real_directory_replacement() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("linked-root");
    let parked = outer.path().join("parked-root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("note.txt"), "original").unwrap();
    let linked = temp_linked(&root, FsMode::Ro);

    fs::rename(&root, &parked).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("note.txt"), "replacement").unwrap();
    let error = linked
        .read_resource("tempestmiku:note.txt", None)
        .unwrap_err();
    assert!(matches!(error, HostError::InvalidPath(_)));
    assert!(error.to_string().contains("root identity changed"));

    fs::remove_dir_all(&root).unwrap();
    fs::rename(&parked, &root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn fs_write_rejects_dangling_and_intermediate_outward_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("created.txt");
    std::os::unix::fs::symlink(&outside_file, root.path().join("dangling.txt")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    let write = FsWriteFn::new(temp_linked(root.path(), FsMode::Rw));

    for path in ["tempestmiku:dangling.txt", "tempestmiku:escape/created.txt"] {
        let error = write
            .call(json!({"path":path,"data":"outside"}), &approved_ctx())
            .await
            .unwrap_err();
        assert!(matches!(error, HostError::InvalidPath(_)));
    }
    assert!(!outside_file.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn recursive_listing_and_search_do_not_follow_outward_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.txt"), "outside needle\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape-dir")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        root.path().join("escape-file.txt"),
    )
    .unwrap();
    let linked = temp_linked(root.path(), FsMode::Ro);

    let listed = call_fn(
        &FsLsFn::new(linked.clone()),
        json!({"path":"tempestmiku:","recursive":true}),
        &ctx(),
    )
    .await;
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|entry| entry["kind"] == json!("symlink")));
    assert!(
        !listed
            .iter()
            .any(|entry| entry["path"] == json!("tempestmiku:escape-dir/secret.txt"))
    );

    let hits = call_fn(
        &FsGrepFn::new(linked),
        json!({"pattern":"needle","paths":["tempestmiku:"],"regex":false}),
        &ctx(),
    )
    .await;
    assert!(hits.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn linked_path_vanished_fails_closed_without_host_path() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("vanished.txt");
    fs::write(&file, "temporary").unwrap();
    let linked = temp_linked(root.path(), FsMode::Ro);
    fs::remove_file(&file).unwrap();

    let err = linked
        .read_resource("linked://tempestmiku/vanished.txt", None)
        .unwrap_err();
    assert!(matches!(err, HostError::InvalidPath(_)));
    let error = err.to_string();
    assert!(error.contains("tempestmiku:vanished.txt"));
    assert!(!error.contains(root.path().to_str().unwrap()));
}

#[test]
fn linked_text_reads_are_paged_before_loading_the_whole_file() {
    let root = tempfile::tempdir().unwrap();
    let content = (0..400)
        .map(|line| format!("{line:04} {}\n", "x".repeat(512)))
        .collect::<String>();
    fs::write(root.path().join("large.txt"), &content).unwrap();
    let linked = temp_linked(root.path(), FsMode::Ro);

    let page = linked.read_resource("tempestmiku:large.txt", None).unwrap();
    assert!(page.content.len() <= 64 * 1024);
    assert!(page.has_more);
    assert_eq!(page.size_bytes, content.len());

    let error = linked
        .read_resource("tempestmiku:large.txt", Some("1-1001"))
        .unwrap_err();
    assert!(matches!(error, HostError::InvalidArgs(_)));
}

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tm_artifacts::ArtifactStore;

use crate::{
    ApprovalDecision, ApprovalPolicy, ArtifactResourceHandler, CapabilityGrants,
    DefaultDenyApprovalPolicy, HostError, HostFn, HostRegistry, InvocationCtx, ResourceRegistry,
    Result, ToolDocs,
};

use super::*;
use super::{
    docs::docs,
    secure_fs::MAX_SECURE_WALK_DEPTH,
    tools::{
        CodeSearchFn, FsFindFn, FsLsFn, FsMoveFn, FsPatchFn, FsReadFn, FsRemoveFn, FsWriteFn,
        ProcRunFn,
    },
    util::{
        MAX_FS_RESULT_BYTES, MAX_GLOB_PATTERN_BYTES, MAX_GLOB_PATTERNS, MAX_GLOB_TOTAL_BYTES,
        MAX_MUTATION_FILE_BYTES, file_tag, load_simple_gitignore, push_bounded_fs_entry,
    },
};

struct EchoFn {
    docs: ToolDocs,
}

#[async_trait]
impl HostFn for EchoFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, _ctx: &InvocationCtx) -> Result<Value> {
        Ok(args)
    }
}

#[derive(Debug)]
struct StaticApproval(ApprovalDecision);

#[async_trait]
impl ApprovalPolicy for StaticApproval {
    async fn request(&self, _action: &str, _timeout: Duration) -> Result<ApprovalDecision> {
        Ok(self.0)
    }
}

#[derive(Debug)]
struct RewriteThenApprove {
    path: PathBuf,
    data: Vec<u8>,
}

#[derive(Debug)]
struct NarrowPolicyThenApprove {
    linked: LinkedFolders,
    root: PathBuf,
}

#[async_trait]
impl ApprovalPolicy for NarrowPolicyThenApprove {
    async fn request(&self, _action: &str, _timeout: Duration) -> Result<ApprovalDecision> {
        self.linked.insert_policy(FsPolicy {
            alias: "tempestmiku".to_string(),
            root: self.root.clone(),
            mode: FsMode::Ro,
            commands: std::collections::BTreeSet::new(),
            safe_args: Vec::new(),
        })?;
        Ok(ApprovalDecision::Approved)
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct SwapDirectoryThenApprove {
    path: PathBuf,
    parked: PathBuf,
    replacement: PathBuf,
}

#[cfg(unix)]
#[async_trait]
impl ApprovalPolicy for SwapDirectoryThenApprove {
    async fn request(&self, _action: &str, _timeout: Duration) -> Result<ApprovalDecision> {
        fs::rename(&self.path, &self.parked).map_err(|err| HostError::HostCall(err.to_string()))?;
        std::os::unix::fs::symlink(&self.replacement, &self.path)
            .map_err(|err| HostError::HostCall(err.to_string()))?;
        Ok(ApprovalDecision::Approved)
    }
}

#[derive(Debug, Default)]
struct CaptureThenApprove {
    actions: Mutex<Vec<String>>,
}

#[async_trait]
impl ApprovalPolicy for CaptureThenApprove {
    async fn request(&self, action: &str, _timeout: Duration) -> Result<ApprovalDecision> {
        self.actions
            .lock()
            .map_err(|err| HostError::HostCall(err.to_string()))?
            .push(action.to_string());
        Ok(ApprovalDecision::Approved)
    }
}

#[async_trait]
impl ApprovalPolicy for RewriteThenApprove {
    async fn request(&self, _action: &str, _timeout: Duration) -> Result<ApprovalDecision> {
        fs::write(&self.path, &self.data).map_err(|err| HostError::HostCall(err.to_string()))?;
        Ok(ApprovalDecision::Approved)
    }
}

fn temp_linked(root: &Path, mode: FsMode) -> LinkedFolders {
    temp_linked_with_commands(
        root,
        mode,
        vec!["cargo".to_string()],
        vec![vec!["cargo".to_string(), "test".to_string()]],
    )
}

fn temp_linked_with_commands(
    root: &Path,
    mode: FsMode,
    commands: Vec<String>,
    safe_args: Vec<Vec<String>>,
) -> LinkedFolders {
    LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: root.to_path_buf(),
        mode,
        commands,
        safe_args,
    }])
    .unwrap()
}

fn ctx() -> InvocationCtx {
    InvocationCtx::new(CapabilityGrants::default().allow_many([
        "fs.read",
        "fs.write",
        "fs.ls",
        "fs.find",
        "fs.move",
        "code.search",
        "fs.patch",
        "fs.remove",
        "proc.run",
        "resources.read:artifact",
        "resources.read:linked",
    ]))
}

fn approved_ctx() -> InvocationCtx {
    InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        Duration::from_secs(1),
    )
}

async fn call_fn(function: &dyn HostFn, args: Value, ctx: &InvocationCtx) -> Value {
    function.call(args, ctx).await.unwrap()
}

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

    let invalid_path = root.path().join("invalid.json");
    fs::write(&invalid_path, r#"{"proc_run_timeout_ms":900001}"#).unwrap();
    let err = P0HostConfig::from_json_file(invalid_path).unwrap_err();
    assert!(matches!(err, HostError::InvalidArgs(_)));
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

    for name in ["fs.read", "fs.ls", "fs.find", "code.search"] {
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
        "code.search",
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
    assert!(matches!(
        host_registry.docs("code.edit", &ctx()),
        Err(HostError::NotFound(_))
    ));
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
        &CodeSearchFn::new(linked),
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
async fn code_search_returns_tag_and_context() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("lib.rs"), "before\nneedle here\nafter\n").unwrap();
    let search = CodeSearchFn::new(temp_linked(root.path(), FsMode::Rw));
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
    let search = CodeSearchFn::new(linked);

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

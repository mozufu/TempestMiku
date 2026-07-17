use std::{fs, path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::{Value, json};
use tm_artifacts::ArtifactStore;

use crate::{
    ApprovalDecision, ApprovalPolicy, ArtifactResourceHandler, CapabilityGrants,
    DefaultDenyApprovalPolicy, HostError, HostFn, HostRegistry, InvocationCtx, ResourceRegistry,
    Result, ToolDocs,
};

use super::*;
use super::{
    docs::docs,
    tools::{
        CodeSearchFn, FsFindFn, FsLsFn, FsMoveFn, FsPatchFn, FsReadFn, FsRemoveFn, FsWriteFn,
        ProcRunFn,
    },
    util::file_tag,
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
    assert_eq!(docs.stability, "experimental");

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
async fn proc_run_allows_safe_prefix_approval_gates_unsafe_and_spills() {
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
    let value = call_fn(
        &proc_run,
        json!({"cmd":"cargo","args":["test"],"cwd":"tempestmiku:"}),
        &ctx(),
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
    let unknown = proc_run
        .call(
            json!({"cmd":"rm","args":["-rf","."],"cwd":"tempestmiku:"}),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(matches!(unknown, HostError::CapabilityDenied(_)));
    let spill = call_fn(
            &proc_run,
            json!({"cmd":"cargo","args":["test","--","--nocapture"],"cwd":"tempestmiku:","outputBytes":1000}),
            &ctx(),
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
        &ctx(),
    )
    .await;
    assert_eq!(echoed["exitCode"], json!(0));
    assert_eq!(echoed["stdout"], json!("hello, 世界"));

    let empty = call_fn(
        &proc_run,
        json!({"cmd":"cat","cwd":"tempestmiku:","stdin":""}),
        &ctx(),
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
        &ctx(),
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
        &ctx(),
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
        &ctx(),
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
        &ctx(),
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
    let invocation = ctx();

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

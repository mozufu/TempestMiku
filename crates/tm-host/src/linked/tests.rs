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
    tools::{CodeEditFn, CodeSearchFn, FsFindFn, FsLsFn, FsReadFn, FsWriteFn, ProcRunFn},
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
    LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: root.to_path_buf(),
        mode,
        commands: vec!["cargo".to_string()],
        safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
    }])
    .unwrap()
}

fn ctx() -> InvocationCtx {
    InvocationCtx::new(CapabilityGrants::default().allow_many([
        "fs.read",
        "fs.write",
        "fs.ls",
        "fs.find",
        "code.search",
        "code.edit",
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

#[tokio::test]
async fn p0_tool_docs_include_sdk_contract_metadata() {
    let sdk_types = include_str!("../../../../docs/sdk/tm-runtime.d.ts");
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
    );

    let docs = host_registry.docs("fs.read", &ctx()).unwrap();
    assert!(
        sdk_types.contains(&docs.signature),
        "docs/sdk/tm-runtime.d.ts is missing {}",
        docs.signature
    );
    assert_eq!(
        docs.signature,
        "fs.read(path: SdkPath, opts?: FsReadOptions): Promise<ResourceContent>"
    );
    assert_eq!(docs.args_schema["required"], json!(["path"]));
    assert_eq!(
        docs.result_schema.as_ref().unwrap()["properties"]["content"]["type"],
        json!("string")
    );
    assert!(
        docs.examples
            .iter()
            .any(|example| example.code.contains("fs.read"))
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
        "code.search",
        "code.edit",
        "proc.run",
    ] {
        let docs = host_registry.docs(name, &ctx()).unwrap();
        assert!(
            sdk_types.contains(&docs.signature),
            "docs/sdk/tm-runtime.d.ts is missing {}",
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
async fn code_edit_applies_json_hunks_and_rejects_stale_tags() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lib.rs");
    fs::write(&path, "one\ntwo\nthree\n").unwrap();
    let tag = file_tag(&fs::read(&path).unwrap());
    let edit = CodeEditFn::new(temp_linked(root.path(), FsMode::Rw));
    let value = call_fn(
        &edit,
        json!({
            "path":"tempestmiku:lib.rs",
            "tag":tag,
            "hunks":[
                {"op":"replace","startLine":2,"endLine":2,"lines":["TWO"]},
                {"op":"insert","at":"head","lines":["zero"]},
                {"op":"delete","startLine":3,"endLine":3}
            ]
        }),
        &ctx(),
    )
    .await;
    assert_ne!(value["newTag"].as_str().unwrap(), tag);
    assert_eq!(fs::read_to_string(&path).unwrap(), "zero\none\nTWO\n");
    let err = edit
            .call(
                json!({"path":"tempestmiku:lib.rs","tag":"deadbeefdeadbeef","hunks":[{"op":"insert","at":"tail","lines":["x"]}]}),
                &ctx(),
            )
            .await
            .unwrap_err();
    assert!(matches!(err, HostError::InvalidArgs(_)));
    assert_eq!(fs::read_to_string(&path).unwrap(), "zero\none\nTWO\n");
}

#[tokio::test]
async fn code_edit_remove_requires_approval() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lib.rs");
    fs::write(&path, "bye\n").unwrap();
    let tag = file_tag(&fs::read(&path).unwrap());
    let edit = CodeEditFn::new(temp_linked(root.path(), FsMode::Rw));
    let denied_ctx = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(StaticApproval(ApprovalDecision::Denied)),
        Duration::from_secs(1),
    );
    let err = edit
        .call(
            json!({"path":"tempestmiku:lib.rs","tag":tag,"hunks":[{"op":"remove"}]}),
            &denied_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::ApprovalDenied(_)));
    assert!(path.exists());
    let approved_ctx = InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        Duration::from_secs(1),
    );
    edit.call(
            json!({"path":"tempestmiku:lib.rs","tag":file_tag(&fs::read(&path).unwrap()),"hunks":[{"op":"remove"}]}),
            &approved_ctx,
        )
        .await
        .unwrap();
    assert!(!path.exists());
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
        "#[test]\nfn prints() { println!(\"{}\", \"x\".repeat(60000)); }\n",
    )
    .unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(artifact_dir.path(), "proc").unwrap();
    let proc_run = ProcRunFn::new(temp_linked(root.path(), FsMode::Rw), store);
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
}

use super::*;

pub(super) struct EchoFn {
    pub(super) docs: ToolDocs,
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
pub(super) struct StaticApproval(pub(super) ApprovalDecision);

#[async_trait]
impl ApprovalPolicy for StaticApproval {
    async fn request(&self, _action: &str, _timeout: Duration) -> Result<ApprovalDecision> {
        Ok(self.0)
    }
}

#[derive(Debug)]
pub(super) struct RewriteThenApprove {
    pub(super) path: PathBuf,
    pub(super) data: Vec<u8>,
}

#[derive(Debug)]
pub(super) struct NarrowPolicyThenApprove {
    pub(super) linked: LinkedFolders,
    pub(super) root: PathBuf,
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
pub(super) struct SwapDirectoryThenApprove {
    pub(super) path: PathBuf,
    pub(super) parked: PathBuf,
    pub(super) replacement: PathBuf,
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
pub(super) struct CaptureThenApprove {
    pub(super) actions: Mutex<Vec<String>>,
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

pub(super) fn temp_linked(root: &Path, mode: FsMode) -> LinkedFolders {
    temp_linked_with_commands(
        root,
        mode,
        vec!["cargo".to_string()],
        vec![vec!["cargo".to_string(), "test".to_string()]],
    )
}

pub(super) fn temp_linked_with_commands(
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

pub(super) fn ctx() -> InvocationCtx {
    InvocationCtx::new(CapabilityGrants::default().allow_many([
        "fs.read",
        "fs.write",
        "fs.ls",
        "fs.find",
        "fs.move",
        "fs.grep",
        "fs.patch",
        "fs.remove",
        "proc.run",
        "resources.read:artifact",
        "resources.read:linked",
    ]))
}

pub(super) fn approved_ctx() -> InvocationCtx {
    InvocationCtx::with_approvals(
        ctx().grants,
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
        Duration::from_secs(1),
    )
}

pub(super) async fn call_fn(function: &dyn HostFn, args: Value, ctx: &InvocationCtx) -> Value {
    function.call(args, ctx).await.unwrap()
}

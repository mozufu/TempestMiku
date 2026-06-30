//! Host capability, resource, and approval foundations.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs, io,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tm_artifacts::{ArtifactRef, ArtifactStore, ResourceContent, preview};
use url::Url;

pub type Result<T, E = HostError> = std::result::Result<T, E>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HostError {
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
    #[error("approval denied: {0}")]
    ApprovalDenied(String),
    #[error("approval timed out: {0}")]
    ApprovalTimeout(String),
    #[error("unknown resource scheme: {scheme}; registered: {registered:?}")]
    UnknownScheme {
        scheme: String,
        registered: Vec<String>,
    },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid args: {0}")]
    InvalidArgs(String),
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("output truncated: {0}")]
    OutputTruncated(String),
    #[error("host call error: {0}")]
    HostCall(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostErrorPayload {
    pub name: String,
    pub message: String,
    pub capability: Option<String>,
    pub path: Option<String>,
    pub uri: Option<String>,
    pub retryable: bool,
    pub details: Value,
}

impl HostError {
    pub fn sdk_name(&self) -> &'static str {
        match self {
            Self::CapabilityDenied(_) | Self::UnknownScheme { .. } => "CapabilityDeniedError",
            Self::ApprovalDenied(_) => "ApprovalDeniedError",
            Self::ApprovalTimeout(_) => "ApprovalTimeoutError",
            Self::NotFound(_) => "NotFoundError",
            Self::InvalidArgs(_) => "InvalidArgsError",
            Self::InvalidPath(_) => "InvalidPathError",
            Self::NotImplemented(_) => "NotImplementedError",
            Self::QuotaExceeded(_) => "QuotaExceededError",
            Self::Timeout(_) => "TimeoutError",
            Self::OutputTruncated(_) => "OutputTruncatedError",
            Self::HostCall(_) => "HostCallError",
        }
    }

    pub fn to_payload(&self) -> HostErrorPayload {
        let (capability, path, uri, retryable, details) = match self {
            Self::CapabilityDenied(capability) => (
                Some(capability.clone()),
                None,
                None,
                false,
                json!({ "capability": capability }),
            ),
            Self::ApprovalDenied(action) => (None, None, None, false, json!({ "action": action })),
            Self::ApprovalTimeout(action) => (None, None, None, true, json!({ "action": action })),
            Self::UnknownScheme { scheme, registered } => (
                None,
                None,
                Some(format!("{scheme}://")),
                false,
                json!({ "scheme": scheme, "registered": registered }),
            ),
            Self::NotFound(target) => (None, None, None, false, json!({ "target": target })),
            Self::InvalidArgs(message) => (None, None, None, false, json!({ "reason": message })),
            Self::InvalidPath(path) => (
                None,
                Some(path.clone()),
                None,
                false,
                json!({ "path": path }),
            ),
            Self::NotImplemented(feature) => {
                (None, None, None, false, json!({ "feature": feature }))
            }
            Self::QuotaExceeded(quota) => (None, None, None, true, json!({ "quota": quota })),
            Self::Timeout(operation) => (None, None, None, true, json!({ "operation": operation })),
            Self::OutputTruncated(target) => (None, None, None, false, json!({ "target": target })),
            Self::HostCall(message) => (None, None, None, false, json!({ "reason": message })),
        };
        HostErrorPayload {
            name: self.sdk_name().to_string(),
            message: self.to_string(),
            capability,
            path,
            uri,
            retryable,
            details,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSummary {
    pub name: String,
    pub namespace: String,
    pub summary: String,
    pub sensitive: bool,
    pub granted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDocs {
    pub name: String,
    pub namespace: String,
    pub summary: String,
    pub description: Option<String>,
    pub signature: String,
    pub args_schema: Value,
    pub result_schema: Option<Value>,
    pub examples: Vec<ToolExample>,
    pub errors: Vec<ToolErrorDoc>,
    pub grants: Vec<GrantDoc>,
    pub sensitive: bool,
    pub approval: String,
    pub since: String,
    pub stability: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExample {
    pub title: Option<String>,
    pub code: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolErrorDoc {
    pub name: String,
    pub when: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantDoc {
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct CapabilityGrants {
    allowed: BTreeSet<String>,
}

impl CapabilityGrants {
    pub fn allow(mut self, name: impl Into<String>) -> Self {
        self.allowed.insert(name.into());
        self
    }

    pub fn allow_many(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        for name in names {
            self.allowed.insert(name.into());
        }
        self
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.allowed.iter().map(String::as_str)
    }

    pub fn permits(&self, name: &str) -> bool {
        self.allowed.contains(name)
    }
}

#[derive(Clone)]
pub struct InvocationCtx {
    pub grants: CapabilityGrants,
    pub approvals: Arc<dyn ApprovalPolicy>,
    pub approval_timeout: Duration,
}

impl std::fmt::Debug for InvocationCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvocationCtx")
            .field("grants", &self.grants)
            .field("approval_timeout", &self.approval_timeout)
            .finish_non_exhaustive()
    }
}

impl InvocationCtx {
    pub fn new(grants: CapabilityGrants) -> Self {
        Self::with_approvals(
            grants,
            Arc::new(DefaultDenyApprovalPolicy),
            Duration::from_secs(60),
        )
    }

    pub fn with_approvals(
        grants: CapabilityGrants,
        approvals: Arc<dyn ApprovalPolicy>,
        approval_timeout: Duration,
    ) -> Self {
        Self {
            grants,
            approvals,
            approval_timeout,
        }
    }

    pub async fn require_approval(&self, action: &str) -> Result<()> {
        match self
            .approvals
            .request(action, self.approval_timeout)
            .await?
        {
            ApprovalDecision::Approved => Ok(()),
            ApprovalDecision::Denied => Err(HostError::ApprovalDenied(action.to_string())),
        }
    }
}

#[async_trait]
pub trait HostFn: Send + Sync {
    fn docs(&self) -> &ToolDocs;

    fn name(&self) -> &str {
        &self.docs().name
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value>;
}

#[derive(Default, Clone)]
pub struct HostRegistry {
    functions: BTreeMap<String, Arc<dyn HostFn>>,
}

impl HostRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, function: Arc<dyn HostFn>) {
        self.functions.insert(function.name().to_string(), function);
    }

    pub fn search(
        &self,
        query: &str,
        namespace: Option<&str>,
        limit: usize,
        ctx: &InvocationCtx,
    ) -> Vec<ToolSummary> {
        let needle = query.to_lowercase();
        let limit = limit.max(1);
        self.functions
            .values()
            .filter_map(|function| {
                let docs = function.docs();
                if let Some(namespace) = namespace
                    && docs.namespace != namespace
                {
                    return None;
                }
                let haystack =
                    format!("{} {} {}", docs.name, docs.namespace, docs.summary).to_lowercase();
                (needle.is_empty() || haystack.contains(&needle)).then(|| ToolSummary {
                    name: docs.name.clone(),
                    namespace: docs.namespace.clone(),
                    summary: docs.summary.clone(),
                    sensitive: docs.sensitive,
                    granted: ctx.grants.permits(&docs.name),
                })
            })
            .take(limit)
            .collect()
    }

    pub fn docs(&self, name: &str, _ctx: &InvocationCtx) -> Result<ToolDocs> {
        self.functions
            .get(name)
            .map(|function| function.docs().clone())
            .ok_or_else(|| HostError::NotFound(format!("tool {name}")))
    }

    pub async fn invoke(&self, name: &str, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        if !ctx.grants.permits(name) {
            return Err(HostError::CapabilityDenied(name.to_string()));
        }
        let function = self
            .functions
            .get(name)
            .ok_or_else(|| HostError::CapabilityDenied(name.to_string()))?;
        function.call(args, ctx).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

#[async_trait]
pub trait ApprovalPolicy: Send + Sync {
    async fn request(&self, action: &str, timeout: Duration) -> Result<ApprovalDecision>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultDenyApprovalPolicy;

#[async_trait]
impl ApprovalPolicy for DefaultDenyApprovalPolicy {
    async fn request(&self, action: &str, _timeout: Duration) -> Result<ApprovalDecision> {
        Err(HostError::ApprovalTimeout(action.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceEntry {
    pub uri: String,
    pub name: String,
    pub kind: String,
    pub title: Option<String>,
    pub size_bytes: Option<usize>,
    pub modified_at: Option<String>,
}

#[async_trait]
pub trait ResourceHandler: Send + Sync {
    fn scheme(&self) -> &str;
    fn capability(&self) -> &str;
    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> Result<ResourceContent>;

    async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> Result<ResourceContent> {
        let mut content = self.read(uri, None, ctx).await?;
        content.content.clear();
        Ok(content)
    }

    async fn list(&self, uri: Option<&str>, _ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        Err(HostError::NotFound(format!(
            "resource list unsupported for {} {}",
            self.scheme(),
            uri.unwrap_or("")
        )))
    }
}

#[derive(Default, Clone)]
pub struct ResourceRegistry {
    handlers: BTreeMap<String, Arc<dyn ResourceHandler>>,
}

impl ResourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, handler: Arc<dyn ResourceHandler>) {
        self.handlers.insert(handler.scheme().to_string(), handler);
    }

    pub fn schemes(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    pub async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> Result<ResourceContent> {
        let handler = self.handler_for(uri, ctx)?;
        handler.read(uri, selector, ctx).await
    }

    pub async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> Result<ResourceContent> {
        let handler = self.handler_for(uri, ctx)?;
        handler.preview(uri, ctx).await
    }

    pub async fn list(&self, uri: Option<&str>, ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        let Some(uri) = uri.filter(|uri| !uri.is_empty()) else {
            return Ok(self
                .handlers
                .keys()
                .map(|scheme| ResourceEntry {
                    uri: format!("{scheme}://"),
                    name: scheme.clone(),
                    kind: "scheme".to_string(),
                    title: None,
                    size_bytes: None,
                    modified_at: None,
                })
                .collect());
        };
        let handler = self.handler_for(uri, ctx)?;
        handler.list(Some(uri), ctx).await
    }

    fn handler_for(&self, uri: &str, ctx: &InvocationCtx) -> Result<Arc<dyn ResourceHandler>> {
        let scheme = parse_scheme(uri)?;
        let handler = self
            .handlers
            .get(&scheme)
            .ok_or_else(|| HostError::UnknownScheme {
                scheme: scheme.clone(),
                registered: self.schemes(),
            })?;
        if !ctx.grants.permits(handler.capability()) {
            return Err(HostError::CapabilityDenied(
                handler.capability().to_string(),
            ));
        }
        Ok(Arc::clone(handler))
    }
}

pub struct ArtifactResourceHandler {
    store: ArtifactStore,
}

impl ArtifactResourceHandler {
    pub fn new(store: ArtifactStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ResourceHandler for ArtifactResourceHandler {
    fn scheme(&self) -> &str {
        "artifact"
    }

    fn capability(&self) -> &str {
        "resources.read:artifact"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> Result<ResourceContent> {
        self.store
            .read(uri, selector)
            .map_err(|err| HostError::NotFound(err.to_string()))
    }

    async fn list(&self, _uri: Option<&str>, _ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        Ok(self
            .store
            .list()
            .into_iter()
            .map(|artifact| ResourceEntry {
                uri: artifact.uri,
                name: artifact.id,
                kind: artifact.kind,
                title: artifact.title,
                size_bytes: Some(artifact.size_bytes),
                modified_at: None,
            })
            .collect())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct P0HostConfig {
    #[serde(default)]
    pub linked_folders: Vec<LinkedFolderConfig>,
    #[serde(default)]
    pub approvals: ApprovalConfig,
    #[serde(default)]
    pub artifact_root: Option<PathBuf>,
}

impl P0HostConfig {
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content =
            fs::read_to_string(path).map_err(|err| HostError::HostCall(err.to_string()))?;
        let config: Self = serde_json::from_str(&content)
            .map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        LinkedFolders::from_configs(self.linked_folders.clone()).map(|_| ())
    }

    pub fn linked_folders(&self) -> Result<LinkedFolders> {
        LinkedFolders::from_configs(self.linked_folders.clone())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LinkedFolderConfig {
    pub name: String,
    pub path: PathBuf,
    pub mode: FsMode,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub safe_args: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsMode {
    Ro,
    Rw,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ApprovalConfig {
    #[serde(default = "default_approval_mode")]
    pub mode: String,
    #[serde(default = "default_approval_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            mode: default_approval_mode(),
            timeout_ms: default_approval_timeout_ms(),
        }
    }
}

pub fn default_approval_mode() -> String {
    "deny".to_string()
}

pub fn default_approval_timeout_ms() -> u64 {
    60_000
}

#[derive(Debug, Clone)]
pub struct FsPolicy {
    pub alias: String,
    pub root: PathBuf,
    pub mode: FsMode,
    pub commands: BTreeSet<String>,
    pub safe_args: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct LinkedFolders {
    policies: BTreeMap<String, FsPolicy>,
}

impl LinkedFolders {
    pub fn from_configs(configs: Vec<LinkedFolderConfig>) -> Result<Self> {
        let mut policies = BTreeMap::new();
        for config in configs {
            validate_alias(&config.name)?;
            if policies.contains_key(&config.name) {
                return Err(HostError::InvalidArgs(format!(
                    "duplicate linked folder alias {}",
                    config.name
                )));
            }
            let root = config.path.canonicalize().map_err(|err| {
                HostError::InvalidPath(format!("{}: {err}", config.path.display()))
            })?;
            if !root.is_dir() {
                return Err(HostError::InvalidPath(format!(
                    "linked folder {} is not a directory",
                    root.display()
                )));
            }
            let mut commands = BTreeSet::new();
            for command in config.commands {
                validate_command_name(&command)?;
                commands.insert(command);
            }
            for argv in &config.safe_args {
                let Some(command) = argv.first() else {
                    return Err(HostError::InvalidArgs(
                        "safe_args entries must be non-empty".into(),
                    ));
                };
                if !commands.contains(command) {
                    return Err(HostError::InvalidArgs(format!(
                        "safe_args command {command} is not in commands"
                    )));
                }
            }
            policies.insert(
                config.name.clone(),
                FsPolicy {
                    alias: config.name,
                    root,
                    mode: config.mode,
                    commands,
                    safe_args: config.safe_args,
                },
            );
        }
        Ok(Self { policies })
    }

    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }

    pub fn first_alias(&self) -> Option<&str> {
        self.policies.keys().next().map(String::as_str)
    }

    pub fn policies(&self) -> impl Iterator<Item = &FsPolicy> {
        self.policies.values()
    }

    pub fn policy(&self, alias: &str) -> Result<&FsPolicy> {
        self.policies
            .get(alias)
            .ok_or_else(|| HostError::InvalidPath(format!("unknown linked folder alias {alias}")))
    }

    fn resolve_existing(&self, input: Option<&str>) -> Result<ResolvedPath> {
        let parsed = self.parse_path(input)?;
        let policy = self.policy(&parsed.alias)?.clone();
        let joined = policy.root.join(&parsed.relative);
        let path = joined
            .canonicalize()
            .map_err(|err| HostError::InvalidPath(format!("{}: {err}", parsed.display)))?;
        ensure_under_root(&path, &policy.root, &parsed.display)?;
        Ok(ResolvedPath {
            alias: parsed.alias,
            relative: parsed.relative,
            display: parsed.display,
            path,
            policy,
        })
    }

    fn resolve_for_create(&self, input: &str, create_parents: bool) -> Result<ResolvedPath> {
        let parsed = self.parse_path(Some(input))?;
        let policy = self.policy(&parsed.alias)?.clone();
        let target = policy.root.join(&parsed.relative);
        if target.exists() {
            let path = target
                .canonicalize()
                .map_err(|err| HostError::InvalidPath(format!("{}: {err}", parsed.display)))?;
            ensure_under_root(&path, &policy.root, &parsed.display)?;
            return Ok(ResolvedPath {
                alias: parsed.alias,
                relative: parsed.relative,
                display: parsed.display,
                path,
                policy,
            });
        }
        let parent = target.parent().ok_or_else(|| {
            HostError::InvalidPath(format!("missing parent for {}", parsed.display))
        })?;
        ensure_existing_ancestor_under_root(parent, &policy.root, &parsed.display)?;
        if !parent.exists() {
            if !create_parents {
                return Err(HostError::InvalidPath(format!(
                    "parent does not exist for {}",
                    parsed.display
                )));
            }
            fs::create_dir_all(parent).map_err(|err| HostError::HostCall(err.to_string()))?;
        }
        let canonical_parent = parent
            .canonicalize()
            .map_err(|err| HostError::InvalidPath(format!("{}: {err}", parsed.display)))?;
        ensure_under_root(&canonical_parent, &policy.root, &parsed.display)?;
        Ok(ResolvedPath {
            alias: parsed.alias,
            relative: parsed.relative,
            display: parsed.display,
            path: target,
            policy,
        })
    }

    fn parse_path(&self, input: Option<&str>) -> Result<ParsedPath> {
        let input = match input {
            Some(input) => input,
            None => self
                .first_alias()
                .map(|alias| format!("{alias}:"))
                .ok_or_else(|| HostError::InvalidPath("no linked folders configured".to_string()))?
                .leak(),
        };
        parse_linked_path(input)
    }

    fn read_resource(&self, path: &str, selector: Option<&str>) -> Result<ResourceContent> {
        let resolved = self.resolve_existing(Some(path))?;
        let bytes = fs::read(&resolved.path).map_err(|err| HostError::HostCall(err.to_string()))?;
        let content = String::from_utf8(bytes).map_err(|_| {
            HostError::InvalidArgs("fs.read supports UTF-8 text only in P0".to_string())
        })?;
        let size_bytes = content.len();
        let (selected, has_more) = select_text(&content, selector)?;
        Ok(ResourceContent {
            uri: linked_uri(&resolved.alias, &resolved.relative),
            kind: "text".to_string(),
            mime: "text/plain".to_string(),
            title: resolved
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string),
            size_bytes,
            selector: selector.map(str::to_string),
            has_more,
            preview: preview(&selected, 1024),
            content: selected,
        })
    }
}

#[derive(Debug, Clone)]
struct ParsedPath {
    alias: String,
    relative: PathBuf,
    display: String,
}

#[derive(Debug, Clone)]
struct ResolvedPath {
    alias: String,
    relative: PathBuf,
    display: String,
    path: PathBuf,
    policy: FsPolicy,
}

pub fn register_p0_linked_folder_functions(
    host_registry: &mut HostRegistry,
    resource_registry: &mut ResourceRegistry,
    linked_folders: LinkedFolders,
    artifact_store: ArtifactStore,
) {
    host_registry.register(Arc::new(FsReadFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(FsWriteFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(FsLsFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(FsFindFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(CodeSearchFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(CodeEditFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(ProcRunFn::new(
        linked_folders.clone(),
        artifact_store,
    )));
    resource_registry.register(Arc::new(LinkedResourceHandler::new(linked_folders)));
}

struct FsReadFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsReadFn {
    fn new(linked: LinkedFolders) -> Self {
        Self {
            linked,
            docs: docs(
                "fs.read",
                "fs",
                "Read UTF-8 text from a linked folder",
                false,
            ),
        }
    }
}

#[async_trait]
impl HostFn for FsReadFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, _ctx: &InvocationCtx) -> Result<Value> {
        #[derive(Deserialize)]
        struct Args {
            path: String,
            selector: Option<String>,
            #[allow(dead_code)]
            raw: Option<bool>,
        }
        let args: Args = parse_args(args)?;
        serde_json::to_value(
            self.linked
                .read_resource(&args.path, args.selector.as_deref())?,
        )
        .map_err(|err| HostError::HostCall(err.to_string()))
    }
}

struct FsWriteFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsWriteFn {
    fn new(linked: LinkedFolders) -> Self {
        Self {
            linked,
            docs: docs(
                "fs.write",
                "fs",
                "Write UTF-8 text under a writable linked folder",
                true,
            ),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WriteResult {
    path: String,
    uri: String,
    bytes_written: usize,
    created: bool,
    overwritten: bool,
}

#[async_trait]
impl HostFn for FsWriteFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, _ctx: &InvocationCtx) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            path: String,
            data: String,
            #[serde(default)]
            create_parents: bool,
            #[serde(default)]
            overwrite: bool,
            #[allow(dead_code)]
            mime: Option<String>,
        }
        let args: Args = parse_args(args)?;
        let resolved = self
            .linked
            .resolve_for_create(&args.path, args.create_parents)?;
        ensure_rw(&resolved.policy, &resolved.display)?;
        let existed = resolved.path.exists();
        if existed && !args.overwrite {
            return Err(HostError::InvalidArgs(format!(
                "{} already exists; set overwrite=true",
                resolved.display
            )));
        }
        fs::write(&resolved.path, args.data.as_bytes())
            .map_err(|err| HostError::HostCall(err.to_string()))?;
        let result = WriteResult {
            path: display_path(&resolved.alias, &resolved.relative),
            uri: linked_uri(&resolved.alias, &resolved.relative),
            bytes_written: args.data.len(),
            created: !existed,
            overwritten: existed,
        };
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

struct FsLsFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsLsFn {
    fn new(linked: LinkedFolders) -> Self {
        Self {
            linked,
            docs: docs("fs.ls", "fs", "List linked-folder entries", false),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    pub path: String,
    pub uri: String,
    pub name: String,
    pub kind: String,
    pub size_bytes: Option<u64>,
    pub modified_at: Option<String>,
}

#[async_trait]
impl HostFn for FsLsFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, _ctx: &InvocationCtx) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            path: Option<String>,
            #[serde(default)]
            recursive: bool,
            limit: Option<usize>,
            #[serde(default)]
            include_hidden: bool,
        }
        let args: Args = parse_args(args)?;
        let resolved = self.linked.resolve_existing(args.path.as_deref())?;
        let entries = list_entries(
            &resolved,
            args.recursive,
            args.limit.unwrap_or(1000),
            args.include_hidden,
        )?;
        serde_json::to_value(entries).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

struct FsFindFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsFindFn {
    fn new(linked: LinkedFolders) -> Self {
        Self {
            linked,
            docs: docs("fs.find", "fs", "Find linked-folder entries by glob", false),
        }
    }
}

#[async_trait]
impl HostFn for FsFindFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, _ctx: &InvocationCtx) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            patterns: PatternInput,
            cwd: Option<String>,
            limit: Option<usize>,
            #[serde(default)]
            include_hidden: bool,
            respect_gitignore: Option<bool>,
        }
        let args: Args = parse_args(args)?;
        let respect_gitignore = args.respect_gitignore.unwrap_or(true);
        let patterns = args.patterns.into_vec();
        let globset = compile_globs(&patterns)?;
        let resolved = self.linked.resolve_existing(args.cwd.as_deref())?;
        let root_ignores = if respect_gitignore {
            load_simple_gitignore(&resolved.policy.root)?
        } else {
            None
        };
        let limit = args.limit.unwrap_or(1000);
        let mut builder = WalkBuilder::new(&resolved.path);
        builder
            .hidden(!args.include_hidden)
            .git_ignore(respect_gitignore)
            .git_exclude(respect_gitignore)
            .ignore(respect_gitignore)
            .parents(respect_gitignore);
        let mut entries = Vec::new();
        for dent in builder.build() {
            let dent = dent.map_err(|err| HostError::HostCall(err.to_string()))?;
            let path = dent.path();
            if path == resolved.path {
                continue;
            }
            let rel_to_cwd = path.strip_prefix(&resolved.path).unwrap_or(path);
            let rel_for_alias = path.strip_prefix(&resolved.policy.root).unwrap_or(path);
            if let Some(ignores) = &root_ignores
                && ignores.is_match(rel_for_alias)
            {
                continue;
            }
            if !globset.is_match(rel_to_cwd) && !globset.is_match(rel_for_alias) {
                continue;
            }
            entries.push(fs_entry(&resolved.alias, rel_for_alias, path)?);
            if entries.len() >= limit {
                break;
            }
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        serde_json::to_value(entries).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PatternInput {
    One(String),
    Many(Vec<String>),
}

impl PatternInput {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(pattern) => vec![pattern],
            Self::Many(patterns) => patterns,
        }
    }
}

struct CodeSearchFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl CodeSearchFn {
    fn new(linked: LinkedFolders) -> Self {
        Self {
            linked,
            docs: docs("code.search", "code", "Search UTF-8 linked files", false),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchMatch {
    path: String,
    uri: String,
    line: usize,
    column: usize,
    text: String,
    before: Vec<String>,
    after: Vec<String>,
    tag: String,
}

#[async_trait]
impl HostFn for CodeSearchFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, _ctx: &InvocationCtx) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            pattern: String,
            paths: Vec<String>,
            case_sensitive: Option<bool>,
            regex: Option<bool>,
            context_lines: Option<usize>,
            limit: Option<usize>,
        }
        let args: Args = parse_args(args)?;
        let pattern = if args.regex.unwrap_or(true) {
            args.pattern
        } else {
            regex::escape(&args.pattern)
        };
        let re = RegexBuilder::new(&pattern)
            .case_insensitive(!args.case_sensitive.unwrap_or(true))
            .build()
            .map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let context_lines = args.context_lines.unwrap_or(0);
        let limit = args.limit.unwrap_or(1000);
        let mut files = Vec::new();
        for path in &args.paths {
            let resolved = self.linked.resolve_existing(Some(path))?;
            if resolved.path.is_dir() {
                collect_files(&resolved, &mut files)?;
            } else {
                files.push(resolved);
            }
        }
        files.sort_by(|a, b| a.display.cmp(&b.display));
        let mut out = Vec::new();
        for file in files {
            let bytes = fs::read(&file.path).map_err(|err| HostError::HostCall(err.to_string()))?;
            let tag = file_tag(&bytes);
            let content = String::from_utf8(bytes).map_err(|_| {
                HostError::InvalidArgs("code.search supports UTF-8 text only in P0".to_string())
            })?;
            let lines: Vec<&str> = content.lines().collect();
            for (idx, line) in lines.iter().enumerate() {
                for mat in re.find_iter(line) {
                    let start = idx.saturating_sub(context_lines);
                    let end = (idx + 1 + context_lines).min(lines.len());
                    out.push(SearchMatch {
                        path: display_path(&file.alias, &file.relative),
                        uri: linked_uri(&file.alias, &file.relative),
                        line: idx + 1,
                        column: mat.start() + 1,
                        text: (*line).to_string(),
                        before: lines[start..idx]
                            .iter()
                            .map(|line| (*line).to_string())
                            .collect(),
                        after: lines[idx + 1..end]
                            .iter()
                            .map(|line| (*line).to_string())
                            .collect(),
                        tag: tag.clone(),
                    });
                    if out.len() >= limit {
                        return serde_json::to_value(out)
                            .map_err(|err| HostError::HostCall(err.to_string()));
                    }
                }
            }
        }
        serde_json::to_value(out).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

struct CodeEditFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl CodeEditFn {
    fn new(linked: LinkedFolders) -> Self {
        Self {
            linked,
            docs: docs("code.edit", "code", "Apply patch-only line edits", true),
        }
    }
}

#[derive(Debug, Deserialize)]
struct PatchEdit {
    path: String,
    tag: Option<String>,
    hunks: Vec<PatchHunk>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum PatchHunk {
    #[serde(rename_all = "camelCase")]
    Replace {
        start_line: usize,
        end_line: usize,
        lines: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Delete {
        start_line: usize,
        end_line: usize,
    },
    Insert {
        at: InsertAt,
        line: Option<usize>,
        lines: Vec<String>,
    },
    Move {
        dest: String,
    },
    Remove,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum InsertAt {
    Head,
    Tail,
    Before,
    After,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditResult {
    path: String,
    changed: bool,
    diff: String,
    new_tag: Option<String>,
    diagnostics: Vec<Value>,
}

#[async_trait]
impl HostFn for CodeEditFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        let edit: PatchEdit = parse_args(args)?;
        if edit.hunks.is_empty() {
            return Err(HostError::InvalidArgs(
                "code.edit requires at least one hunk".to_string(),
            ));
        }
        let resolved = self.linked.resolve_for_create(&edit.path, false)?;
        ensure_rw(&resolved.policy, &resolved.display)?;
        let exists = resolved.path.exists();
        let old_bytes = if exists {
            fs::read(&resolved.path).map_err(|err| HostError::HostCall(err.to_string()))?
        } else {
            Vec::new()
        };
        if exists {
            let actual = file_tag(&old_bytes);
            let provided = edit.tag.as_deref().ok_or_else(|| {
                HostError::InvalidArgs("code.edit requires tag for existing files".to_string())
            })?;
            if provided != actual {
                return Err(HostError::InvalidArgs(format!(
                    "stale tag for {}: expected {actual}, got {provided}",
                    edit.path
                )));
            }
        }
        let remove_only = edit.hunks.len() == 1 && matches!(edit.hunks[0], PatchHunk::Remove);
        if edit
            .hunks
            .iter()
            .any(|hunk| matches!(hunk, PatchHunk::Remove))
            && !remove_only
        {
            return Err(HostError::InvalidArgs(
                "code.edit remove must be the only hunk".to_string(),
            ));
        }
        if remove_only {
            ctx.require_approval(&format!("code.edit remove {}", edit.path))
                .await?;
            fs::remove_file(&resolved.path).map_err(|err| HostError::HostCall(err.to_string()))?;
            let result = EditResult {
                path: display_path(&resolved.alias, &resolved.relative),
                changed: true,
                diff: simple_diff(&String::from_utf8_lossy(&old_bytes), "", &edit.path),
                new_tag: None,
                diagnostics: Vec::new(),
            };
            return serde_json::to_value(result)
                .map_err(|err| HostError::HostCall(err.to_string()));
        }
        let old = String::from_utf8(old_bytes.clone()).map_err(|_| {
            HostError::InvalidArgs("code.edit supports UTF-8 text only in P0".to_string())
        })?;
        let mut new = apply_line_hunks(&old, &edit.hunks)?;
        let mut final_path = resolved.path.clone();
        let mut final_alias = resolved.alias.clone();
        let mut final_relative = resolved.relative.clone();
        for hunk in &edit.hunks {
            if let PatchHunk::Move { dest } = hunk {
                let dest_resolved = self.linked.resolve_for_create(dest, false)?;
                if dest_resolved.alias != resolved.alias {
                    return Err(HostError::InvalidPath(
                        "code.edit move destination must stay in the same alias".to_string(),
                    ));
                }
                ensure_rw(&dest_resolved.policy, &dest_resolved.display)?;
                if dest_resolved.path.exists() {
                    ctx.require_approval(&format!(
                        "code.edit move overwrite {} -> {}",
                        edit.path, dest
                    ))
                    .await?;
                }
                final_path = dest_resolved.path;
                final_alias = dest_resolved.alias;
                final_relative = dest_resolved.relative;
            }
        }
        let changed = old != new || final_path != resolved.path;
        if changed {
            if let Some(parent) = final_path.parent() {
                fs::create_dir_all(parent).map_err(|err| HostError::HostCall(err.to_string()))?;
            }
            fs::write(&final_path, new.as_bytes())
                .map_err(|err| HostError::HostCall(err.to_string()))?;
            if final_path != resolved.path && resolved.path.exists() {
                fs::remove_file(&resolved.path)
                    .map_err(|err| HostError::HostCall(err.to_string()))?;
            }
        } else {
            new = old.clone();
        }
        let new_bytes =
            fs::read(&final_path).map_err(|err| HostError::HostCall(err.to_string()))?;
        let result = EditResult {
            path: display_path(&final_alias, &final_relative),
            changed,
            diff: simple_diff(&old, &new, &edit.path),
            new_tag: Some(file_tag(&new_bytes)),
            diagnostics: Vec::new(),
        };
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

struct ProcRunFn {
    linked: LinkedFolders,
    artifact_store: ArtifactStore,
    docs: ToolDocs,
}

impl ProcRunFn {
    fn new(linked: LinkedFolders, artifact_store: ArtifactStore) -> Self {
        Self {
            linked,
            artifact_store,
            docs: docs(
                "proc.run",
                "proc",
                "Run allowlisted argv-vector commands",
                true,
            ),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcRunResult {
    cmd: String,
    args: Vec<String>,
    cwd: String,
    exit_code: i32,
    timed_out: bool,
    stdout: String,
    stderr: String,
    truncated: bool,
    artifact: Option<ArtifactRef>,
    duration_ms: u128,
}

#[async_trait]
impl HostFn for ProcRunFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            cmd: String,
            #[serde(default)]
            args: Vec<String>,
            cwd: Option<String>,
            timeout_ms: Option<u64>,
            output_bytes: Option<usize>,
            stdin: Option<Value>,
            env: Option<BTreeMap<String, Value>>,
        }
        let args: Args = parse_args(args)?;
        validate_command_name(&args.cmd)?;
        if stdin_present(&args.stdin) {
            return Err(HostError::InvalidArgs(
                "proc.run stdin is unavailable in P0".to_string(),
            ));
        }
        if args.env.as_ref().is_some_and(|env| !env.is_empty()) {
            return Err(HostError::InvalidArgs(
                "proc.run env overrides are unavailable in P0".to_string(),
            ));
        }
        let cwd = self.linked.resolve_existing(args.cwd.as_deref())?;
        if !cwd.policy.commands.contains(&args.cmd) {
            return Err(HostError::CapabilityDenied(args.cmd.clone()));
        }
        let mut argv = vec![args.cmd.clone()];
        argv.extend(args.args.clone());
        let safe = cwd
            .policy
            .safe_args
            .iter()
            .any(|prefix| argv.starts_with(prefix));
        if !safe {
            ctx.require_approval(&format!("proc.run {}", argv.join(" ")))
                .await?;
        }
        let timeout_ms = args.timeout_ms.unwrap_or(180_000).min(180_000);
        let output_bytes = args.output_bytes.unwrap_or(50_000);
        let start = Instant::now();
        let mut command = tokio::process::Command::new(&args.cmd);
        command
            .args(&args.args)
            .current_dir(&cwd.path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env_clear();
        for (key, value) in env::vars() {
            let upper = key.to_uppercase();
            if ["KEY", "TOKEN", "SECRET", "PASSWORD", "COOKIE", "AUTH"]
                .iter()
                .any(|needle| upper.contains(needle))
            {
                continue;
            }
            command.env(key, value);
        }
        let output =
            tokio::time::timeout(Duration::from_millis(timeout_ms), command.output()).await;
        let duration_ms = start.elapsed().as_millis();
        let (exit_code, timed_out, stdout, stderr) = match output {
            Ok(Ok(output)) => (
                output.status.code().unwrap_or(-1),
                false,
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
            ),
            Ok(Err(err)) => return Err(HostError::HostCall(err.to_string())),
            Err(_) => (
                -1,
                true,
                String::new(),
                "TimeoutError: proc.run timed out".to_string(),
            ),
        };
        let combined = format!("{stdout}{stderr}");
        let (stdout, stderr, truncated, artifact) = if combined.len() > output_bytes {
            let artifact = self
                .artifact_store
                .put_text(
                    &combined,
                    Some(format!("proc.run {}", args.cmd)),
                    "text/plain",
                )
                .map_err(|err| HostError::HostCall(err.to_string()))?;
            (
                preview(&stdout, output_bytes),
                preview(&stderr, output_bytes),
                true,
                Some(artifact),
            )
        } else {
            (stdout, stderr, false, None)
        };
        let result = ProcRunResult {
            cmd: args.cmd,
            args: args.args,
            cwd: display_path(&cwd.alias, &cwd.relative),
            exit_code,
            timed_out,
            stdout,
            stderr,
            truncated,
            artifact,
            duration_ms,
        };
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

pub struct LinkedResourceHandler {
    linked: LinkedFolders,
}

impl LinkedResourceHandler {
    pub fn new(linked: LinkedFolders) -> Self {
        Self { linked }
    }
}

#[async_trait]
impl ResourceHandler for LinkedResourceHandler {
    fn scheme(&self) -> &str {
        "linked"
    }

    fn capability(&self) -> &str {
        "resources.read:linked"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> Result<ResourceContent> {
        self.linked.read_resource(uri, selector)
    }

    async fn preview(&self, uri: &str, _ctx: &InvocationCtx) -> Result<ResourceContent> {
        let mut content = self.linked.read_resource(uri, None)?;
        content.content.clear();
        Ok(content)
    }

    async fn list(&self, uri: Option<&str>, _ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        let resolved = self.linked.resolve_existing(uri)?;
        list_entries(&resolved, false, 1000, false).map(|entries| {
            entries
                .into_iter()
                .map(|entry| ResourceEntry {
                    uri: entry.uri,
                    name: entry.name,
                    kind: entry.kind,
                    title: None,
                    size_bytes: entry.size_bytes.map(|n| n as usize),
                    modified_at: entry.modified_at,
                })
                .collect()
        })
    }
}

fn docs(name: &str, namespace: &str, summary: &str, sensitive: bool) -> ToolDocs {
    let base = ToolDocs {
        name: name.to_string(),
        namespace: namespace.to_string(),
        summary: summary.to_string(),
        description: None,
        signature: format!("{name}(args)"),
        args_schema: json!({ "type": "object" }),
        result_schema: None,
        examples: Vec::new(),
        errors: Vec::new(),
        grants: vec![GrantDoc {
            kind: "workspace".to_string(),
            description: format!("Allows the {name} capability."),
        }],
        sensitive,
        approval: if sensitive { "policy" } else { "none" }.to_string(),
        since: "P0".to_string(),
        stability: "experimental".to_string(),
    };

    match name {
        "fs.read" => ToolDocs {
            description: Some(
                "Read a UTF-8 text file from a granted linked folder and return a ResourceContent envelope."
                    .to_string(),
            ),
            signature: "fs.read(path: SdkPath, opts?: FsReadOptions): Promise<ResourceContent>"
                .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["path"],
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string", "description": "Linked path such as tempestmiku:src/lib.rs or linked://tempestmiku/src/lib.rs." },
                    "selector": { "type": "string", "description": "Optional 1-based inclusive line range, for example 10-40." },
                    "raw": { "type": "boolean", "description": "Reserved in P0; reads still return ResourceContent." }
                }
            }),
            result_schema: Some(resource_content_schema()),
            examples: vec![ToolExample {
                title: Some("Read a Rust file".to_string()),
                code: "const file = await fs.read('tempestmiku:src/lib.rs');\ndisplay(file.content, { kind: 'text' });"
                    .to_string(),
                notes: Some("The result is an envelope with uri, kind, mime, content, preview, sizeBytes, and hasMore.".to_string()),
            }],
            errors: vec![
                tool_error("CapabilityDeniedError", "The session lacks the fs.read grant.", false),
                tool_error("InvalidPathError", "The linked alias is unknown, the path escapes the root, or the file does not exist.", false),
                tool_error("InvalidArgsError", "The selector is malformed or the file is not UTF-8 text.", false),
                tool_error("HostCallError", "The host filesystem read fails after policy checks.", false),
            ],
            grants: vec![linked_grant("Read access to the target linked folder.")],
            approval: "none".to_string(),
            ..base
        },
        "fs.write" => ToolDocs {
            description: Some(
                "Write UTF-8 text below a writable linked-folder grant. Binary writes are deferred."
                    .to_string(),
            ),
            signature:
                "fs.write(path: SdkPath, data: string, opts?: FsWriteOptions): Promise<FsWriteResult>"
                    .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["path", "data"],
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string" },
                    "data": { "type": "string" },
                    "createParents": { "type": "boolean", "default": false },
                    "overwrite": { "type": "boolean", "default": false },
                    "mime": { "type": "string", "description": "Reserved metadata hint in P0." }
                }
            }),
            result_schema: Some(write_result_schema()),
            examples: vec![ToolExample {
                title: Some("Create a file".to_string()),
                code: "await fs.write('tempestmiku:notes/todo.txt', 'ship P1\\n', { createParents: true });"
                    .to_string(),
                notes: None,
            }],
            errors: vec![
                tool_error("CapabilityDeniedError", "The linked folder is read-only or fs.write is not granted.", false),
                tool_error("InvalidPathError", "The path is outside the linked root or the parent is unavailable.", false),
                tool_error("InvalidArgsError", "The target exists without overwrite=true, or args do not match the schema.", false),
                tool_error("HostCallError", "The host write fails after policy checks.", false),
            ],
            grants: vec![linked_grant("Write access to the target linked folder.")],
            approval: "on-write".to_string(),
            ..base
        },
        "fs.ls" => ToolDocs {
            description: Some("List files and directories under a linked folder.".to_string()),
            signature: "fs.ls(path?: SdkPath, opts?: FsListOptions): Promise<FsEntry[]>"
                .to_string(),
            args_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string", "description": "Defaults to the first linked folder alias." },
                    "recursive": { "type": "boolean", "default": false },
                    "limit": { "type": "integer", "minimum": 1, "default": 1000 },
                    "includeHidden": { "type": "boolean", "default": false }
                }
            }),
            result_schema: Some(json!({ "type": "array", "items": fs_entry_schema() })),
            examples: vec![ToolExample {
                title: Some("List source files".to_string()),
                code: "const entries = await fs.ls('tempestmiku:src');\ndisplay(entries, { kind: 'json' });"
                    .to_string(),
                notes: None,
            }],
            errors: read_only_errors("fs.ls"),
            grants: vec![linked_grant("List access to the target linked folder.")],
            approval: "none".to_string(),
            ..base
        },
        "fs.find" => ToolDocs {
            description: Some(
                "Find linked-folder entries by glob, honoring gitignore by default.".to_string(),
            ),
            signature:
                "fs.find(patterns: string | string[], opts?: FsFindOptions): Promise<FsEntry[]>"
                    .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["patterns"],
                "additionalProperties": false,
                "properties": {
                    "patterns": {
                        "oneOf": [
                            { "type": "string" },
                            { "type": "array", "items": { "type": "string" } }
                        ]
                    },
                    "cwd": { "type": "string", "description": "Linked path to search from; defaults to the first linked folder." },
                    "limit": { "type": "integer", "minimum": 1, "default": 1000 },
                    "includeHidden": { "type": "boolean", "default": false },
                    "respectGitignore": { "type": "boolean", "default": true }
                }
            }),
            result_schema: Some(json!({ "type": "array", "items": fs_entry_schema() })),
            examples: vec![ToolExample {
                title: Some("Find Rust files".to_string()),
                code: "const files = await fs.find('**/*.rs', { cwd: 'tempestmiku:' });"
                    .to_string(),
                notes: Some("Patterns are matched against both the cwd-relative and linked-root-relative path.".to_string()),
            }],
            errors: read_only_errors("fs.find"),
            grants: vec![linked_grant("Search access to the target linked folder.")],
            approval: "none".to_string(),
            ..base
        },
        "code.search" => ToolDocs {
            description: Some(
                "Search UTF-8 files with regex or literal text and return optimistic-concurrency tags."
                    .to_string(),
            ),
            signature: "code.search(query: CodeSearchQuery): Promise<CodeSearchResult[]>"
                .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["pattern", "paths"],
                "additionalProperties": false,
                "properties": {
                    "pattern": { "type": "string" },
                    "paths": { "type": "array", "minItems": 1, "items": { "type": "string" } },
                    "caseSensitive": { "type": "boolean", "default": true },
                    "regex": { "type": "boolean", "default": true },
                    "contextLines": { "type": "integer", "minimum": 0, "default": 0 },
                    "limit": { "type": "integer", "minimum": 1, "default": 1000 }
                }
            }),
            result_schema: Some(json!({ "type": "array", "items": code_search_result_schema() })),
            examples: vec![ToolExample {
                title: Some("Search literal text".to_string()),
                code: "const hits = await code.search({ pattern: 'TODO', paths: ['tempestmiku:'], regex: false, contextLines: 1 });"
                    .to_string(),
                notes: Some("Use a hit's tag with code.edit when editing an existing file.".to_string()),
            }],
            errors: read_only_errors("code.search"),
            grants: vec![linked_grant("Read/search access to the target linked folder.")],
            approval: "none".to_string(),
            ..base
        },
        "code.edit" => ToolDocs {
            description: Some(
                "Apply narrow JSON line hunks with optimistic concurrency tags; raw patch strings are not accepted."
                    .to_string(),
            ),
            signature:
                "code.edit(patch: PatchEdit, opts?: CodeEditOptions): Promise<CodeEditResult>"
                    .to_string(),
            args_schema: code_edit_args_schema(),
            result_schema: Some(code_edit_result_schema()),
            examples: vec![ToolExample {
                title: Some("Replace one line".to_string()),
                code: "const [hit] = await code.search({ pattern: 'old', paths: ['tempestmiku:src/lib.rs'], regex: false });\nawait code.edit({ path: hit.path, tag: hit.tag, hunks: [{ op: 'replace', startLine: hit.line, endLine: hit.line, lines: ['new'] }] });"
                    .to_string(),
                notes: Some("Existing files require a fresh tag from code.search or another read/search path.".to_string()),
            }],
            errors: vec![
                tool_error("CapabilityDeniedError", "code.edit is not granted or the linked folder is read-only.", false),
                tool_error("ApprovalDeniedError", "The user denies a destructive remove or overwrite move.", false),
                tool_error("ApprovalTimeoutError", "The approval request times out and defaults to deny.", true),
                tool_error("InvalidArgsError", "The tag is stale, hunks are malformed, or remove is mixed with other hunks.", false),
                tool_error("InvalidPathError", "The path escapes the linked root or move crosses aliases.", false),
                tool_error("HostCallError", "The host edit fails after policy checks.", false),
            ],
            grants: vec![linked_grant("Write access to the target linked folder.")],
            approval: "policy".to_string(),
            ..base
        },
        "proc.run" => ToolDocs {
            description: Some(
                "Run an allowlisted command with argv-vector arguments inside a linked-folder cwd; shell strings, pipes, redirects, env overrides, and stdin are rejected."
                    .to_string(),
            ),
            signature: "proc.run(cmd: string, args?: string[], opts?: ProcRunOptions): Promise<ProcOutput>"
                .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["cmd"],
                "additionalProperties": false,
                "properties": {
                    "cmd": { "type": "string", "description": "Executable name only, for example cargo. Shell strings like cargo test are invalid." },
                    "args": { "type": "array", "items": { "type": "string" }, "default": [] },
                    "cwd": { "type": "string", "description": "Linked cwd; defaults to the first linked folder." },
                    "timeoutMs": { "type": "integer", "minimum": 1, "maximum": 180000, "default": 180000 },
                    "outputBytes": { "type": "integer", "minimum": 1, "default": 50000 },
                    "env": { "type": "object", "description": "Reserved in P0; non-empty env overrides are rejected." },
                    "stdin": { "description": "Reserved in P0; non-empty stdin is rejected." }
                }
            }),
            result_schema: Some(proc_output_schema()),
            examples: vec![ToolExample {
                title: Some("Run tests".to_string()),
                code: "const run = await proc.run('cargo', ['test'], { cwd: 'tempestmiku:' });\ndisplay(run, { kind: 'json' });"
                    .to_string(),
                notes: Some("Use cmd plus args. Do not pass proc.run('cargo test').".to_string()),
            }],
            errors: vec![
                tool_error("CapabilityDeniedError", "proc.run is not granted or the command is not allowlisted for the linked cwd.", false),
                tool_error("ApprovalDeniedError", "The user denies a command outside safe argv prefixes.", false),
                tool_error("ApprovalTimeoutError", "The approval request times out and defaults to deny.", true),
                tool_error("InvalidArgsError", "cmd is not a bare executable, or stdin/env/shell-style args are requested.", false),
                tool_error("InvalidPathError", "cwd is outside the linked root or missing.", false),
                tool_error("HostCallError", "The host process cannot be spawned or collected.", false),
            ],
            grants: vec![GrantDoc {
                kind: "process".to_string(),
                description: "Allowlisted process execution for the linked folder cwd.".to_string(),
            }],
            approval: "policy".to_string(),
            ..base
        },
        _ => base,
    }
}

fn linked_grant(description: &str) -> GrantDoc {
    GrantDoc {
        kind: "linked-folder".to_string(),
        description: description.to_string(),
    }
}

fn tool_error(name: &str, when: &str, retryable: bool) -> ToolErrorDoc {
    ToolErrorDoc {
        name: name.to_string(),
        when: when.to_string(),
        retryable,
    }
}

fn read_only_errors(capability: &str) -> Vec<ToolErrorDoc> {
    vec![
        tool_error(
            "CapabilityDeniedError",
            &format!("The session lacks the {capability} grant."),
            false,
        ),
        tool_error(
            "InvalidPathError",
            "The linked alias is unknown or the path escapes the root.",
            false,
        ),
        tool_error(
            "InvalidArgsError",
            "Arguments do not match the schema, a glob/regex is invalid, or a file is not UTF-8 text.",
            false,
        ),
        tool_error(
            "HostCallError",
            "The host filesystem operation fails after policy checks.",
            false,
        ),
    ]
}

fn resource_content_schema() -> Value {
    json!({
        "type": "object",
        "required": ["uri", "kind", "mime", "sizeBytes", "hasMore", "content", "preview"],
        "properties": {
            "uri": { "type": "string" },
            "kind": { "type": "string" },
            "mime": { "type": "string" },
            "title": { "type": ["string", "null"] },
            "sizeBytes": { "type": "integer" },
            "selector": { "type": ["string", "null"] },
            "hasMore": { "type": "boolean" },
            "content": { "type": "string" },
            "preview": { "type": "string" }
        }
    })
}

fn write_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "uri", "bytesWritten", "created", "overwritten"],
        "properties": {
            "path": { "type": "string" },
            "uri": { "type": "string" },
            "bytesWritten": { "type": "integer" },
            "created": { "type": "boolean" },
            "overwritten": { "type": "boolean" }
        }
    })
}

fn fs_entry_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "uri", "name", "kind"],
        "properties": {
            "path": { "type": "string" },
            "uri": { "type": "string" },
            "name": { "type": "string" },
            "kind": { "enum": ["file", "directory", "symlink", "other"] },
            "sizeBytes": { "type": ["integer", "null"] },
            "modifiedAt": { "type": ["string", "null"] }
        }
    })
}

fn code_search_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "uri", "line", "column", "text", "before", "after", "tag"],
        "properties": {
            "path": { "type": "string" },
            "uri": { "type": "string" },
            "line": { "type": "integer", "minimum": 1 },
            "column": { "type": "integer", "minimum": 1 },
            "text": { "type": "string" },
            "before": { "type": "array", "items": { "type": "string" } },
            "after": { "type": "array", "items": { "type": "string" } },
            "tag": { "type": "string", "description": "Optimistic concurrency tag for code.edit." }
        }
    })
}

fn code_edit_args_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "hunks"],
        "additionalProperties": false,
        "properties": {
            "path": { "type": "string" },
            "tag": { "type": "string", "description": "Required for existing files." },
            "format": { "type": "boolean", "description": "Reserved in P0." },
            "hunks": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "required": ["op", "startLine", "endLine", "lines"],
                            "properties": {
                                "op": { "const": "replace" },
                                "startLine": { "type": "integer", "minimum": 1 },
                                "endLine": { "type": "integer", "minimum": 1 },
                                "lines": { "type": "array", "items": { "type": "string" } }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["op", "startLine", "endLine"],
                            "properties": {
                                "op": { "const": "delete" },
                                "startLine": { "type": "integer", "minimum": 1 },
                                "endLine": { "type": "integer", "minimum": 1 }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["op", "at", "lines"],
                            "properties": {
                                "op": { "const": "insert" },
                                "at": { "enum": ["head", "tail", "before", "after"] },
                                "line": { "type": "integer", "minimum": 1 },
                                "lines": { "type": "array", "items": { "type": "string" } }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["op", "dest"],
                            "properties": {
                                "op": { "const": "move" },
                                "dest": { "type": "string" }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["op"],
                            "properties": {
                                "op": { "const": "remove" }
                            }
                        }
                    ]
                }
            }
        }
    })
}

fn code_edit_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "changed", "diff", "diagnostics"],
        "properties": {
            "path": { "type": "string" },
            "changed": { "type": "boolean" },
            "diff": { "type": "string" },
            "newTag": { "type": ["string", "null"] },
            "diagnostics": { "type": "array" }
        }
    })
}

fn artifact_ref_schema() -> Value {
    json!({
        "type": "object",
        "required": ["uri", "id", "kind", "mime", "sizeBytes", "preview"],
        "properties": {
            "uri": { "type": "string" },
            "id": { "type": "string" },
            "kind": { "type": "string" },
            "mime": { "type": "string" },
            "title": { "type": ["string", "null"] },
            "sizeBytes": { "type": "integer" },
            "preview": { "type": "string" }
        }
    })
}

fn proc_output_schema() -> Value {
    json!({
        "type": "object",
        "required": ["cmd", "args", "cwd", "exitCode", "stdout", "stderr", "timedOut", "durationMs", "truncated"],
        "properties": {
            "cmd": { "type": "string" },
            "args": { "type": "array", "items": { "type": "string" } },
            "cwd": { "type": "string" },
            "exitCode": { "type": "integer" },
            "stdout": { "type": "string" },
            "stderr": { "type": "string" },
            "timedOut": { "type": "boolean" },
            "durationMs": { "type": "integer" },
            "truncated": { "type": "boolean" },
            "artifact": {
                "oneOf": [
                    artifact_ref_schema(),
                    { "type": "null" }
                ]
            }
        }
    })
}

fn parse_args<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T> {
    serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))
}

fn parse_scheme(uri: &str) -> Result<String> {
    if let Ok(url) = Url::parse(uri) {
        return Ok(url.scheme().to_string());
    }
    uri.split_once("://")
        .map(|(scheme, _)| scheme.to_string())
        .ok_or_else(|| HostError::InvalidArgs(format!("missing URI scheme in {uri}")))
}

fn validate_alias(alias: &str) -> Result<()> {
    let mut chars = alias.chars();
    let Some(first) = chars.next() else {
        return Err(HostError::InvalidArgs(
            "linked folder alias cannot be empty".to_string(),
        ));
    };
    if !first.is_ascii_alphabetic()
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(HostError::InvalidArgs(format!(
            "invalid linked folder alias {alias}"
        )));
    }
    Ok(())
}

fn validate_command_name(command: &str) -> Result<()> {
    let forbidden = [
        ' ', '\t', '\n', '\r', ';', '&', '|', '$', '`', '>', '<', '*', '?', '(', ')',
    ];
    if command.is_empty()
        || command.contains('/')
        || command.contains('\\')
        || command.chars().any(std::path::is_separator)
        || command.chars().any(|ch| forbidden.contains(&ch))
        || !command
            .chars()
            .any(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
    {
        return Err(HostError::InvalidArgs(format!(
            "invalid command name {command}"
        )));
    }
    Ok(())
}

fn parse_linked_path(input: &str) -> Result<ParsedPath> {
    if input.contains('\0') {
        return Err(HostError::InvalidPath("path contains NUL byte".to_string()));
    }
    if input.starts_with('/') || input.starts_with('\\') {
        return Err(HostError::InvalidPath(format!(
            "raw absolute path rejected: {input}"
        )));
    }
    if is_windows_drive(input) {
        return Err(HostError::InvalidPath(format!(
            "windows drive path rejected: {input}"
        )));
    }
    let (alias, relative) = if input.starts_with("linked://") {
        let url = Url::parse(input).map_err(|err| HostError::InvalidPath(err.to_string()))?;
        let alias = url
            .host_str()
            .ok_or_else(|| HostError::InvalidPath(format!("missing linked alias in {input}")))?;
        (
            alias.to_string(),
            url.path().trim_start_matches('/').to_string(),
        )
    } else {
        let (alias, relative) = input
            .split_once(':')
            .ok_or_else(|| HostError::InvalidPath(format!("missing linked alias in {input}")))?;
        (alias.to_string(), relative.to_string())
    };
    if alias.is_empty() {
        return Err(HostError::InvalidPath("empty linked alias".to_string()));
    }
    let relative = normalize_relative(&relative)?;
    let display = display_path(&alias, &relative);
    Ok(ParsedPath {
        alias,
        relative,
        display,
    })
}

fn normalize_relative(relative: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    let path = Path::new(relative);
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(HostError::InvalidPath(format!(
                    "invalid relative path component in {relative}"
                )));
            }
        }
    }
    Ok(out)
}

fn is_windows_drive(input: &str) -> bool {
    let bytes = input.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/' || bytes[2] == b'\\')
}

fn ensure_under_root(path: &Path, root: &Path, display: &str) -> Result<()> {
    if !path.starts_with(root) {
        return Err(HostError::InvalidPath(format!(
            "{display} resolves outside linked folder"
        )));
    }
    Ok(())
}

fn ensure_existing_ancestor_under_root(parent: &Path, root: &Path, display: &str) -> Result<()> {
    let mut current = parent;
    while !current.exists() {
        current = current
            .parent()
            .ok_or_else(|| HostError::InvalidPath(format!("missing ancestor for {display}")))?;
    }
    let canonical = current
        .canonicalize()
        .map_err(|err| HostError::InvalidPath(format!("{display}: {err}")))?;
    ensure_under_root(&canonical, root, display)
}

fn ensure_rw(policy: &FsPolicy, display: &str) -> Result<()> {
    if policy.mode != FsMode::Rw {
        return Err(HostError::CapabilityDenied(format!(
            "{display} is read-only"
        )));
    }
    Ok(())
}

fn display_path(alias: &str, relative: &Path) -> String {
    let rel = path_slash(relative);
    if rel.is_empty() {
        format!("{alias}:")
    } else {
        format!("{alias}:{rel}")
    }
}

fn linked_uri(alias: &str, relative: &Path) -> String {
    let rel = path_slash(relative);
    if rel.is_empty() {
        format!("linked://{alias}/")
    } else {
        format!("linked://{alias}/{rel}")
    }
}

fn path_slash(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn select_text(content: &str, selector: Option<&str>) -> Result<(String, bool)> {
    let Some(selector) = selector else {
        return Ok((content.to_string(), false));
    };
    let (start, end) = selector
        .split_once('-')
        .ok_or_else(|| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let start: usize = start
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let end: usize = end
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    if start == 0 || end < start {
        return Err(HostError::InvalidArgs(format!(
            "invalid selector {selector}"
        )));
    }
    let lines: Vec<&str> = content.lines().collect();
    let selected = lines
        .iter()
        .skip(start - 1)
        .take(end - start + 1)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Ok((selected, end < lines.len()))
}

fn list_entries(
    resolved: &ResolvedPath,
    recursive: bool,
    limit: usize,
    include_hidden: bool,
) -> Result<Vec<FsEntry>> {
    let mut out = Vec::new();
    if resolved.path.is_file() {
        out.push(fs_entry(
            &resolved.alias,
            &resolved.relative,
            &resolved.path,
        )?);
        return Ok(out);
    }
    visit_dir(
        &resolved.alias,
        &resolved.policy.root,
        &resolved.path,
        recursive,
        limit,
        include_hidden,
        &mut out,
    )?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn visit_dir(
    alias: &str,
    root: &Path,
    dir: &Path,
    recursive: bool,
    limit: usize,
    include_hidden: bool,
    out: &mut Vec<FsEntry>,
) -> Result<()> {
    if out.len() >= limit {
        return Ok(());
    }
    let mut entries = fs::read_dir(dir)
        .map_err(|err| HostError::HostCall(err.to_string()))?
        .collect::<std::result::Result<Vec<_>, io::Error>>()
        .map_err(|err| HostError::HostCall(err.to_string()))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        let name = entry.file_name();
        if !include_hidden && name.to_string_lossy().starts_with('.') {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(path.as_path());
        out.push(fs_entry(alias, rel, &path)?);
        if recursive && path.is_dir() {
            visit_dir(alias, root, &path, recursive, limit, include_hidden, out)?;
        }
    }
    Ok(())
}

fn fs_entry(alias: &str, relative: &Path, path: &Path) -> Result<FsEntry> {
    let metadata =
        fs::symlink_metadata(path).map_err(|err| HostError::HostCall(err.to_string()))?;
    let kind = if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_dir() {
        "dir"
    } else {
        "file"
    };
    Ok(FsEntry {
        path: display_path(alias, relative),
        uri: linked_uri(alias, relative),
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(alias)
            .to_string(),
        kind: kind.to_string(),
        size_bytes: metadata.is_file().then_some(metadata.len()),
        modified_at: metadata.modified().ok().map(system_time_rfc3339),
    })
}

fn system_time_rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339()
}

fn compile_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).map_err(|err| HostError::InvalidArgs(err.to_string()))?);
    }
    builder
        .build()
        .map_err(|err| HostError::InvalidArgs(err.to_string()))
}

fn load_simple_gitignore(root: &Path) -> Result<Option<GlobSet>> {
    let path = root.join(".gitignore");
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|err| HostError::HostCall(err.to_string()))?;
    let patterns = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('!'))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if patterns.is_empty() {
        return Ok(None);
    }
    compile_globs(&patterns).map(Some)
}

fn collect_files(resolved: &ResolvedPath, out: &mut Vec<ResolvedPath>) -> Result<()> {
    for dent in WalkBuilder::new(&resolved.path).hidden(true).build() {
        let dent = dent.map_err(|err| HostError::HostCall(err.to_string()))?;
        let path = dent.path();
        if path == resolved.path || !path.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(&resolved.policy.root)
            .map_err(|err| HostError::HostCall(err.to_string()))?
            .to_path_buf();
        out.push(ResolvedPath {
            alias: resolved.alias.clone(),
            relative: relative.clone(),
            display: display_path(&resolved.alias, &relative),
            path: path.to_path_buf(),
            policy: resolved.policy.clone(),
        });
    }
    Ok(())
}

fn file_tag(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    hex::encode(hash)[..16].to_string()
}

fn apply_line_hunks(old: &str, hunks: &[PatchHunk]) -> Result<String> {
    let had_trailing_newline = old.ends_with('\n');
    let body = if had_trailing_newline {
        &old[..old.len() - 1]
    } else {
        old
    };
    let lines: Vec<String> = if body.is_empty() {
        Vec::new()
    } else {
        body.split('\n').map(str::to_string).collect()
    };
    #[derive(Clone)]
    struct Replacement {
        start: usize,
        end: usize,
        lines: Vec<String>,
    }
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut inserts: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for hunk in hunks {
        match hunk {
            PatchHunk::Replace {
                start_line,
                end_line,
                lines: new_lines,
            } => {
                validate_range(*start_line, *end_line, lines.len())?;
                replacements.push(Replacement {
                    start: start_line - 1,
                    end: *end_line,
                    lines: new_lines.clone(),
                });
            }
            PatchHunk::Delete {
                start_line,
                end_line,
            } => {
                validate_range(*start_line, *end_line, lines.len())?;
                replacements.push(Replacement {
                    start: start_line - 1,
                    end: *end_line,
                    lines: Vec::new(),
                });
            }
            PatchHunk::Insert {
                at,
                line,
                lines: new_lines,
            } => {
                let pos = match at {
                    InsertAt::Head => {
                        if line.is_some() {
                            return Err(HostError::InvalidArgs(
                                "insert head must not include line".to_string(),
                            ));
                        }
                        0
                    }
                    InsertAt::Tail => {
                        if line.is_some() {
                            return Err(HostError::InvalidArgs(
                                "insert tail must not include line".to_string(),
                            ));
                        }
                        lines.len()
                    }
                    InsertAt::Before => {
                        let line = line.ok_or_else(|| {
                            HostError::InvalidArgs("insert before requires line".to_string())
                        })?;
                        validate_line(line, lines.len())?;
                        line - 1
                    }
                    InsertAt::After => {
                        let line = line.ok_or_else(|| {
                            HostError::InvalidArgs("insert after requires line".to_string())
                        })?;
                        validate_line(line, lines.len())?;
                        line
                    }
                };
                inserts.entry(pos).or_default().extend(new_lines.clone());
            }
            PatchHunk::Move { .. } => {}
            PatchHunk::Remove => {}
        }
    }
    replacements.sort_by_key(|replacement| replacement.start);
    for pair in replacements.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(HostError::InvalidArgs(
                "overlapping replace/delete hunks".to_string(),
            ));
        }
    }
    let mut out = Vec::new();
    let mut idx = 0;
    let mut replacement_idx = 0;
    while idx <= lines.len() {
        if let Some(new_lines) = inserts.get(&idx) {
            out.extend(new_lines.clone());
        }
        if idx == lines.len() {
            break;
        }
        if replacement_idx < replacements.len() && replacements[replacement_idx].start == idx {
            out.extend(replacements[replacement_idx].lines.clone());
            idx = replacements[replacement_idx].end;
            replacement_idx += 1;
            continue;
        }
        out.push(lines[idx].clone());
        idx += 1;
    }
    let mut new = out.join("\n");
    if had_trailing_newline {
        new.push('\n');
    }
    Ok(new)
}

fn validate_range(start: usize, end: usize, len: usize) -> Result<()> {
    if start == 0 || end < start || end > len {
        return Err(HostError::InvalidArgs(format!(
            "invalid line range {start}-{end} for {len} lines"
        )));
    }
    Ok(())
}

fn validate_line(line: usize, len: usize) -> Result<()> {
    if line == 0 || line > len {
        return Err(HostError::InvalidArgs(format!(
            "invalid line {line} for {len} lines"
        )));
    }
    Ok(())
}

fn simple_diff(old: &str, new: &str, path: &str) -> String {
    if old == new {
        return String::new();
    }
    let mut diff = format!("--- {path}\n+++ {path}\n");
    for line in old.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn stdin_present(stdin: &Option<Value>) -> bool {
    match stdin {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

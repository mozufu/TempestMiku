use std::{
    path::{Component, Path},
    process::Stdio,
    sync::{Arc, atomic::AtomicUsize},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tm_artifacts::{ArtifactRef, ArtifactStore};
use url::Url;

#[cfg(unix)]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::os::fd::FromRawFd;

use crate::{HostError, HostFn, InvocationCtx, Result, ToolDocs};

use super::{
    LinkedFolders,
    proc::{
        bounded_io::{
            MAX_RETAINED_PROCESS_OUTPUT_BYTES, bounded_inline_output, read_bounded_output,
        },
        environment::{resolve_executable, sanitize_path},
        process_group::{ProcessGroupGuard, stop_process_tree},
    },
};
use crate::linked::{
    secure_fs::{SecureKind, open_existing},
    util::{approval_action, display_path, ensure_rw, parse_args},
};

const LOCAL_GIT_TIMEOUT: Duration = Duration::from_secs(30);
const NETWORK_GIT_TIMEOUT: Duration = Duration::from_secs(120);
const INLINE_OUTPUT_BYTES: usize = 50_000;
const MAX_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
const MAX_COMMIT_MESSAGE_BYTES: usize = 4 * 1024;
const MAX_PROBE_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_PATTERN_BYTES: usize = 4 * 1024;
const MAX_PATHS: usize = 64;
const MAX_PATHS_TOTAL_BYTES: usize = 16 * 1024;
const MAX_BISECT_GOOD_REVISIONS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitOperation {
    Status,
    Diff,
    Grep,
    Log,
    Show,
    Clone,
    Init,
    Add,
    Mv,
    Restore,
    Rm,
    Bisect,
    Commit,
    Push,
    Pull,
}

impl GitOperation {
    fn capability(self) -> &'static str {
        match self {
            Self::Status => "git.status",
            Self::Diff => "git.diff",
            Self::Grep => "git.grep",
            Self::Log => "git.log",
            Self::Show => "git.show",
            Self::Clone => "git.clone",
            Self::Init => "git.init",
            Self::Add => "git.add",
            Self::Mv => "git.mv",
            Self::Restore => "git.restore",
            Self::Rm => "git.rm",
            Self::Bisect => "git.bisect",
            Self::Commit => "git.commit",
            Self::Push => "git.push",
            Self::Pull => "git.pull",
        }
    }

    fn requires_approval(self) -> bool {
        !matches!(self, Self::Status | Self::Diff | Self::Grep)
    }

    fn requires_rw(self) -> bool {
        matches!(
            self,
            Self::Clone
                | Self::Init
                | Self::Add
                | Self::Mv
                | Self::Restore
                | Self::Rm
                | Self::Bisect
                | Self::Commit
                | Self::Push
                | Self::Pull
        )
    }

    fn mutates_local(self) -> bool {
        matches!(
            self,
            Self::Clone
                | Self::Init
                | Self::Add
                | Self::Mv
                | Self::Restore
                | Self::Rm
                | Self::Bisect
                | Self::Commit
                | Self::Pull
        )
    }

    fn requires_repository(self) -> bool {
        !matches!(self, Self::Clone | Self::Init)
    }

    fn timeout(self) -> Duration {
        if matches!(self, Self::Clone | Self::Push | Self::Pull) {
            NETWORK_GIT_TIMEOUT
        } else {
            LOCAL_GIT_TIMEOUT
        }
    }
}

pub(in crate::linked) struct GitReadFn {
    linked: LinkedFolders,
    artifact_store: ArtifactStore,
    operation: GitOperation,
    docs: ToolDocs,
}

impl GitReadFn {
    fn new(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        operation: GitOperation,
        docs: ToolDocs,
    ) -> Self {
        Self {
            linked,
            artifact_store,
            operation,
            docs,
        }
    }

    pub(in crate::linked) fn status(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Status, docs)
    }

    pub(in crate::linked) fn diff(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Diff, docs)
    }

    pub(in crate::linked) fn grep(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Grep, docs)
    }

    pub(in crate::linked) fn log(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Log, docs)
    }

    pub(in crate::linked) fn show(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Show, docs)
    }

    pub(in crate::linked) fn clone(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Clone, docs)
    }

    pub(in crate::linked) fn init(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Init, docs)
    }

    pub(in crate::linked) fn add(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Add, docs)
    }

    pub(in crate::linked) fn mv(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Mv, docs)
    }

    pub(in crate::linked) fn restore(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Restore, docs)
    }

    pub(in crate::linked) fn rm(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Rm, docs)
    }

    pub(in crate::linked) fn bisect(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Bisect, docs)
    }

    pub(in crate::linked) fn commit(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Commit, docs)
    }

    pub(in crate::linked) fn push(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Push, docs)
    }

    pub(in crate::linked) fn pull(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        docs: ToolDocs,
    ) -> Self {
        Self::new(linked, artifact_store, GitOperation::Pull, docs)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitResult {
    operation: &'static str,
    cwd: String,
    exit_code: i32,
    stdout: String,
    stderr: String,
    truncated: bool,
    artifact: Option<ArtifactRef>,
    duration_ms: u128,
}

struct RawGitOutput {
    exit_code: i32,
    stdout: String,
    stderr: String,
    output_limit_reached: bool,
    cwd: String,
    duration_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Upstream {
    remote_url: String,
    remote_ref: String,
    branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApprovalState {
    repository_root: String,
    git_dir: String,
    head: Option<String>,
    index_sha256: Option<String>,
    upstream: Option<Upstream>,
}

#[derive(Debug, Clone)]
enum GitArgs {
    Cwd,
    Commit {
        message: String,
    },
    Clone {
        url: String,
    },
    Paths {
        paths: Vec<String>,
    },
    Mv {
        path: String,
        dest: String,
    },
    Grep {
        pattern: String,
        case_sensitive: bool,
    },
    Show {
        revision: String,
    },
    Bisect(BisectArgs),
}

#[derive(Debug, Clone)]
enum BisectArgs {
    Start {
        bad: String,
        good: Vec<String>,
    },
    Mark {
        action: &'static str,
        revision: Option<String>,
    },
    Reset,
}

fn parse_git_args(operation: GitOperation, value: Value) -> Result<(Option<String>, GitArgs)> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct CwdArgs {
        cwd: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct CommitArgs {
        cwd: Option<String>,
        message: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct CloneArgs {
        cwd: Option<String>,
        url: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct PathsArgs {
        cwd: Option<String>,
        paths: Vec<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct MvArgs {
        cwd: Option<String>,
        path: String,
        dest: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct GrepArgs {
        cwd: Option<String>,
        pattern: String,
        case_sensitive: Option<bool>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct ShowArgs {
        cwd: Option<String>,
        revision: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct BisectSchema {
        cwd: Option<String>,
        action: String,
        revision: Option<String>,
        bad: Option<String>,
        good: Option<Vec<String>>,
    }

    match operation {
        GitOperation::Commit => {
            let args: CommitArgs = parse_args(value)?;
            validate_commit_message(&args.message)?;
            Ok((
                args.cwd,
                GitArgs::Commit {
                    message: args.message,
                },
            ))
        }
        GitOperation::Clone => {
            let args: CloneArgs = parse_args(value)?;
            validate_https_remote(&args.url)?;
            Ok((args.cwd, GitArgs::Clone { url: args.url }))
        }
        GitOperation::Add | GitOperation::Restore | GitOperation::Rm => {
            let args: PathsArgs = parse_args(value)?;
            validate_paths(&args.paths)?;
            Ok((args.cwd, GitArgs::Paths { paths: args.paths }))
        }
        GitOperation::Mv => {
            let args: MvArgs = parse_args(value)?;
            validate_top_level_name(&args.path, "path")?;
            validate_top_level_name(&args.dest, "dest")?;
            if args.path == args.dest {
                return Err(HostError::InvalidArgs(
                    "git.mv path and dest must differ".to_string(),
                ));
            }
            Ok((
                args.cwd,
                GitArgs::Mv {
                    path: args.path,
                    dest: args.dest,
                },
            ))
        }
        GitOperation::Grep => {
            let args: GrepArgs = parse_args(value)?;
            validate_pattern(&args.pattern)?;
            Ok((
                args.cwd,
                GitArgs::Grep {
                    pattern: args.pattern,
                    case_sensitive: args.case_sensitive.unwrap_or(true),
                },
            ))
        }
        GitOperation::Show => {
            let args: ShowArgs = parse_args(value)?;
            if let Some(revision) = &args.revision {
                validate_full_oid(revision, "revision")?;
            }
            Ok((
                args.cwd,
                GitArgs::Show {
                    revision: args.revision.unwrap_or_else(|| "HEAD".to_string()),
                },
            ))
        }
        GitOperation::Bisect => {
            let args: BisectSchema = parse_args(value)?;
            let bisect = match args.action.as_str() {
                "start" => {
                    if args.revision.is_some() {
                        return Err(HostError::InvalidArgs(
                            "git.bisect start does not accept revision".to_string(),
                        ));
                    }
                    let bad = args.bad.ok_or_else(|| {
                        HostError::InvalidArgs("git.bisect start requires bad".to_string())
                    })?;
                    let good = args.good.ok_or_else(|| {
                        HostError::InvalidArgs("git.bisect start requires good".to_string())
                    })?;
                    validate_full_oid(&bad, "bad")?;
                    if good.is_empty() || good.len() > MAX_BISECT_GOOD_REVISIONS {
                        return Err(HostError::InvalidArgs(format!(
                            "git.bisect good must contain 1..={MAX_BISECT_GOOD_REVISIONS} full commit OIDs"
                        )));
                    }
                    for oid in &good {
                        validate_full_oid(oid, "good")?;
                    }
                    BisectArgs::Start { bad, good }
                }
                "good" | "bad" | "skip" => {
                    if args.bad.is_some() || args.good.is_some() {
                        return Err(HostError::InvalidArgs(format!(
                            "git.bisect {} does not accept bad or good",
                            args.action
                        )));
                    }
                    if let Some(revision) = &args.revision {
                        validate_full_oid(revision, "revision")?;
                    }
                    let action = match args.action.as_str() {
                        "good" => "good",
                        "bad" => "bad",
                        _ => "skip",
                    };
                    BisectArgs::Mark {
                        action,
                        revision: args.revision,
                    }
                }
                "reset" => {
                    if args.revision.is_some() || args.bad.is_some() || args.good.is_some() {
                        return Err(HostError::InvalidArgs(
                            "git.bisect reset accepts only cwd and action".to_string(),
                        ));
                    }
                    BisectArgs::Reset
                }
                _ => {
                    return Err(HostError::InvalidArgs(
                        "git.bisect action must be start, good, bad, skip, or reset".to_string(),
                    ));
                }
            };
            Ok((args.cwd, GitArgs::Bisect(bisect)))
        }
        _ => {
            let args: CwdArgs = parse_args(value)?;
            Ok((args.cwd, GitArgs::Cwd))
        }
    }
}

#[async_trait]
impl HostFn for GitReadFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        if !ctx.grants.permits(self.operation.capability()) {
            return Err(HostError::CapabilityDenied(
                self.operation.capability().to_string(),
            ));
        }
        let (cwd, mut git_args) = parse_git_args(self.operation, args)?;
        let revision = self.linked.revision();
        let requested_cwd = self.linked.resolve_spec(cwd.as_deref())?;
        ctx.require_linked_alias(&requested_cwd.alias)?;
        if self.operation.requires_rw() {
            ensure_rw(&requested_cwd.policy, &requested_cwd.display)?;
        }
        let stable_cwd_path = display_path(&requested_cwd.alias, &requested_cwd.relative);
        let initial_cwd = self
            .linked
            .with_stable_policy_snapshot(revision, |linked| {
                let cwd = linked.resolve_spec(Some(&stable_cwd_path))?;
                if self.operation.requires_rw() {
                    ensure_rw(&cwd.policy, &cwd.display)?;
                }
                let handle =
                    open_existing(&cwd.policy, cwd.root_identity, &cwd.relative, &cwd.display)?;
                if handle.kind != SecureKind::Directory {
                    return Err(HostError::InvalidPath(format!(
                        "{} is not a directory",
                        cwd.display
                    )));
                }
                Ok(handle)
            })?;
        let initial_cwd_identity = initial_cwd.identity;
        let raw_path = std::env::var_os("PATH").unwrap_or_default();
        let sanitized_path = sanitize_path(&raw_path)?;
        let (resolved_git, git_target, git_identity) = resolve_executable("git", &sanitized_path)?;
        #[cfg(unix)]
        let pinned_git = pin_git(&git_target, git_identity)?;
        let linked_root = requested_cwd.policy.root.canonicalize().map_err(|error| {
            HostError::InvalidPath(format!("linked root cannot be canonicalized: {error}"))
        })?;

        if self.operation.requires_repository() {
            self.validate_repository(
                revision,
                &stable_cwd_path,
                initial_cwd_identity,
                &linked_root,
                &sanitized_path,
                &resolved_git,
                &git_target,
                git_identity,
                #[cfg(unix)]
                &pinned_git,
            )
            .await?;
        } else {
            self.validate_pre_repository_cwd(revision, &stable_cwd_path, initial_cwd_identity)?;
        }
        if matches!(self.operation, GitOperation::Add | GitOperation::Restore) {
            self.reject_local_filters(
                revision,
                &stable_cwd_path,
                initial_cwd_identity,
                &sanitized_path,
                &resolved_git,
                &git_target,
                git_identity,
                #[cfg(unix)]
                &pinned_git,
            )
            .await?;
        }
        if let GitArgs::Bisect(args) = &mut git_args {
            self.prepare_bisect_args(
                args,
                revision,
                &stable_cwd_path,
                initial_cwd_identity,
                &sanitized_path,
                &resolved_git,
                &git_target,
                git_identity,
                #[cfg(unix)]
                &pinned_git,
            )
            .await?;
        }
        if let GitArgs::Show { revision: selected } = &mut git_args {
            let resolved = self
                .probe(
                    revision,
                    &stable_cwd_path,
                    initial_cwd_identity,
                    &sanitized_path,
                    &resolved_git,
                    &git_target,
                    git_identity,
                    #[cfg(unix)]
                    &pinned_git,
                    &["rev-parse", "--verify", &format!("{selected}^{{object}}")],
                )
                .await?;
            validate_full_oid(&resolved, "revision")?;
            *selected = resolved;
        }

        let upstream = if matches!(self.operation, GitOperation::Push | GitOperation::Pull) {
            Some(
                self.resolve_upstream(
                    revision,
                    &stable_cwd_path,
                    initial_cwd_identity,
                    &sanitized_path,
                    &resolved_git,
                    &git_target,
                    git_identity,
                    #[cfg(unix)]
                    &pinned_git,
                )
                .await?,
            )
        } else {
            None
        };
        let approval_state =
            if self.operation.requires_approval() && self.operation.requires_repository() {
                Some(
                    self.approval_state(
                        revision,
                        &stable_cwd_path,
                        initial_cwd_identity,
                        &linked_root,
                        &sanitized_path,
                        &resolved_git,
                        &git_target,
                        git_identity,
                        #[cfg(unix)]
                        &pinned_git,
                        upstream.clone(),
                    )
                    .await?,
                )
            } else {
                None
            };
        let command_args = operation_args(self.operation, &git_args, upstream.as_ref())?;

        if self.operation.requires_approval() {
            let message_details = match &git_args {
                GitArgs::Commit { message } => {
                    let redacted = tm_memory::redact_dream_text(message).text;
                    Some(
                        serde_json::json!({"sha256": hex::encode(Sha256::digest(message.as_bytes())), "bytes": message.len(), "preview": bounded_preview(&redacted, 512)}),
                    )
                }
                _ => None,
            };
            let approval = approval_action(
                self.operation.capability(),
                serde_json::json!({
                    "cwd": stable_cwd_path,
                    "fixedArgv": approval_argv(&command_args, matches!(git_args, GitArgs::Commit { .. })),
                    "message": message_details,
                    "upstream": upstream.as_ref().map(|upstream| serde_json::json!({"url": upstream.remote_url, "remoteRef": upstream.remote_ref, "localBranch": upstream.branch})),
                    "network": matches!(self.operation, GitOperation::Clone | GitOperation::Push | GitOperation::Pull),
                    "state": approval_state.as_ref().map(|state| serde_json::json!({"repositoryRoot": state.repository_root, "gitDir": state.git_dir, "head": state.head, "indexSha256": state.index_sha256})),
                }),
            );
            ctx.require_approval(&approval).await?;
        }

        let _mutation_guard = if self.operation.mutates_local() {
            Some(self.linked.lock_mutations().await)
        } else {
            None
        };
        if self.operation.requires_repository() {
            self.validate_repository(
                revision,
                &stable_cwd_path,
                initial_cwd_identity,
                &linked_root,
                &sanitized_path,
                &resolved_git,
                &git_target,
                git_identity,
                #[cfg(unix)]
                &pinned_git,
            )
            .await?;
        } else {
            self.validate_pre_repository_cwd(revision, &stable_cwd_path, initial_cwd_identity)?;
        }
        if matches!(self.operation, GitOperation::Add | GitOperation::Restore) {
            self.reject_local_filters(
                revision,
                &stable_cwd_path,
                initial_cwd_identity,
                &sanitized_path,
                &resolved_git,
                &git_target,
                git_identity,
                #[cfg(unix)]
                &pinned_git,
            )
            .await?;
        }
        if let Some(expected) = approval_state {
            let fresh_upstream =
                if matches!(self.operation, GitOperation::Push | GitOperation::Pull) {
                    Some(
                        self.resolve_upstream(
                            revision,
                            &stable_cwd_path,
                            initial_cwd_identity,
                            &sanitized_path,
                            &resolved_git,
                            &git_target,
                            git_identity,
                            #[cfg(unix)]
                            &pinned_git,
                        )
                        .await?,
                    )
                } else {
                    None
                };
            let actual = self
                .approval_state(
                    revision,
                    &stable_cwd_path,
                    initial_cwd_identity,
                    &linked_root,
                    &sanitized_path,
                    &resolved_git,
                    &git_target,
                    git_identity,
                    #[cfg(unix)]
                    &pinned_git,
                    fresh_upstream,
                )
                .await?;
            if actual != expected {
                return Err(HostError::InvalidArgs(
                    "git repository state changed while approval was pending; retry".to_string(),
                ));
            }
        }
        let output = self
            .run_git(
                revision,
                &stable_cwd_path,
                initial_cwd_identity,
                &sanitized_path,
                &resolved_git,
                &git_target,
                git_identity,
                #[cfg(unix)]
                &pinned_git,
                &command_args,
                self.operation.timeout(),
            )
            .await?;
        if self.operation == GitOperation::Clone && output.exit_code == 0 {
            self.validate_repository(
                revision,
                &stable_cwd_path,
                initial_cwd_identity,
                &linked_root,
                &sanitized_path,
                &resolved_git,
                &git_target,
                git_identity,
                #[cfg(unix)]
                &pinned_git,
            )
            .await?;
        }
        self.shape_result(output).await
    }
}

impl GitReadFn {
    #[allow(clippy::too_many_arguments)]
    async fn validate_repository(
        &self,
        revision: u64,
        stable_cwd_path: &str,
        initial_cwd_identity: crate::linked::secure_fs::FileIdentity,
        linked_root: &Path,
        sanitized_path: &std::ffi::OsStr,
        resolved_git: &Path,
        git_target: &Path,
        git_identity: (u64, u64),
        #[cfg(unix)] pinned_git: &PinnedGit,
    ) -> Result<(String, String)> {
        let repository_root = self
            .probe(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &["rev-parse", "--show-toplevel"],
            )
            .await?;
        let git_dir = self
            .probe(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &["rev-parse", "--absolute-git-dir"],
            )
            .await?;
        for (label, value) in [("worktree", &repository_root), ("git directory", &git_dir)] {
            let canonical = Path::new(value).canonicalize().map_err(|error| {
                HostError::InvalidPath(format!("git {label} cannot be canonicalized: {error}"))
            })?;
            if !canonical.starts_with(linked_root) {
                return Err(HostError::CapabilityDenied(format!(
                    "git {label} is outside the linked root"
                )));
            }
        }
        Ok((repository_root, git_dir))
    }

    fn validate_pre_repository_cwd(
        &self,
        revision: u64,
        stable_cwd_path: &str,
        initial_cwd_identity: crate::linked::secure_fs::FileIdentity,
    ) -> Result<()> {
        self.linked.with_stable_policy_snapshot(revision, |linked| {
            let cwd = linked.resolve_spec(Some(stable_cwd_path))?;
            ensure_rw(&cwd.policy, &cwd.display)?;
            let handle =
                open_existing(&cwd.policy, cwd.root_identity, &cwd.relative, &cwd.display)?;
            if handle.kind != SecureKind::Directory || handle.identity != initial_cwd_identity {
                return Err(HostError::InvalidArgs(
                    "git cwd changed while approval was pending; retry".to_string(),
                ));
            }
            let path = cwd.policy.root.join(&cwd.relative);
            if path.join(".git").exists() {
                return Err(HostError::InvalidArgs(format!(
                    "{} is already a git repository",
                    cwd.display
                )));
            }
            if self.operation == GitOperation::Clone {
                let mut entries = std::fs::read_dir(&path).map_err(|error| {
                    HostError::InvalidPath(format!("{} cannot be inspected: {error}", cwd.display))
                })?;
                if entries
                    .next()
                    .transpose()
                    .map_err(|error| {
                        HostError::InvalidPath(format!(
                            "{} cannot be inspected: {error}",
                            cwd.display
                        ))
                    })?
                    .is_some()
                {
                    return Err(HostError::InvalidArgs(
                        "git.clone cwd must be empty".to_string(),
                    ));
                }
            }
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn reject_local_filters(
        &self,
        revision: u64,
        stable_cwd_path: &str,
        initial_cwd_identity: crate::linked::secure_fs::FileIdentity,
        sanitized_path: &std::ffi::OsStr,
        resolved_git: &Path,
        git_target: &Path,
        git_identity: (u64, u64),
        #[cfg(unix)] pinned_git: &PinnedGit,
    ) -> Result<()> {
        let configured = self
            .probe_optional(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &[
                    "config",
                    "--local",
                    "--no-includes",
                    "--get-regexp",
                    "^(include\\.|includeif\\.|filter\\..*\\.(clean|smudge|process|required)$)",
                ],
            )
            .await?;
        if configured.is_some() {
            return Err(HostError::CapabilityDenied(
                "git.add/restore rejects repository-local includes and clean, smudge, process, or required filters"
                    .to_string(),
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_bisect_args(
        &self,
        args: &mut BisectArgs,
        revision: u64,
        stable_cwd_path: &str,
        initial_cwd_identity: crate::linked::secure_fs::FileIdentity,
        sanitized_path: &std::ffi::OsStr,
        resolved_git: &Path,
        git_target: &Path,
        git_identity: (u64, u64),
        #[cfg(unix)] pinned_git: &PinnedGit,
    ) -> Result<()> {
        match args {
            BisectArgs::Start { .. } => {
                if self
                    .probe_optional(
                        revision,
                        stable_cwd_path,
                        initial_cwd_identity,
                        sanitized_path,
                        resolved_git,
                        git_target,
                        git_identity,
                        #[cfg(unix)]
                        pinned_git,
                        &["rev-parse", "--verify", "BISECT_HEAD"],
                    )
                    .await?
                    .is_some()
                {
                    return Err(HostError::InvalidArgs(
                        "git.bisect start requires no active bisect".to_string(),
                    ));
                }
            }
            BisectArgs::Mark {
                revision: selected, ..
            } => {
                let head = self
                    .probe_optional(
                        revision,
                        stable_cwd_path,
                        initial_cwd_identity,
                        sanitized_path,
                        resolved_git,
                        git_target,
                        git_identity,
                        #[cfg(unix)]
                        pinned_git,
                        &["rev-parse", "--verify", "BISECT_HEAD^{commit}"],
                    )
                    .await?
                    .ok_or_else(|| {
                        HostError::InvalidArgs(
                            "git.bisect mark requires an active no-checkout bisect".to_string(),
                        )
                    })?;
                validate_full_oid(&head, "BISECT_HEAD")?;
                if selected.is_none() {
                    *selected = Some(head);
                }
            }
            BisectArgs::Reset => {
                if self
                    .probe_optional(
                        revision,
                        stable_cwd_path,
                        initial_cwd_identity,
                        sanitized_path,
                        resolved_git,
                        git_target,
                        git_identity,
                        #[cfg(unix)]
                        pinned_git,
                        &["rev-parse", "--verify", "BISECT_HEAD^{commit}"],
                    )
                    .await?
                    .is_none()
                {
                    return Err(HostError::InvalidArgs(
                        "git.bisect reset requires an active no-checkout bisect".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    async fn index_state_sha256(
        &self,
        revision: u64,
        stable_cwd_path: &str,
        initial_cwd_identity: crate::linked::secure_fs::FileIdentity,
        linked_root: &Path,
        sanitized_path: &std::ffi::OsStr,
        resolved_git: &Path,
        git_target: &Path,
        git_identity: (u64, u64),
        #[cfg(unix)] pinned_git: &PinnedGit,
    ) -> Result<Option<String>> {
        let Some(index_path) = self
            .probe_optional(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &["rev-parse", "--path-format=absolute", "--git-path", "index"],
            )
            .await?
        else {
            return Ok(None);
        };
        let index_path = Path::new(&index_path);
        if !index_path.exists() {
            return Ok(None);
        }
        let canonical = index_path.canonicalize().map_err(|error| {
            HostError::InvalidPath(format!("git index cannot be canonicalized: {error}"))
        })?;
        if !canonical.starts_with(linked_root) {
            return Err(HostError::CapabilityDenied(
                "git index is outside the linked root".to_string(),
            ));
        }
        let canonical = canonical.to_str().ok_or_else(|| {
            HostError::InvalidPath("git index path is not valid UTF-8".to_string())
        })?;
        let content_id = self
            .probe(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &["hash-object", "--no-filters", "--", canonical],
            )
            .await?;
        Ok(Some(hex::encode(Sha256::digest(content_id.as_bytes()))))
    }

    #[allow(clippy::too_many_arguments)]
    async fn approval_state(
        &self,
        revision: u64,
        stable_cwd_path: &str,
        initial_cwd_identity: crate::linked::secure_fs::FileIdentity,
        linked_root: &Path,
        sanitized_path: &std::ffi::OsStr,
        resolved_git: &Path,
        git_target: &Path,
        git_identity: (u64, u64),
        #[cfg(unix)] pinned_git: &PinnedGit,
        upstream: Option<Upstream>,
    ) -> Result<ApprovalState> {
        let (repository_root, git_dir) = self
            .validate_repository(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                linked_root,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
            )
            .await?;
        let head = self
            .probe_optional(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &["rev-parse", "--verify", "HEAD"],
            )
            .await?;
        let index_sha256 = if matches!(
            self.operation,
            GitOperation::Add
                | GitOperation::Mv
                | GitOperation::Restore
                | GitOperation::Rm
                | GitOperation::Bisect
                | GitOperation::Commit
        ) {
            self.index_state_sha256(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                linked_root,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
            )
            .await?
        } else {
            None
        };
        Ok(ApprovalState {
            repository_root,
            git_dir,
            head,
            index_sha256,
            upstream,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn resolve_upstream(
        &self,
        revision: u64,
        stable_cwd_path: &str,
        initial_cwd_identity: crate::linked::secure_fs::FileIdentity,
        sanitized_path: &std::ffi::OsStr,
        resolved_git: &Path,
        git_target: &Path,
        git_identity: (u64, u64),
        #[cfg(unix)] pinned_git: &PinnedGit,
    ) -> Result<Upstream> {
        let branch = self
            .probe(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
            )
            .await?;
        validate_git_token(&branch, "current branch")?;
        let remote = self
            .probe(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &[
                    "config",
                    "--local",
                    "--get",
                    &format!("branch.{branch}.remote"),
                ],
            )
            .await?;
        if remote == "." {
            return Err(HostError::CapabilityDenied(
                "git push/pull requires an HTTPS remote, not a local repository".to_string(),
            ));
        }
        validate_git_token(&remote, "upstream remote")?;
        let remote_ref = self
            .probe(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &[
                    "config",
                    "--local",
                    "--get",
                    &format!("branch.{branch}.merge"),
                ],
            )
            .await?;
        if !remote_ref.starts_with("refs/heads/") {
            return Err(HostError::CapabilityDenied(format!(
                "git upstream ref must be a branch, got {remote_ref}"
            )));
        }
        validate_git_token(&remote_ref, "upstream ref")?;
        let remote_url = self
            .probe(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &[
                    "config",
                    "--local",
                    "--get",
                    &format!("remote.{remote}.url"),
                ],
            )
            .await?;
        let push_url = self
            .probe_optional(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &[
                    "config",
                    "--local",
                    "--get",
                    &format!("remote.{remote}.pushurl"),
                ],
            )
            .await?;
        if push_url.is_some() {
            return Err(HostError::CapabilityDenied(
                "git remotes with a separate pushurl are not supported".to_string(),
            ));
        }
        let network_overrides = self
            .probe_optional(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &[
                    "config",
                    "--no-includes",
                    "--get-regexp",
                    "^(include\\.|includeif\\.|url\\.|http\\.|https\\.|credential\\.|filter\\.|merge\\..*\\.driver$|diff\\..*\\.(command|textconv)$|core\\.(gitproxy|sshcommand|attributesfile)$|remote\\..*\\.(proxy|uploadpack|receivepack)$)",
                ],
            )
            .await?;
        if network_overrides.is_some() {
            return Err(HostError::CapabilityDenied(
                "git repository-local URL, HTTP, credential, proxy, or SSH overrides are not supported"
                    .to_string(),
            ));
        }
        validate_https_remote(&remote_url)?;
        Ok(Upstream {
            remote_url,
            remote_ref,
            branch,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn probe(
        &self,
        revision: u64,
        stable_cwd_path: &str,
        initial_cwd_identity: crate::linked::secure_fs::FileIdentity,
        sanitized_path: &std::ffi::OsStr,
        resolved_git: &Path,
        git_target: &Path,
        git_identity: (u64, u64),
        #[cfg(unix)] pinned_git: &PinnedGit,
        args: &[&str],
    ) -> Result<String> {
        self.probe_optional(
            revision,
            stable_cwd_path,
            initial_cwd_identity,
            sanitized_path,
            resolved_git,
            git_target,
            git_identity,
            #[cfg(unix)]
            pinned_git,
            args,
        )
        .await?
        .ok_or_else(|| HostError::InvalidArgs(format!("git probe returned no value: {args:?}")))
    }

    #[allow(clippy::too_many_arguments)]
    async fn probe_optional(
        &self,
        revision: u64,
        stable_cwd_path: &str,
        initial_cwd_identity: crate::linked::secure_fs::FileIdentity,
        sanitized_path: &std::ffi::OsStr,
        resolved_git: &Path,
        git_target: &Path,
        git_identity: (u64, u64),
        #[cfg(unix)] pinned_git: &PinnedGit,
        args: &[&str],
    ) -> Result<Option<String>> {
        let args = common_args(false)
            .into_iter()
            .chain(args.iter().map(|arg| (*arg).to_string()))
            .collect::<Vec<_>>();
        let output = self
            .run_git(
                revision,
                stable_cwd_path,
                initial_cwd_identity,
                sanitized_path,
                resolved_git,
                git_target,
                git_identity,
                #[cfg(unix)]
                pinned_git,
                &args,
                LOCAL_GIT_TIMEOUT,
            )
            .await?;
        if output.exit_code != 0
            && output.stdout.trim().is_empty()
            && matches!(output.exit_code, 1 | 128)
        {
            return Ok(None);
        }
        if output.exit_code != 0 {
            return Err(HostError::InvalidArgs(format!(
                "git upstream probe failed: {}",
                bounded_preview(output.stderr.trim(), 512)
            )));
        }
        let value = output.stdout.trim();
        if value.is_empty() {
            Ok(None)
        } else if value.len() > MAX_PROBE_OUTPUT_BYTES || value.contains('\0') {
            Err(HostError::InvalidArgs(
                "git upstream probe returned an invalid value".to_string(),
            ))
        } else {
            Ok(Some(value.to_string()))
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_git(
        &self,
        revision: u64,
        stable_cwd_path: &str,
        initial_cwd_identity: crate::linked::secure_fs::FileIdentity,
        sanitized_path: &std::ffi::OsStr,
        resolved_git: &Path,
        git_target: &Path,
        git_identity: (u64, u64),
        #[cfg(unix)] _pinned_git: &PinnedGit,
        args: &[String],
        timeout: Duration,
    ) -> Result<RawGitOutput> {
        let start = Instant::now();
        let (mut child, executed_cwd) =
            self.linked
                .with_stable_policy_snapshot(revision, |linked| {
                    let fresh_cwd = linked.resolve_spec(Some(stable_cwd_path))?;
                    if self.operation.requires_rw() {
                        ensure_rw(&fresh_cwd.policy, &fresh_cwd.display)?;
                    }
                    let fresh_handle = open_existing(
                        &fresh_cwd.policy,
                        fresh_cwd.root_identity,
                        &fresh_cwd.relative,
                        &fresh_cwd.display,
                    )?;
                    if fresh_handle.kind != SecureKind::Directory
                        || fresh_handle.identity != initial_cwd_identity
                    {
                        return Err(HostError::InvalidArgs(format!(
                            "git cwd {} changed while the operation was prepared; retry",
                            fresh_cwd.display
                        )));
                    }
                    let (fresh_git, fresh_target, fresh_identity) =
                        resolve_executable("git", sanitized_path)?;
                    if fresh_git != resolved_git
                        || fresh_target != git_target
                        || fresh_identity != git_identity
                    {
                        return Err(HostError::InvalidArgs(
                            "git executable changed while the operation was prepared; retry"
                                .to_string(),
                        ));
                    }

                    #[cfg(target_os = "linux")]
                    let executable_file = _pinned_git.file.try_clone().map_err(|error| {
                        HostError::HostCall(format!("failed to clone pinned git: {error}"))
                    })?;
                    #[cfg(unix)]
                    let mut command = {
                        #[cfg(target_os = "linux")]
                        let mut command = tokio::process::Command::new(format!(
                            "/proc/self/fd/{}",
                            std::os::fd::AsRawFd::as_raw_fd(&executable_file)
                        ));
                        #[cfg(not(target_os = "linux"))]
                        let mut command = tokio::process::Command::new(&fresh_target);
                        command.arg0("git");
                        command
                    };
                    #[cfg(not(unix))]
                    let mut command = tokio::process::Command::new(&fresh_git);

                    command
                        .args(args)
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .kill_on_drop(true)
                        .env_clear()
                        .env("PATH", sanitized_path)
                        .env("GIT_CONFIG_NOSYSTEM", "1")
                        .env("GIT_CONFIG_GLOBAL", "/dev/null")
                        .env("GIT_TERMINAL_PROMPT", "0")
                        .env("GIT_ASKPASS", "false")
                        .env("SSH_ASKPASS", "false")
                        .env("GIT_OPTIONAL_LOCKS", "0")
                        .env("GIT_EDITOR", "true")
                        .env("GIT_SEQUENCE_EDITOR", "true")
                        .env("GIT_MERGE_AUTOEDIT", "no")
                        .env("LC_ALL", "C");

                    #[cfg(unix)]
                    {
                        #[cfg(target_os = "linux")]
                        use std::os::fd::AsRawFd;
                        #[cfg(not(target_os = "linux"))]
                        use std::os::unix::ffi::OsStrExt;

                        command.process_group(0);
                        let cwd_file = fresh_handle.file;
                        #[cfg(not(target_os = "linux"))]
                        let executable_path = std::ffi::CString::new(
                            fresh_target.as_os_str().as_bytes(),
                        )
                        .map_err(|_| {
                            HostError::HostCall(
                                "git executable path contains a NUL byte".to_string(),
                            )
                        })?;
                        unsafe {
                            command.pre_exec(move || {
                                rustix::process::fchdir(&cwd_file).map_err(|error| {
                                    std::io::Error::from_raw_os_error(error.raw_os_error())
                                })?;
                                #[cfg(target_os = "linux")]
                                {
                                    let flags =
                                        libc::fcntl(executable_file.as_raw_fd(), libc::F_GETFD);
                                    if flags < 0
                                        || libc::fcntl(
                                            executable_file.as_raw_fd(),
                                            libc::F_SETFD,
                                            flags & !libc::FD_CLOEXEC,
                                        ) < 0
                                    {
                                        return Err(std::io::Error::last_os_error());
                                    }
                                }
                                #[cfg(not(target_os = "linux"))]
                                {
                                    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
                                    if libc::stat(executable_path.as_ptr(), stat.as_mut_ptr()) != 0
                                    {
                                        return Err(std::io::Error::last_os_error());
                                    }
                                    #[allow(clippy::unnecessary_cast)]
                                    let actual = (
                                        (*stat.as_ptr()).st_dev as u64,
                                        (*stat.as_ptr()).st_ino as u64,
                                    );
                                    if actual != fresh_identity {
                                        return Err(std::io::Error::from_raw_os_error(
                                            libc::ESTALE,
                                        ));
                                    }
                                }
                                Ok(())
                            });
                        }
                    }
                    #[cfg(not(unix))]
                    command.current_dir(fresh_cwd.policy.root.join(&fresh_cwd.relative));

                    let child = command.spawn().map_err(|error| {
                        HostError::HostCall(format!("git spawn failed: {error}"))
                    })?;
                    Ok((child, display_path(&fresh_cwd.alias, &fresh_cwd.relative)))
                })?;

        let mut process_group = ProcessGroupGuard::new(child.id());
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HostError::HostCall("git stdout pipe missing".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| HostError::HostCall("git stderr pipe missing".to_string()))?;
        let retained = Arc::new(AtomicUsize::new(0));
        let run = async {
            tokio::join!(
                child.wait(),
                read_bounded_output(
                    stdout,
                    Arc::clone(&retained),
                    MAX_RETAINED_PROCESS_OUTPUT_BYTES,
                ),
                read_bounded_output(
                    stderr,
                    Arc::clone(&retained),
                    MAX_RETAINED_PROCESS_OUTPUT_BYTES,
                ),
            )
        };
        let (status, stdout, stderr) = match tokio::time::timeout(timeout, run).await {
            Ok((Ok(status), Ok(stdout), Ok(stderr))) => {
                process_group.disarm();
                (status, stdout, stderr)
            }
            Ok((Err(error), _, _)) | Ok((_, Err(error), _)) | Ok((_, _, Err(error))) => {
                return Err(HostError::HostCall(error.to_string()));
            }
            Err(_) => {
                stop_process_tree(&mut child, &mut process_group)
                    .await
                    .map_err(|error| HostError::HostCall(error.to_string()))?;
                return Err(HostError::Timeout(self.operation.capability().to_string()));
            }
        };
        let output_limit_reached = stdout.truncated || stderr.truncated;
        let stdout = tm_memory::redact_dream_text(&String::from_utf8_lossy(&stdout.bytes)).text;
        let stderr = tm_memory::redact_dream_text(&String::from_utf8_lossy(&stderr.bytes)).text;
        Ok(RawGitOutput {
            exit_code: status.code().unwrap_or(-1),
            stdout,
            stderr,
            output_limit_reached,
            cwd: executed_cwd,
            duration_ms: start.elapsed().as_millis(),
        })
    }

    async fn shape_result(&self, output: RawGitOutput) -> Result<Value> {
        let combined_len = output.stdout.len().saturating_add(output.stderr.len());
        let (stdout, stderr, truncated, artifact) =
            if output.output_limit_reached || combined_len > INLINE_OUTPUT_BYTES {
                let persisted = bounded_git_artifact(
                    format!("{}{}", output.stdout, output.stderr),
                    output.output_limit_reached,
                );
                let store = self.artifact_store.clone();
                let title = self.operation.capability().to_string();
                let artifact = tokio::task::spawn_blocking(move || {
                    store
                        .put_text(&persisted, Some(title), "text/plain")
                        .map_err(|error| HostError::HostCall(error.to_string()))
                })
                .await
                .map_err(|error| {
                    HostError::HostCall(format!("git artifact worker failed: {error}"))
                })??;
                let (stdout, stderr) =
                    bounded_inline_output(&output.stdout, &output.stderr, INLINE_OUTPUT_BYTES);
                (stdout, stderr, true, Some(artifact))
            } else {
                (output.stdout, output.stderr, false, None)
            };
        serde_json::to_value(GitResult {
            operation: self.operation.capability(),
            cwd: output.cwd,
            exit_code: output.exit_code,
            stdout,
            stderr,
            truncated,
            artifact,
            duration_ms: output.duration_ms,
        })
        .map_err(|error| HostError::HostCall(error.to_string()))
    }
}

fn common_args(optional_locks: bool) -> Vec<String> {
    let mut args = vec!["--no-pager".to_string()];
    if !optional_locks {
        args.push("--no-optional-locks".to_string());
    }
    for (key, value) in [
        ("core.hooksPath", "/dev/null"),
        ("core.fsmonitor", "false"),
        ("diff.external", ""),
        ("credential.helper", ""),
        ("core.askPass", ""),
        ("commit.gpgSign", "false"),
        ("tag.gpgSign", "false"),
        ("push.gpgSign", "false"),
        ("protocol.allow", "never"),
        ("protocol.https.allow", "always"),
        ("http.followRedirects", "false"),
        ("submodule.recurse", "false"),
        ("fetch.recurseSubmodules", "false"),
        ("pull.rebase", "false"),
        ("merge.autoStash", "false"),
    ] {
        args.extend(["-c".to_string(), format!("{key}={value}")]);
    }
    args
}

fn operation_args(
    operation: GitOperation,
    git_args: &GitArgs,
    upstream: Option<&Upstream>,
) -> Result<Vec<String>> {
    let mut args = common_args(matches!(
        operation,
        GitOperation::Clone
            | GitOperation::Init
            | GitOperation::Add
            | GitOperation::Mv
            | GitOperation::Restore
            | GitOperation::Rm
            | GitOperation::Bisect
            | GitOperation::Commit
            | GitOperation::Pull
    ));
    if matches!(
        operation,
        GitOperation::Add
            | GitOperation::Mv
            | GitOperation::Restore
            | GitOperation::Rm
            | GitOperation::Grep
            | GitOperation::Show
    ) {
        args.push("--literal-pathspecs".to_string());
    }
    match (operation, git_args) {
        (GitOperation::Status, GitArgs::Cwd) => args.extend(
            [
                "-c",
                "pager.status=false",
                "status",
                "--short",
                "--branch",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ]
            .map(str::to_string),
        ),
        (GitOperation::Diff, GitArgs::Cwd) => args.extend(
            [
                "-c",
                "pager.diff=false",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=all",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ]
            .map(str::to_string),
        ),
        (
            GitOperation::Grep,
            GitArgs::Grep {
                pattern,
                case_sensitive,
            },
        ) => {
            args.extend(
                [
                    "grep",
                    "--no-ext-grep",
                    "--line-number",
                    "--column",
                    "--full-name",
                    "-F",
                ]
                .map(str::to_string),
            );
            if !case_sensitive {
                args.push("-i".to_string());
            }
            args.extend([
                "-e".to_string(),
                pattern.clone(),
                "--".to_string(),
                ".".to_string(),
            ]);
        }
        (GitOperation::Log, GitArgs::Cwd) => args.extend(
            [
                "-c",
                "pager.log=false",
                "log",
                "-n",
                "20",
                "--no-decorate",
                "--date=iso-strict",
                "--format=%H%x09%aI%x09%an%x09%ae%x09%s",
            ]
            .map(str::to_string),
        ),
        (GitOperation::Show, GitArgs::Show { revision }) => args.extend(
            [
                "show",
                "--no-ext-diff",
                "--no-textconv",
                "--no-decorate",
                "--date=iso-strict",
                "--format=%H%x09%aI%x09%an%x09%ae%x09%s",
                "--no-renames",
            ]
            .map(str::to_string)
            .into_iter()
            .chain([revision.clone(), "--".to_string()]),
        ),
        (GitOperation::Clone, GitArgs::Clone { url }) => args.extend([
            "clone".to_string(),
            "--no-tags".to_string(),
            "--no-recurse-submodules".to_string(),
            "--template=".to_string(),
            "--origin=origin".to_string(),
            "--".to_string(),
            url.clone(),
            ".".to_string(),
        ]),
        (GitOperation::Init, GitArgs::Cwd) => args.extend(
            ["init", "--quiet", "--initial-branch=main", "--template="].map(str::to_string),
        ),
        (GitOperation::Add, GitArgs::Paths { paths }) => {
            args.extend(["add".to_string(), "--".to_string()]);
            args.extend(paths.iter().cloned());
        }
        (GitOperation::Mv, GitArgs::Mv { path, dest }) => args.extend([
            "mv".to_string(),
            "--".to_string(),
            path.clone(),
            dest.clone(),
        ]),
        (GitOperation::Restore, GitArgs::Paths { paths }) => {
            args.extend([
                "restore".to_string(),
                "--worktree".to_string(),
                "--".to_string(),
            ]);
            args.extend(paths.iter().cloned());
        }
        (GitOperation::Rm, GitArgs::Paths { paths }) => {
            args.extend(["rm".to_string(), "--".to_string()]);
            args.extend(paths.iter().cloned());
        }
        (GitOperation::Bisect, GitArgs::Bisect(BisectArgs::Start { bad, good })) => {
            args.extend([
                "bisect".to_string(),
                "start".to_string(),
                "--no-checkout".to_string(),
                bad.clone(),
            ]);
            args.extend(good.iter().cloned());
            args.push("--".to_string());
        }
        (
            GitOperation::Bisect,
            GitArgs::Bisect(BisectArgs::Mark {
                action,
                revision: Some(revision),
            }),
        ) => args.extend([
            "bisect".to_string(),
            (*action).to_string(),
            revision.clone(),
        ]),
        (GitOperation::Bisect, GitArgs::Bisect(BisectArgs::Mark { revision: None, .. })) => {
            return Err(HostError::InvalidArgs(
                "git.bisect mark revision was not materialized".to_string(),
            ));
        }
        (GitOperation::Bisect, GitArgs::Bisect(BisectArgs::Reset)) => {
            args.extend(["bisect".to_string(), "reset".to_string()])
        }
        (GitOperation::Commit, GitArgs::Commit { message }) => args.extend([
            "commit".to_string(),
            "--no-verify".to_string(),
            "--no-gpg-sign".to_string(),
            "--cleanup=verbatim".to_string(),
            "-m".to_string(),
            message.clone(),
        ]),
        (GitOperation::Push, GitArgs::Cwd) => {
            let upstream = upstream.ok_or_else(|| {
                HostError::InvalidArgs("git.push requires an upstream branch".to_string())
            })?;
            args.extend([
                "push".to_string(),
                "--porcelain".to_string(),
                upstream.remote_url.clone(),
                format!("HEAD:{}", upstream.remote_ref),
            ]);
        }
        (GitOperation::Pull, GitArgs::Cwd) => {
            let upstream = upstream.ok_or_else(|| {
                HostError::InvalidArgs("git.pull requires an upstream branch".to_string())
            })?;
            let branch = upstream
                .remote_ref
                .strip_prefix("refs/heads/")
                .ok_or_else(|| HostError::InvalidArgs("invalid upstream branch".to_string()))?;
            args.extend([
                "pull".to_string(),
                "--ff-only".to_string(),
                "--no-rebase".to_string(),
                "--no-edit".to_string(),
                upstream.remote_url.clone(),
                branch.to_string(),
            ]);
        }
        _ => {
            return Err(HostError::InvalidArgs(format!(
                "invalid arguments for {}",
                operation.capability()
            )));
        }
    }
    Ok(args)
}

fn approval_argv(args: &[String], redact_last: bool) -> Vec<String> {
    if !redact_last {
        return args.to_vec();
    }
    let mut args = args.to_vec();
    if let Some(last) = args.last_mut() {
        *last = "[commit message; see digest/preview]".to_string();
    }
    args
}

fn validate_commit_message(message: &str) -> Result<()> {
    if message.trim().is_empty()
        || message.len() > MAX_COMMIT_MESSAGE_BYTES
        || message.contains('\0')
        || message
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return Err(HostError::InvalidArgs(format!(
            "git.commit message must be non-empty UTF-8, contain no NUL/control characters, and fit {MAX_COMMIT_MESSAGE_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_pattern(pattern: &str) -> Result<()> {
    if pattern.is_empty() || pattern.len() > MAX_PATTERN_BYTES || pattern.contains('\0') {
        return Err(HostError::InvalidArgs(format!(
            "git.grep pattern must be non-empty, contain no NUL, and fit {MAX_PATTERN_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_paths(paths: &[String]) -> Result<()> {
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return Err(HostError::InvalidArgs(format!(
            "git paths must contain 1..={MAX_PATHS} entries"
        )));
    }
    if paths.iter().map(String::len).sum::<usize>() > MAX_PATHS_TOTAL_BYTES {
        return Err(HostError::InvalidArgs(format!(
            "git paths must fit {MAX_PATHS_TOTAL_BYTES} aggregate bytes"
        )));
    }
    for path in paths {
        validate_relative_path(path)?;
    }
    Ok(())
}
fn validate_relative_path(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 1024
        || value.starts_with('-')
        || value.contains('\0')
        || Path::new(value).is_absolute()
    {
        return Err(HostError::InvalidArgs(
            "git path must be a relative literal path of at most 1024 bytes and not begin with '-'"
                .to_string(),
        ));
    }
    for component in Path::new(value).components() {
        match component {
            Component::Normal(name) if name != ".git" => {}
            _ => return Err(HostError::InvalidArgs(
                "git path must be normalized and contain no parent/current/root or .git components"
                    .to_string(),
            )),
        }
    }
    Ok(())
}
fn validate_top_level_name(value: &str, label: &str) -> Result<()> {
    validate_relative_path(value)?;
    if Path::new(value).components().count() != 1 {
        return Err(HostError::InvalidArgs(format!(
            "git.mv {label} must be a top-level single-component literal name"
        )));
    }
    Ok(())
}

fn validate_full_oid(value: &str, label: &str) -> Result<()> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(HostError::InvalidArgs(format!(
            "git.bisect {label} must be a full 40- or 64-hex commit OID"
        )));
    }
    Ok(())
}

fn validate_git_token(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('-')
        || value.len() > 1024
        || value.contains('\0')
        || value.chars().any(char::is_whitespace)
    {
        return Err(HostError::InvalidArgs(format!("git {label} is malformed")));
    }
    Ok(())
}

fn validate_https_remote(value: &str) -> Result<()> {
    let url = Url::parse(value)
        .map_err(|_| HostError::CapabilityDenied("git upstream URL is invalid".to_string()))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(HostError::CapabilityDenied(
            "git push/pull requires a credential-free HTTPS upstream URL without query or fragment"
                .to_string(),
        ));
    }
    Ok(())
}

fn bounded_preview(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let marker = format!("…[truncated:{} bytes]", value.len());
    let mut end = limit.saturating_sub(marker.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &value[..end], marker)
}

fn bounded_git_artifact(mut output: String, retained_limit_reached: bool) -> String {
    let marker = if retained_limit_reached || output.len() > MAX_ARTIFACT_BYTES {
        format!(
            "\n… git retained-output limit reached at {MAX_RETAINED_PROCESS_OUTPUT_BYTES} bytes …"
        )
    } else {
        String::new()
    };
    let content_cap = MAX_ARTIFACT_BYTES.saturating_sub(marker.len());
    if output.len() > content_cap {
        let mut end = content_cap;
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
    }
    output.push_str(&marker);
    output
}

#[cfg(unix)]
struct PinnedGit {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    file: File,
}

#[cfg(unix)]
fn pin_git(path: &Path, expected_identity: (u64, u64)) -> Result<PinnedGit> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[cfg(target_os = "linux")]
    let file = {
        use std::os::unix::ffi::OsStrExt;

        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            HostError::HostCall("git executable path contains a NUL byte".to_string())
        })?;
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(HostError::CapabilityDenied(format!(
                "git executable cannot be pinned: {}",
                std::io::Error::last_os_error()
            )));
        }
        unsafe { File::from_raw_fd(fd) }
    };
    #[cfg(not(target_os = "linux"))]
    let file = File::open(path).map_err(|error| {
        HostError::CapabilityDenied(format!(
            "git executable cannot be pinned on this Unix host: {error}"
        ))
    })?;

    let metadata = file.metadata().map_err(|error| {
        HostError::CapabilityDenied(format!("pinned git cannot be inspected: {error}"))
    })?;
    let identity = (metadata.dev(), metadata.ino());
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
        || identity != expected_identity
    {
        return Err(HostError::InvalidArgs(
            "git executable changed while it was being pinned; retry".to_string(),
        ));
    }
    Ok(PinnedGit { file })
}

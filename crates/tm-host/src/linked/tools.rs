use std::{
    collections::BTreeMap,
    env, fs,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tm_artifacts::{ArtifactRef, ArtifactStore, ResourceContent, preview};

use crate::{
    HostError, HostFn, HostRegistry, InvocationCtx, ResourceEntry, ResourceHandler,
    ResourceRegistry, Result, ToolDocs,
};

use super::util::{
    apply_line_hunks, collect_files, compile_globs, display_path, ensure_rw, file_tag, fs_entry,
    linked_uri, list_entries, load_simple_gitignore, parse_args, simple_diff, stdin_present,
    validate_command_name,
};
use super::{LinkedFolders, docs::docs};

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

pub(super) struct FsReadFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsReadFn {
    pub(super) fn new(linked: LinkedFolders) -> Self {
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

pub(super) struct FsWriteFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsWriteFn {
    pub(super) fn new(linked: LinkedFolders) -> Self {
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

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
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
        if existed {
            ctx.require_approval(&format!("fs.write overwrite {}", args.path))
                .await?;
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

pub(super) struct FsLsFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsLsFn {
    pub(super) fn new(linked: LinkedFolders) -> Self {
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

pub(super) struct FsFindFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsFindFn {
    pub(super) fn new(linked: LinkedFolders) -> Self {
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

pub(super) struct CodeSearchFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl CodeSearchFn {
    pub(super) fn new(linked: LinkedFolders) -> Self {
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

pub(super) struct CodeEditFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl CodeEditFn {
    pub(super) fn new(linked: LinkedFolders) -> Self {
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
pub(super) enum PatchHunk {
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
pub(super) enum InsertAt {
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

pub(super) struct ProcRunFn {
    linked: LinkedFolders,
    artifact_store: ArtifactStore,
    docs: ToolDocs,
}

impl ProcRunFn {
    pub(super) fn new(linked: LinkedFolders, artifact_store: ArtifactStore) -> Self {
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

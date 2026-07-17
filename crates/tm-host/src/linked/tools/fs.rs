use super::*;
use std::{
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};

const PATCH_DIFF_PREVIEW_BYTES: usize = 12 * 1024;
const MAX_PATCH_ARTIFACT_BYTES: usize = 4 * 1024 * 1024 - 256;
static PATCH_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(in crate::linked) struct FsReadFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsReadFn {
    pub(in crate::linked) fn new(linked: LinkedFolders) -> Self {
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

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        #[derive(Deserialize)]
        struct Args {
            path: String,
            selector: Option<String>,
            #[allow(dead_code)]
            raw: Option<bool>,
        }
        let args: Args = parse_args(args)?;
        ctx.require_linked_alias(&parse_linked_path(&args.path)?.alias)?;
        serde_json::to_value(
            self.linked
                .read_resource(&args.path, args.selector.as_deref())?,
        )
        .map_err(|err| HostError::HostCall(err.to_string()))
    }
}

pub(in crate::linked) struct FsWriteFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsWriteFn {
    pub(in crate::linked) fn new(linked: LinkedFolders) -> Self {
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
        ctx.require_linked_alias(&parse_linked_path(&args.path)?.alias)?;
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

pub(in crate::linked) struct FsPatchFn {
    linked: LinkedFolders,
    artifact_store: ArtifactStore,
    docs: ToolDocs,
}

impl FsPatchFn {
    pub(in crate::linked) fn new(linked: LinkedFolders, artifact_store: ArtifactStore) -> Self {
        Self {
            linked,
            artifact_store,
            docs: docs("fs.patch", "fs", "Patch an existing UTF-8 file", true),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FsPatchArgs {
    path: String,
    tag: String,
    hunks: Vec<PatchHunk>,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "op",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::linked) enum PatchHunk {
    Replace {
        start_line: usize,
        end_line: usize,
        expected_lines: Vec<String>,
        lines: Vec<String>,
    },
    Delete {
        start_line: usize,
        end_line: usize,
        expected_lines: Vec<String>,
    },
    InsertBefore {
        line: usize,
        expected_line: String,
        lines: Vec<String>,
    },
    InsertAfter {
        line: usize,
        expected_line: String,
        lines: Vec<String>,
    },
    Prepend {
        lines: Vec<String>,
    },
    Append {
        lines: Vec<String>,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsPatchResult {
    path: String,
    changed: bool,
    new_tag: String,
    summary: String,
    diff_preview: String,
    diff_artifact: Option<ArtifactRef>,
    truncated: bool,
}

#[async_trait]
impl HostFn for FsPatchFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        let patch: FsPatchArgs = parse_args(args)?;
        if patch.hunks.is_empty() {
            return Err(HostError::InvalidArgs(
                "fs.patch requires at least one hunk".to_string(),
            ));
        }
        let resolved = self.linked.resolve_existing(Some(&patch.path))?;
        ctx.require_linked_alias(&resolved.alias)?;
        ensure_rw(&resolved.policy, &resolved.display)?;
        if !resolved.path.is_file() {
            return Err(HostError::InvalidPath(format!(
                "{} is not a file",
                resolved.display
            )));
        }
        let old_bytes =
            fs::read(&resolved.path).map_err(|err| HostError::HostCall(err.to_string()))?;
        let actual_tag = file_tag(&old_bytes);
        if patch.tag != actual_tag {
            return Err(HostError::InvalidArgs(format!(
                "stale tag for {}: expected {actual_tag}, got {}",
                patch.path, patch.tag
            )));
        }
        let old = String::from_utf8(old_bytes)
            .map_err(|_| HostError::InvalidArgs("fs.patch supports UTF-8 text only".to_string()))?;
        let new = apply_line_hunks(&old, &patch.hunks)?;
        let changed = old != new;
        let diff = simple_diff(&old, &new, &patch.path);
        let truncated = diff.len() > PATCH_DIFF_PREVIEW_BYTES;
        let diff_artifact = if truncated {
            let persisted_diff = if diff.len() > MAX_PATCH_ARTIFACT_BYTES {
                preview(&diff, MAX_PATCH_ARTIFACT_BYTES)
            } else {
                diff.clone()
            };
            Some(
                self.artifact_store
                    .put_text(
                        &persisted_diff,
                        Some(format!("fs.patch {}", patch.path)),
                        "text/x-diff",
                    )
                    .map_err(|err| HostError::HostCall(err.to_string()))?,
            )
        } else {
            None
        };
        if changed {
            atomic_write(&resolved.path, new.as_bytes())?;
        }
        let new_tag = file_tag(new.as_bytes());
        let result = FsPatchResult {
            path: display_path(&resolved.alias, &resolved.relative),
            changed,
            new_tag,
            summary: format!(
                "{} hunk{} applied",
                patch.hunks.len(),
                if patch.hunks.len() == 1 { "" } else { "s" }
            ),
            diff_preview: if truncated {
                preview(&diff, PATCH_DIFF_PREVIEW_BYTES)
            } else {
                diff
            },
            diff_artifact,
            truncated,
        };
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

pub(in crate::linked) struct FsMoveFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsMoveFn {
    pub(in crate::linked) fn new(linked: LinkedFolders) -> Self {
        Self {
            linked,
            docs: docs("fs.move", "fs", "Move a linked file", true),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsMoveResult {
    path: String,
    dest: String,
    overwritten: bool,
    new_tag: String,
}

#[async_trait]
impl HostFn for FsMoveFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Args {
            path: String,
            dest: String,
            tag: String,
            #[serde(default)]
            create_parents: bool,
            #[serde(default)]
            overwrite: bool,
        }
        let args: Args = parse_args(args)?;
        let source = self.linked.resolve_existing(Some(&args.path))?;
        let source_entry = source.policy.root.join(&source.relative);
        ctx.require_linked_alias(&source.alias)?;
        ensure_rw(&source.policy, &source.display)?;
        if !source_entry.is_file() {
            return Err(HostError::InvalidPath(format!(
                "{} is not a file",
                source.display
            )));
        }
        let source_bytes =
            fs::read(&source_entry).map_err(|err| HostError::HostCall(err.to_string()))?;
        let actual_tag = file_tag(&source_bytes);
        if args.tag != actual_tag {
            return Err(HostError::InvalidArgs(format!(
                "stale tag for {}: expected {actual_tag}, got {}",
                args.path, args.tag
            )));
        }
        let dest_alias = parse_linked_path(&args.dest)?.alias;
        ctx.require_linked_alias(&dest_alias)?;
        if source.alias != dest_alias {
            return Err(HostError::InvalidPath(
                "fs.move destination must stay in the same alias".to_string(),
            ));
        }
        let dest = self
            .linked
            .resolve_for_create(&args.dest, args.create_parents)?;
        let dest_entry = dest.policy.root.join(&dest.relative);
        ensure_rw(&dest.policy, &dest.display)?;
        if source.path == dest.path {
            return Err(HostError::InvalidArgs(
                "fs.move source and destination must differ".to_string(),
            ));
        }
        let overwritten = dest.path.exists();
        if overwritten && !args.overwrite {
            return Err(HostError::InvalidArgs(format!(
                "{} already exists; set overwrite=true",
                dest.display
            )));
        }
        if overwritten {
            ctx.require_approval(&format!("fs.move overwrite {} -> {}", args.path, args.dest))
                .await?;
        }
        fs::rename(&source_entry, &dest_entry)
            .map_err(|err| HostError::HostCall(err.to_string()))?;
        let result = FsMoveResult {
            path: display_path(&source.alias, &source.relative),
            dest: display_path(&dest.alias, &dest.relative),
            overwritten,
            new_tag: actual_tag,
        };
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

pub(in crate::linked) struct FsRemoveFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsRemoveFn {
    pub(in crate::linked) fn new(linked: LinkedFolders) -> Self {
        Self {
            linked,
            docs: docs("fs.remove", "fs", "Remove a linked file", true),
        }
    }
}

#[async_trait]
impl HostFn for FsRemoveFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            path: String,
            tag: String,
        }
        let args: Args = parse_args(args)?;
        let resolved = self.linked.resolve_existing(Some(&args.path))?;
        let remove_path = resolved.policy.root.join(&resolved.relative);
        ctx.require_linked_alias(&resolved.alias)?;
        ensure_rw(&resolved.policy, &resolved.display)?;
        if !remove_path.is_file() {
            return Err(HostError::InvalidPath(format!(
                "{} is not a file",
                resolved.display
            )));
        }
        let bytes = fs::read(&remove_path).map_err(|err| HostError::HostCall(err.to_string()))?;
        let actual_tag = file_tag(&bytes);
        if args.tag != actual_tag {
            return Err(HostError::InvalidArgs(format!(
                "stale tag for {}: expected {actual_tag}, got {}",
                args.path, args.tag
            )));
        }
        ctx.require_approval(&format!("fs.remove {}", args.path))
            .await?;
        fs::remove_file(&remove_path).map_err(|err| HostError::HostCall(err.to_string()))?;
        Ok(serde_json::json!({
            "path": display_path(&resolved.alias, &resolved.relative),
            "removed": true,
        }))
    }
}

fn atomic_write(path: &std::path::Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        HostError::InvalidPath(format!("{} has no parent directory", path.display()))
    })?;
    let sequence = PATCH_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".tm-patch-{}-{sequence}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(data)?;
        file.sync_all()?;
        file.set_permissions(fs::metadata(path)?.permissions())?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|err| HostError::HostCall(err.to_string()))
}

pub(in crate::linked) struct FsLsFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsLsFn {
    pub(in crate::linked) fn new(linked: LinkedFolders) -> Self {
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

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
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
        ctx.require_linked_alias(&resolved.alias)?;
        let entries = list_entries(
            &resolved,
            args.recursive,
            args.limit.unwrap_or(1000),
            args.include_hidden,
        )?;
        serde_json::to_value(entries).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

pub(in crate::linked) struct FsFindFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsFindFn {
    pub(in crate::linked) fn new(linked: LinkedFolders) -> Self {
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

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
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
        ctx.require_linked_alias(&resolved.alias)?;
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

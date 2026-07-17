use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

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
                true,
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
        let linked = self.linked.clone();
        let path = args.path;
        let selector = args.selector;
        let resource =
            tokio::task::spawn_blocking(move || linked.read_resource(&path, selector.as_deref()))
                .await
                .map_err(|err| HostError::HostCall(format!("fs.read worker failed: {err}")))??;
        serde_json::to_value(resource).map_err(|err| HostError::HostCall(err.to_string()))
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
        if args.data.len() > MAX_MUTATION_FILE_BYTES {
            return Err(HostError::InvalidArgs(format!(
                "fs.write data must not exceed {MAX_MUTATION_FILE_BYTES} UTF-8 bytes"
            )));
        }
        ctx.require_linked_alias(&parse_linked_path(&args.path)?.alias)?;
        let _mutation = self.linked.lock_mutations().await;
        let revision = self.linked.revision();
        let resolved = self.linked.resolve_spec(Some(&args.path))?;
        ensure_rw(&resolved.policy, &resolved.display)?;
        let linked = self.linked.clone();
        let preflight_path = args.path.clone();
        let create_parents = args.create_parents;
        let initial = tokio::task::spawn_blocking(move || -> Result<_> {
            linked.with_stable_policy_snapshot(revision, |linked| {
                let resolved = linked.resolve_spec(Some(&preflight_path))?;
                ensure_rw(&resolved.policy, &resolved.display)?;
                let parent = open_parent(
                    &resolved.policy,
                    resolved.root_identity,
                    &resolved.relative,
                    create_parents,
                    &resolved.display,
                )?;
                let snapshot = stat_entry(&parent, &resolved.display)?;
                let state = match snapshot {
                    Some(snapshot) => {
                        if snapshot.kind != SecureKind::File {
                            return Err(HostError::InvalidPath(format!(
                                "{} is not a regular file",
                                resolved.display
                            )));
                        }
                        let file = open_entry_file(&parent, snapshot, &resolved.display)?;
                        let permissions = file
                            .metadata()
                            .map_err(|err| HostError::HostCall(err.to_string()))?
                            .permissions();
                        let tag = file_tag(&read_bounded(
                            file,
                            MAX_MUTATION_FILE_BYTES,
                            &resolved.display,
                        )?);
                        Some((snapshot, tag, permissions))
                    }
                    None => None,
                };
                Ok(state)
            })
        })
        .await
        .map_err(|err| HostError::HostCall(format!("fs.write worker failed: {err}")))??;
        let existed = initial.is_some();
        if existed && !args.overwrite {
            return Err(HostError::InvalidArgs(format!(
                "{} already exists; set overwrite=true",
                resolved.display
            )));
        }
        if existed {
            ctx.require_approval(&approval_action(
                "fs.write",
                serde_json::json!({
                    "path": args.path.clone(),
                    "effect": "overwrite",
                    "expectedTag": initial.as_ref().map(|(_, tag, _)| tag),
                }),
            ))
            .await?;
        }
        let linked = self.linked.clone();
        let commit_path = args.path.clone();
        let data_len = args.data.len();
        let data = args.data.into_bytes();
        let initial_for_commit = initial;
        tokio::task::spawn_blocking(move || -> Result<()> {
            linked.with_stable_policy_snapshot(revision, |linked| {
                let fresh = linked.resolve_spec(Some(&commit_path))?;
                ensure_rw(&fresh.policy, &fresh.display)?;
                let parent = open_parent(
                    &fresh.policy,
                    fresh.root_identity,
                    &fresh.relative,
                    create_parents,
                    &fresh.display,
                )?;
                match initial_for_commit {
                    Some((expected, expected_tag, permissions)) => {
                        let current = stat_entry(&parent, &fresh.display)?.ok_or_else(|| {
                            HostError::InvalidArgs(format!(
                                "stale filesystem entry for {}; retry from a fresh read",
                                fresh.display
                            ))
                        })?;
                        if current.identity != expected.identity || current.kind != expected.kind {
                            return Err(HostError::InvalidArgs(format!(
                                "stale filesystem entry for {}; retry from a fresh read",
                                fresh.display
                            )));
                        }
                        let actual_tag = file_tag(&read_bounded(
                            open_entry_file(&parent, current, &fresh.display)?,
                            MAX_MUTATION_FILE_BYTES,
                            &fresh.display,
                        )?);
                        if actual_tag != expected_tag {
                            return Err(HostError::InvalidArgs(format!(
                                "stale tag for {}: expected {expected_tag}, got {actual_tag}",
                                fresh.display
                            )));
                        }
                        atomic_replace(
                            &parent,
                            current,
                            &permissions,
                            &data,
                            PATCH_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                            &fresh.display,
                        )
                    }
                    None => {
                        if stat_entry(&parent, &fresh.display)?.is_some() {
                            return Err(HostError::InvalidArgs(format!(
                                "{} was created concurrently; retry",
                                fresh.display
                            )));
                        }
                        create_file(&parent, &data, &fresh.display)
                    }
                }
            })
        })
        .await
        .map_err(|err| HostError::HostCall(format!("fs.write worker failed: {err}")))??;
        let result = WriteResult {
            path: display_path(&resolved.alias, &resolved.relative),
            uri: linked_uri(&resolved.alias, &resolved.relative),
            bytes_written: data_len,
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
        let _mutation = self.linked.lock_mutations().await;
        let revision = self.linked.revision();
        let resolved = self.linked.resolve_spec(Some(&patch.path))?;
        ctx.require_linked_alias(&resolved.alias)?;
        ensure_rw(&resolved.policy, &resolved.display)?;
        let linked = self.linked.clone();
        let patch_path = patch.path.clone();
        let patch_tag = patch.tag.clone();
        let hunk_count = patch.hunks.len();
        let prepared = tokio::task::spawn_blocking(move || -> Result<_> {
            linked.with_stable_policy_snapshot(revision, |linked| {
                let current = linked.resolve_spec(Some(&patch_path))?;
                ensure_rw(&current.policy, &current.display)?;
                let parent = open_parent(
                    &current.policy,
                    current.root_identity,
                    &current.relative,
                    false,
                    &current.display,
                )?;
                let snapshot = stat_entry(&parent, &current.display)?.ok_or_else(|| {
                    HostError::InvalidPath(format!(
                        "{}: linked path does not exist",
                        current.display
                    ))
                })?;
                if snapshot.kind != SecureKind::File {
                    return Err(HostError::InvalidPath(format!(
                        "{} is not a file",
                        current.display
                    )));
                }
                let file = open_entry_file(&parent, snapshot, &current.display)?;
                let permissions = file
                    .metadata()
                    .map_err(|err| HostError::HostCall(err.to_string()))?
                    .permissions();
                let old_bytes = read_bounded(file, MAX_MUTATION_FILE_BYTES, &current.display)?;
                let actual_tag = file_tag(&old_bytes);
                if patch_tag != actual_tag {
                    return Err(HostError::InvalidArgs(format!(
                        "stale tag for {patch_path}: expected {patch_tag}, got {actual_tag}"
                    )));
                }
                let old = String::from_utf8(old_bytes).map_err(|_| {
                    HostError::InvalidArgs("fs.patch supports UTF-8 text only".to_string())
                })?;
                let new = apply_line_hunks(&old, &patch.hunks)?;
                if new.len() > MAX_MUTATION_FILE_BYTES {
                    return Err(HostError::InvalidArgs(format!(
                        "fs.patch result exceeds {MAX_MUTATION_FILE_BYTES} bytes"
                    )));
                }
                let changed = old != new;
                let diff = simple_diff(&old, &new, &patch_path);
                let truncated = diff.len() > PATCH_DIFF_PREVIEW_BYTES;
                let new_tag = file_tag(new.as_bytes());
                Ok((
                    snapshot,
                    permissions,
                    new,
                    changed,
                    diff,
                    truncated,
                    new_tag,
                ))
            })
        })
        .await
        .map_err(|err| HostError::HostCall(format!("fs.patch worker failed: {err}")))??;
        let (snapshot, permissions, new, changed, diff, truncated, new_tag) = prepared;
        let diff_artifact = if truncated {
            let persisted_diff = if diff.len() > MAX_PATCH_ARTIFACT_BYTES {
                preview(&diff, MAX_PATCH_ARTIFACT_BYTES)
            } else {
                diff.clone()
            };
            let artifact_store = self.artifact_store.clone();
            let title = format!("fs.patch {}", patch.path);
            Some(
                tokio::task::spawn_blocking(move || {
                    artifact_store
                        .put_text(&persisted_diff, Some(title), "text/x-diff")
                        .map_err(|err| HostError::HostCall(err.to_string()))
                })
                .await
                .map_err(|err| {
                    HostError::HostCall(format!("fs.patch artifact worker failed: {err}"))
                })??,
            )
        } else {
            None
        };
        if changed {
            let linked = self.linked.clone();
            let commit_path = patch.path.clone();
            let expected_tag = patch.tag.clone();
            tokio::task::spawn_blocking(move || -> Result<()> {
                linked.with_stable_policy_snapshot(revision, |linked| {
                    let fresh = linked.resolve_spec(Some(&commit_path))?;
                    ensure_rw(&fresh.policy, &fresh.display)?;
                    let parent = open_parent(
                        &fresh.policy,
                        fresh.root_identity,
                        &fresh.relative,
                        false,
                        &fresh.display,
                    )?;
                    let current = stat_entry(&parent, &fresh.display)?.ok_or_else(|| {
                        HostError::InvalidArgs(format!(
                            "stale filesystem entry for {}; retry from a fresh read",
                            fresh.display
                        ))
                    })?;
                    if current.identity != snapshot.identity || current.kind != snapshot.kind {
                        return Err(HostError::InvalidArgs(format!(
                            "stale filesystem entry for {}; retry from a fresh read",
                            fresh.display
                        )));
                    }
                    let actual_tag = file_tag(&read_bounded(
                        open_entry_file(&parent, current, &fresh.display)?,
                        MAX_MUTATION_FILE_BYTES,
                        &fresh.display,
                    )?);
                    if actual_tag != expected_tag {
                        return Err(HostError::InvalidArgs(format!(
                            "stale tag for {}: expected {expected_tag}, got {actual_tag}",
                            fresh.display
                        )));
                    }
                    atomic_replace(
                        &parent,
                        current,
                        &permissions,
                        new.as_bytes(),
                        PATCH_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                        &fresh.display,
                    )
                })
            })
            .await
            .map_err(|err| HostError::HostCall(format!("fs.patch worker failed: {err}")))??;
        }
        let result = FsPatchResult {
            path: display_path(&resolved.alias, &resolved.relative),
            changed,
            new_tag,
            summary: format!(
                "{} hunk{} applied",
                hunk_count,
                if hunk_count == 1 { "" } else { "s" }
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
        let _mutation = self.linked.lock_mutations().await;
        let revision = self.linked.revision();
        let source = self.linked.resolve_spec(Some(&args.path))?;
        ctx.require_linked_alias(&source.alias)?;
        ensure_rw(&source.policy, &source.display)?;
        let dest_alias = parse_linked_path(&args.dest)?.alias;
        ctx.require_linked_alias(&dest_alias)?;
        if source.alias != dest_alias {
            return Err(HostError::InvalidPath(
                "fs.move destination must stay in the same alias".to_string(),
            ));
        }
        let dest = self.linked.resolve_spec(Some(&args.dest))?;
        ensure_rw(&dest.policy, &dest.display)?;
        if source.relative == dest.relative {
            return Err(HostError::InvalidArgs(
                "fs.move source and destination must differ".to_string(),
            ));
        }
        let linked = self.linked.clone();
        let preflight_source_path = args.path.clone();
        let preflight_dest_path = args.dest.clone();
        let create_parents = args.create_parents;
        let initial = tokio::task::spawn_blocking(move || -> Result<_> {
            linked.with_stable_policy_snapshot(revision, |linked| {
                let source = linked.resolve_spec(Some(&preflight_source_path))?;
                let dest = linked.resolve_spec(Some(&preflight_dest_path))?;
                ensure_rw(&source.policy, &source.display)?;
                ensure_rw(&dest.policy, &dest.display)?;
                if source.alias != dest.alias {
                    return Err(HostError::InvalidPath(
                        "fs.move destination must stay in the same alias".to_string(),
                    ));
                }
                let source_parent = open_parent(
                    &source.policy,
                    source.root_identity,
                    &source.relative,
                    false,
                    &source.display,
                )?;
                let source_snapshot =
                    stat_entry(&source_parent, &source.display)?.ok_or_else(|| {
                        HostError::InvalidPath(format!(
                            "{}: linked path does not exist",
                            source.display
                        ))
                    })?;
                if !matches!(source_snapshot.kind, SecureKind::File | SecureKind::Symlink) {
                    return Err(HostError::InvalidPath(format!(
                        "{} is not a file or file symlink",
                        source.display
                    )));
                }
                let source_bytes = read_entry_bounded(
                    &source.policy,
                    source.root_identity,
                    &source.relative,
                    &source_parent,
                    source_snapshot,
                    MAX_MUTATION_FILE_BYTES,
                    &source.display,
                )?;
                let actual_tag = file_tag(&source_bytes);
                let dest_parent = open_parent(
                    &dest.policy,
                    dest.root_identity,
                    &dest.relative,
                    create_parents,
                    &dest.display,
                )?;
                let dest_snapshot = stat_entry(&dest_parent, &dest.display)?;
                let dest_tag = match dest_snapshot {
                    Some(snapshot) => {
                        if snapshot.kind != SecureKind::File {
                            return Err(HostError::InvalidPath(format!(
                                "{} is not a regular file",
                                dest.display
                            )));
                        }
                        Some(file_tag(&read_bounded(
                            open_entry_file(&dest_parent, snapshot, &dest.display)?,
                            MAX_MUTATION_FILE_BYTES,
                            &dest.display,
                        )?))
                    }
                    None => None,
                };
                Ok((source_snapshot, actual_tag, dest_snapshot, dest_tag))
            })
        })
        .await
        .map_err(|err| HostError::HostCall(format!("fs.move worker failed: {err}")))??;
        let (source_snapshot, actual_tag, dest_snapshot, dest_tag) = initial;
        if args.tag != actual_tag {
            let path = &args.path;
            let expected_tag = &args.tag;
            return Err(HostError::InvalidArgs(format!(
                "stale tag for {path}: expected {expected_tag}, got {actual_tag}"
            )));
        }
        let overwritten = dest_snapshot.is_some();
        if overwritten && !args.overwrite {
            return Err(HostError::InvalidArgs(format!(
                "{} already exists; set overwrite=true",
                dest.display
            )));
        }
        if overwritten {
            ctx.require_approval(&approval_action(
                "fs.move",
                serde_json::json!({
                    "source": args.path.clone(),
                    "destination": args.dest.clone(),
                    "effect": "overwrite_destination",
                    "sourceTag": args.tag.clone(),
                    "destinationTag": dest_tag.clone(),
                }),
            ))
            .await?;
        }
        let linked = self.linked.clone();
        let commit_source_path = args.path.clone();
        let commit_dest_path = args.dest.clone();
        let expected_source_tag = args.tag.clone();
        let expected_dest_tag = dest_tag;
        tokio::task::spawn_blocking(move || -> Result<()> {
            linked.with_stable_policy_snapshot(revision, |linked| {
                let fresh_source = linked.resolve_spec(Some(&commit_source_path))?;
                let fresh_dest = linked.resolve_spec(Some(&commit_dest_path))?;
                ensure_rw(&fresh_source.policy, &fresh_source.display)?;
                ensure_rw(&fresh_dest.policy, &fresh_dest.display)?;
                let source_parent = open_parent(
                    &fresh_source.policy,
                    fresh_source.root_identity,
                    &fresh_source.relative,
                    false,
                    &fresh_source.display,
                )?;
                let current_source = stat_entry(&source_parent, &fresh_source.display)?
                    .ok_or_else(|| {
                        HostError::InvalidArgs(format!(
                            "stale source for {}; retry from a fresh read",
                            fresh_source.display
                        ))
                    })?;
                if current_source.identity != source_snapshot.identity
                    || current_source.kind != source_snapshot.kind
                {
                    return Err(HostError::InvalidArgs(format!(
                        "stale source for {}; retry from a fresh read",
                        fresh_source.display
                    )));
                }
                let current_source_tag = file_tag(&read_entry_bounded(
                    &fresh_source.policy,
                    fresh_source.root_identity,
                    &fresh_source.relative,
                    &source_parent,
                    current_source,
                    MAX_MUTATION_FILE_BYTES,
                    &fresh_source.display,
                )?);
                if current_source_tag != expected_source_tag {
                    return Err(HostError::InvalidArgs(format!(
                        "stale tag for {}: expected {expected_source_tag}, got {current_source_tag}",
                        fresh_source.display
                    )));
                }
                let dest_parent = open_parent(
                    &fresh_dest.policy,
                    fresh_dest.root_identity,
                    &fresh_dest.relative,
                    create_parents,
                    &fresh_dest.display,
                )?;
                let current_dest = stat_entry(&dest_parent, &fresh_dest.display)?;
                if current_dest.map(|entry| (entry.identity, entry.kind))
                    != dest_snapshot.map(|entry| (entry.identity, entry.kind))
                {
                    return Err(HostError::InvalidArgs(format!(
                        "stale destination for {}; retry from a fresh read",
                        fresh_dest.display
                    )));
                }
                if let (Some(current), Some(expected_tag)) = (current_dest, expected_dest_tag) {
                    let current_tag = file_tag(&read_bounded(
                        open_entry_file(&dest_parent, current, &fresh_dest.display)?,
                        MAX_MUTATION_FILE_BYTES,
                        &fresh_dest.display,
                    )?);
                    if current_tag != expected_tag {
                        return Err(HostError::InvalidArgs(format!(
                            "stale destination tag for {}; retry from a fresh read",
                            fresh_dest.display
                        )));
                    }
                }
                rename_entry(
                    &source_parent,
                    current_source,
                    &dest_parent,
                    current_dest,
                    &fresh_dest.display,
                )
            })
        })
        .await
        .map_err(|err| HostError::HostCall(format!("fs.move worker failed: {err}")))??;
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
        let _mutation = self.linked.lock_mutations().await;
        let revision = self.linked.revision();
        let resolved = self.linked.resolve_spec(Some(&args.path))?;
        ctx.require_linked_alias(&resolved.alias)?;
        ensure_rw(&resolved.policy, &resolved.display)?;
        let linked = self.linked.clone();
        let preflight_path = args.path.clone();
        let (snapshot, actual_tag) = tokio::task::spawn_blocking(move || -> Result<_> {
            linked.with_stable_policy_snapshot(revision, |linked| {
                let current = linked.resolve_spec(Some(&preflight_path))?;
                ensure_rw(&current.policy, &current.display)?;
                let parent = open_parent(
                    &current.policy,
                    current.root_identity,
                    &current.relative,
                    false,
                    &current.display,
                )?;
                let snapshot = stat_entry(&parent, &current.display)?.ok_or_else(|| {
                    HostError::InvalidPath(format!(
                        "{}: linked path does not exist",
                        current.display
                    ))
                })?;
                if !matches!(snapshot.kind, SecureKind::File | SecureKind::Symlink) {
                    return Err(HostError::InvalidPath(format!(
                        "{} is not a file or file symlink",
                        current.display
                    )));
                }
                let bytes = read_entry_bounded(
                    &current.policy,
                    current.root_identity,
                    &current.relative,
                    &parent,
                    snapshot,
                    MAX_MUTATION_FILE_BYTES,
                    &current.display,
                )?;
                Ok((snapshot, file_tag(&bytes)))
            })
        })
        .await
        .map_err(|err| HostError::HostCall(format!("fs.remove worker failed: {err}")))??;
        if args.tag != actual_tag {
            let path = &args.path;
            let expected_tag = &args.tag;
            return Err(HostError::InvalidArgs(format!(
                "stale tag for {path}: expected {expected_tag}, got {actual_tag}"
            )));
        }
        ctx.require_approval(&approval_action(
            "fs.remove",
            serde_json::json!({
                "path": args.path.clone(),
                "effect": "remove",
                "expectedTag": args.tag.clone(),
            }),
        ))
        .await?;
        let linked = self.linked.clone();
        let commit_path = args.path.clone();
        let expected_tag = args.tag.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            linked.with_stable_policy_snapshot(revision, |linked| {
                let fresh = linked.resolve_spec(Some(&commit_path))?;
                ensure_rw(&fresh.policy, &fresh.display)?;
                let parent = open_parent(
                    &fresh.policy,
                    fresh.root_identity,
                    &fresh.relative,
                    false,
                    &fresh.display,
                )?;
                let current = stat_entry(&parent, &fresh.display)?.ok_or_else(|| {
                    HostError::InvalidArgs(format!(
                        "stale source for {}; retry from a fresh read",
                        fresh.display
                    ))
                })?;
                if current.identity != snapshot.identity || current.kind != snapshot.kind {
                    return Err(HostError::InvalidArgs(format!(
                        "stale source for {}; retry from a fresh read",
                        fresh.display
                    )));
                }
                let current_tag = file_tag(&read_entry_bounded(
                    &fresh.policy,
                    fresh.root_identity,
                    &fresh.relative,
                    &parent,
                    current,
                    MAX_MUTATION_FILE_BYTES,
                    &fresh.display,
                )?);
                if current_tag != expected_tag {
                    return Err(HostError::InvalidArgs(format!(
                        "stale tag for {}: expected {expected_tag}, got {current_tag}",
                        fresh.display
                    )));
                }
                remove_entry(&parent, current, &fresh.display)
            })
        })
        .await
        .map_err(|err| HostError::HostCall(format!("fs.remove worker failed: {err}")))??;
        Ok(serde_json::json!({
            "path": display_path(&resolved.alias, &resolved.relative),
            "removed": true,
        }))
    }
}

pub(in crate::linked) struct FsLsFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl FsLsFn {
    pub(in crate::linked) fn new(linked: LinkedFolders) -> Self {
        Self {
            linked,
            docs: docs("fs.ls", "fs", "List linked-folder entries", true),
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
        let limit = validate_result_limit("fs.ls", args.limit)?;
        let resolved = self.linked.resolve_spec(args.path.as_deref())?;
        ctx.require_linked_alias(&resolved.alias)?;
        let revision = self.linked.revision();
        let linked = self.linked.clone();
        let stable_path = display_path(&resolved.alias, &resolved.relative);
        let recursive = args.recursive;
        let include_hidden = args.include_hidden;
        let entries = tokio::task::spawn_blocking(move || -> Result<Vec<FsEntry>> {
            linked.with_stable_policy_snapshot(revision, |linked| {
                let resolved = linked.resolve_spec(Some(&stable_path))?;
                list_entries(&resolved, recursive, limit, include_hidden)
            })
        })
        .await
        .map_err(|err| HostError::HostCall(format!("fs.ls worker failed: {err}")))??;
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
            docs: docs("fs.find", "fs", "Find linked-folder entries by glob", true),
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
        let resolved = self.linked.resolve_spec(args.cwd.as_deref())?;
        ctx.require_linked_alias(&resolved.alias)?;
        let revision = self.linked.revision();
        let linked = self.linked.clone();
        let stable_path = display_path(&resolved.alias, &resolved.relative);
        let limit = validate_result_limit("fs.find", args.limit)?;
        let include_hidden = args.include_hidden;
        let entries = tokio::task::spawn_blocking(move || -> Result<Vec<FsEntry>> {
            linked.with_stable_policy_snapshot(revision, |linked| {
                let resolved = linked.resolve_spec(Some(&stable_path))?;
                let root_ignores = if respect_gitignore {
                    load_simple_gitignore(&resolved.policy, resolved.root_identity)?
                } else {
                    None
                };
                let mut entries = Vec::new();
                let mut visited = 0_usize;
                let mut result_bytes = 2_usize;
                walk_secure(
                    &resolved.policy,
                    resolved.root_identity,
                    &resolved.relative,
                    WalkOptions {
                        recursive: true,
                        include_hidden,
                        max_visited: MAX_FS_WALK_ENTRIES,
                    },
                    &mut visited,
                    &mut |entry| {
                        let rel_to_cwd = entry
                            .relative
                            .strip_prefix(&resolved.relative)
                            .unwrap_or(&entry.relative);
                        if let Some(ignores) = &root_ignores
                            && ignores.is_match(&entry.relative)
                        {
                            return Ok(if entry.kind == SecureKind::Directory {
                                WalkControl::SkipDirectory
                            } else {
                                WalkControl::Continue
                            });
                        }
                        if !globset.is_match(rel_to_cwd) && !globset.is_match(&entry.relative) {
                            return Ok(WalkControl::Continue);
                        }
                        let entry = fs_entry_from_secure(&resolved.alias, entry);
                        if !push_bounded_fs_entry(&mut entries, entry, limit, &mut result_bytes)? {
                            return Ok(WalkControl::Stop);
                        }
                        Ok(WalkControl::Continue)
                    },
                )?;
                entries.sort_by(|a, b| a.path.cmp(&b.path));
                Ok(entries)
            })
        })
        .await
        .map_err(|err| HostError::HostCall(format!("fs.find worker failed: {err}")))??;
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

use std::sync::atomic::Ordering;

use super::super::*;
use super::{MAX_PATCH_ARTIFACT_BYTES, PATCH_DIFF_PREVIEW_BYTES, PATCH_TEMP_SEQUENCE};

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

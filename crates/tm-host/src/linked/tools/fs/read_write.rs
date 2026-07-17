use std::sync::atomic::Ordering;

use super::super::*;
use super::PATCH_TEMP_SEQUENCE;

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

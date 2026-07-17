use super::super::*;

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

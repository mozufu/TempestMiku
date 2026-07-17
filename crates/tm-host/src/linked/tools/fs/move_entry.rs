use super::super::*;

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

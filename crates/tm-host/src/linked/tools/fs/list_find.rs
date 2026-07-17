use super::super::*;

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

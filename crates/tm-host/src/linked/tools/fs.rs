use super::*;

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

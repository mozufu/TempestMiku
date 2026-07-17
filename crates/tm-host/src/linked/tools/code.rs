use super::*;

pub(in crate::linked) struct CodeSearchFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl CodeSearchFn {
    pub(in crate::linked) fn new(linked: LinkedFolders) -> Self {
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

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
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
            ctx.require_linked_alias(&parse_linked_path(path)?.alias)?;
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

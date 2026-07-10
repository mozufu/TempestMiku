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

pub(in crate::linked) struct CodeEditFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl CodeEditFn {
    pub(in crate::linked) fn new(linked: LinkedFolders) -> Self {
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
pub(in crate::linked) enum PatchHunk {
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
pub(in crate::linked) enum InsertAt {
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
        ctx.require_linked_alias(&parse_linked_path(&edit.path)?.alias)?;
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

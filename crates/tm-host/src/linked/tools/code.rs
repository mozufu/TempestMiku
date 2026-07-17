use super::*;
use crate::linked::config::ResolvedPath;

pub(in crate::linked) struct CodeSearchFn {
    linked: LinkedFolders,
    docs: ToolDocs,
}

impl CodeSearchFn {
    pub(in crate::linked) fn new(linked: LinkedFolders) -> Self {
        Self {
            linked,
            docs: docs("code.search", "code", "Search UTF-8 linked files", true),
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
        if args.pattern.len() > MAX_SEARCH_PATTERN_BYTES {
            return Err(HostError::InvalidArgs(format!(
                "code.search pattern must not exceed {MAX_SEARCH_PATTERN_BYTES} UTF-8 bytes"
            )));
        }
        if args.paths.is_empty() || args.paths.len() > MAX_SEARCH_PATHS {
            return Err(HostError::InvalidArgs(format!(
                "code.search paths must contain between 1 and {MAX_SEARCH_PATHS} entries"
            )));
        }
        let context_lines = args.context_lines.unwrap_or(0);
        if context_lines > MAX_SEARCH_CONTEXT_LINES {
            return Err(HostError::InvalidArgs(format!(
                "code.search contextLines must not exceed {MAX_SEARCH_CONTEXT_LINES}"
            )));
        }
        let limit = validate_result_limit("code.search", args.limit)?;
        let pattern = if args.regex.unwrap_or(true) {
            args.pattern
        } else {
            regex::escape(&args.pattern)
        };
        let re = RegexBuilder::new(&pattern)
            .case_insensitive(!args.case_sensitive.unwrap_or(true))
            .build()
            .map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let mut roots = Vec::new();
        for path in &args.paths {
            ctx.require_linked_alias(&parse_linked_path(path)?.alias)?;
            roots.push(self.linked.resolve_spec(Some(path))?);
        }
        let stable_paths = roots
            .iter()
            .map(|resolved| display_path(&resolved.alias, &resolved.relative))
            .collect::<Vec<_>>();
        let revision = self.linked.revision();
        let linked = self.linked.clone();
        let out = tokio::task::spawn_blocking(move || -> Result<Vec<SearchMatch>> {
            linked.with_stable_policy_snapshot(revision, |linked| {
                let roots = stable_paths
                    .iter()
                    .map(|path| linked.resolve_spec(Some(path)))
                    .collect::<Result<Vec<_>>>()?;
                search_roots(roots, &re, context_lines, limit)
            })
        })
        .await
        .map_err(|err| HostError::HostCall(format!("code.search worker failed: {err}")))??;
        serde_json::to_value(out).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

fn search_roots(
    mut roots: Vec<ResolvedPath>,
    re: &regex::Regex,
    context_lines: usize,
    limit: usize,
) -> Result<Vec<SearchMatch>> {
    roots.sort_by(|left, right| left.display.cmp(&right.display));
    let mut visited = 0_usize;
    let mut seen = std::collections::BTreeSet::new();
    let mut files_seen = 0_usize;
    let mut total_bytes = 0_u64;
    // The two array delimiters are present even when no match is returned. Each match below is
    // charged by its actual JSON encoding plus its comma, so control characters and other escaped
    // content cannot expand the final response beyond the host result budget.
    let mut result_bytes = 2_usize;
    let mut out = Vec::new();
    let mut done = false;
    for resolved in roots {
        if done || files_seen >= MAX_FS_RESULT_LIMIT {
            break;
        }
        let alias = resolved.alias.clone();
        walk_secure(
            &resolved.policy,
            resolved.root_identity,
            &resolved.relative,
            WalkOptions {
                recursive: true,
                include_hidden: true,
                max_visited: MAX_FS_WALK_ENTRIES,
            },
            &mut visited,
            &mut |mut entry| {
                if entry.kind != SecureKind::File {
                    return Ok(WalkControl::Continue);
                }
                if !seen.insert((alias.clone(), entry.identity)) {
                    return Ok(WalkControl::Continue);
                }
                files_seen += 1;
                if files_seen > MAX_FS_RESULT_LIMIT {
                    done = true;
                    return Ok(WalkControl::Stop);
                }
                let display = display_path(&alias, &entry.relative);
                let size = entry.size_bytes.unwrap_or(0);
                if size > MAX_SEARCH_FILE_BYTES {
                    return Err(HostError::InvalidArgs(format!(
                        "code.search file {display} exceeds {MAX_SEARCH_FILE_BYTES} bytes"
                    )));
                }
                total_bytes = total_bytes.checked_add(size).ok_or_else(|| {
                    HostError::InvalidArgs("code.search byte budget overflow".to_string())
                })?;
                if total_bytes > MAX_SEARCH_TOTAL_BYTES {
                    return Err(HostError::InvalidArgs(format!(
                        "code.search input exceeds {MAX_SEARCH_TOTAL_BYTES} bytes"
                    )));
                }
                let file = entry.file.take().ok_or_else(|| {
                    HostError::HostCall("secure code.search file descriptor missing".to_string())
                })?;
                let bytes = read_bounded(file, MAX_SEARCH_FILE_BYTES as usize, &display)?;
                let tag = file_tag(&bytes);
                let content = String::from_utf8(bytes).map_err(|_| {
                    HostError::InvalidArgs("code.search supports UTF-8 text only in P0".to_string())
                })?;
                let lines: Vec<&str> = content.lines().collect();
                for (idx, line) in lines.iter().enumerate() {
                    for mat in re.find_iter(line) {
                        let start = idx.saturating_sub(context_lines);
                        let end = idx
                            .checked_add(1)
                            .and_then(|end| end.checked_add(context_lines))
                            .unwrap_or(usize::MAX)
                            .min(lines.len());
                        let candidate = SearchMatch {
                            path: display.clone(),
                            uri: linked_uri(&alias, &entry.relative),
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
                        };
                        if !push_bounded_search_match(&mut out, candidate, &mut result_bytes)? {
                            if out.is_empty() {
                                return Err(HostError::InvalidArgs(format!(
                                    "a code.search match exceeds the {MAX_SEARCH_RESULT_BYTES}-byte result budget"
                                )));
                            }
                            done = true;
                            return Ok(WalkControl::Stop);
                        }
                        if out.len() >= limit {
                            done = true;
                            return Ok(WalkControl::Stop);
                        }
                    }
                }
                Ok(WalkControl::Continue)
            },
        )?;
    }
    Ok(out)
}

fn push_bounded_search_match(
    matches: &mut Vec<SearchMatch>,
    candidate: SearchMatch,
    result_bytes: &mut usize,
) -> Result<bool> {
    let encoded =
        serde_json::to_vec(&candidate).map_err(|err| HostError::HostCall(err.to_string()))?;
    let extra = encoded
        .len()
        .saturating_add(usize::from(!matches.is_empty()));
    if result_bytes.saturating_add(extra) > MAX_SEARCH_RESULT_BYTES {
        return Ok(false);
    }
    *result_bytes += extra;
    matches.push(candidate);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_result_budget_counts_json_escaping_and_array_framing() {
        let mut matches = Vec::new();
        let mut result_bytes = 2_usize;
        for index in 0..10_000 {
            let candidate = SearchMatch {
                path: format!("tempestmiku:{index}"),
                uri: format!("linked://tempestmiku/{index}"),
                line: index + 1,
                column: 1,
                // A NUL is one source byte but six bytes in its JSON string representation. This
                // specifically guards against returning a bounded raw string set whose serialized
                // response is several times larger than the configured cap.
                text: "\0".repeat(700),
                before: vec!["\0".repeat(64)],
                after: vec!["\0".repeat(64)],
                tag: "0123456789abcdef".to_string(),
            };
            if !push_bounded_search_match(&mut matches, candidate, &mut result_bytes).unwrap() {
                break;
            }
        }

        let encoded = serde_json::to_vec(&matches).unwrap();
        assert!(matches.len() < 10_000);
        assert_eq!(encoded.len(), result_bytes);
        assert!(encoded.len() <= MAX_SEARCH_RESULT_BYTES);
    }
}

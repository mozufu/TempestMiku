use super::*;

impl DriveService<InMemoryDriveMetadataStore> {
    pub fn get(&self, path_or_uri: &str) -> crate::Result<DriveEntry> {
        let path = normalize_canonical_path_path_or_uri(path_or_uri)?;
        let inner = self.metadata.inner.lock();
        let id = inner
            .path_to_id
            .get(&path)
            .ok_or_else(|| drive_not_found(&inner, path_or_uri, &path))?;
        inner
            .entries
            .get(id)
            .cloned()
            .ok_or_else(|| DriveError::NotFound(path_or_uri.to_string()))
    }

    pub fn read(&self, path_or_uri: &str) -> crate::Result<DriveRead> {
        let entry = self.get(path_or_uri)?;
        let bytes = self
            .artifacts
            .read_blob(&entry.blob_uri)
            .map_err(|err| DriveError::NotFound(err.to_string()))?;
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != entry.content_hash {
            return Err(DriveError::Integrity {
                path: entry.path,
                expected: entry.content_hash,
                actual,
            });
        }
        Ok(DriveRead { entry, bytes })
    }

    pub fn resource_content(
        &self,
        uri: &str,
        selector: Option<&str>,
    ) -> crate::Result<ResourceContent> {
        let path = drive_uri_path(uri)?;
        let read = self.read(&path)?;
        let title = read.entry.title.clone();
        if read.entry.mime.starts_with("text/")
            || read.entry.mime == "application/json"
            || read.entry.mime == "text/markdown"
        {
            let text = String::from_utf8(read.bytes)
                .map_err(|_| DriveError::InvalidArgs("drive text resource is not UTF-8".into()))?;
            let (selected, has_more) = select_text(&text, selector)?;
            Ok(ResourceContent {
                uri: read.entry.uri,
                kind: "drive_document".to_string(),
                mime: read.entry.mime,
                title,
                size_bytes: read.entry.size_bytes,
                selector: selector.map(str::to_string),
                has_more,
                preview: preview(&selected, 1024),
                content: selected,
            })
        } else {
            Ok(ResourceContent {
                uri: read.entry.uri,
                kind: "drive_binary".to_string(),
                mime: read.entry.mime,
                title,
                size_bytes: read.entry.size_bytes,
                selector: None,
                has_more: false,
                preview: format!(
                    "binary drive resource: {} ({} bytes, {})",
                    read.entry.path, read.entry.size_bytes, read.entry.content_hash
                ),
                content: String::new(),
            })
        }
    }

    pub fn list(&self, options: DriveListOptions) -> crate::Result<Vec<DriveEntry>> {
        let limit = options.limit.max(1);
        if let Some(path) = options.path.as_deref()
            && let Some(query) = parse_virtual_dir(path)
        {
            return Ok(self
                .search(virtual_query_to_search(&query, limit))?
                .into_iter()
                .filter_map(|result| self.get(&result.uri).ok())
                .collect());
        }

        let prefix = options
            .path
            .as_deref()
            .map(normalize_optional_prefix)
            .transpose()?
            .unwrap_or_default();
        let inner = self.metadata.inner.lock();
        let mut entries = inner
            .entries
            .values()
            .filter(|entry| options.include_archived || entry.status == DriveEntryStatus::Active)
            .filter(|entry| {
                prefix.is_empty()
                    || entry.path == prefix
                    || entry.path.starts_with(&format!("{prefix}/"))
            })
            .filter(|entry| {
                if options.recursive || prefix.is_empty() {
                    return true;
                }
                let rest = entry
                    .path
                    .strip_prefix(&prefix)
                    .unwrap_or(&entry.path)
                    .trim_start_matches('/');
                !rest.contains('/')
            })
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.path.cmp(&b.path))
        });
        entries.truncate(limit);
        Ok(entries)
    }

    pub fn search(&self, options: DriveSearchOptions) -> crate::Result<Vec<DriveSearchResult>> {
        let query = options.query.as_ref().map(|q| q.to_ascii_lowercase());
        let query_terms = query
            .as_deref()
            .map(|q| q.split_whitespace().map(str::to_string).collect::<Vec<_>>())
            .unwrap_or_default();
        let tag_filter = options
            .tags
            .iter()
            .map(|tag| tag.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let inner = self.metadata.inner.lock();
        let mut results = Vec::new();
        for entry in inner.entries.values() {
            if !options.include_archived && entry.status != DriveEntryStatus::Active {
                continue;
            }
            if options.project.as_ref().is_some_and(|project| {
                entry
                    .project
                    .as_ref()
                    .map(|value| value.to_ascii_lowercase())
                    != Some(project.to_ascii_lowercase())
            }) {
                continue;
            }
            if options.project.is_none() && options.unprojected && entry.project.is_some() {
                continue;
            }
            if options.doc_kind.as_ref().is_some_and(|kind| {
                entry
                    .doc_kind
                    .as_ref()
                    .map(|value| value.to_ascii_lowercase())
                    != Some(kind.to_ascii_lowercase())
            }) {
                continue;
            }
            if !tag_filter.is_empty() {
                let tags = entry
                    .tags
                    .iter()
                    .map(|tag| tag.to_ascii_lowercase())
                    .collect::<BTreeSet<_>>();
                if !tag_filter.is_subset(&tags) {
                    continue;
                }
            }
            if let Some(since) = options.since
                && entry.updated_at < since
            {
                continue;
            }
            if let Some(until) = options.until
                && entry.updated_at > until
            {
                continue;
            }
            let mut score = recency_score(entry);
            if !query_terms.is_empty() {
                let lexical = lexical_score(entry, &query_terms);
                if lexical <= 0.01 {
                    continue;
                }
                score += lexical;
            }
            results.push(DriveSearchResult {
                uri: entry.uri.clone(),
                path: entry.path.clone(),
                title: entry.title.clone(),
                doc_kind: entry.doc_kind.clone(),
                project: entry.project.clone(),
                tags: entry.tags.clone(),
                content_hash: entry.content_hash.clone(),
                score,
                snippet: options
                    .return_snippets
                    .then(|| snippet_for(entry, query.as_deref())),
                selector: options.return_snippets.then(|| "1-3".to_string()),
            });
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });
        results.truncate(options.limit.max(1));
        Ok(results)
    }

    pub fn resource_entries(&self, uri: Option<&str>) -> crate::Result<Vec<ResourceEntry>> {
        let path = uri.map(|uri| uri.trim_start_matches("drive://").to_string());
        let entries = self.list(DriveListOptions {
            path,
            recursive: true,
            limit: 1000,
            include_archived: false,
        })?;
        Ok(entries
            .into_iter()
            .map(|entry| ResourceEntry {
                uri: entry.uri,
                name: entry
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&entry.path)
                    .to_string(),
                kind: entry
                    .doc_kind
                    .unwrap_or_else(|| "drive_document".to_string()),
                title: entry.title,
                size_bytes: Some(entry.size_bytes),
                modified_at: Some(entry.updated_at.to_rfc3339()),
            })
            .collect())
    }

    pub fn correction_signals(&self) -> Vec<(String, String, chrono::DateTime<Utc>)> {
        self.metadata
            .inner
            .lock()
            .corrections
            .iter()
            .map(|correction| {
                (
                    correction.from.clone(),
                    correction.to.clone(),
                    correction.created_at,
                )
            })
            .collect()
    }
}

pub(crate) fn select_text(content: &str, selector: Option<&str>) -> crate::Result<(String, bool)> {
    const DEFAULT_LINE_LIMIT: usize = 200;
    const DEFAULT_BYTE_LIMIT: usize = 64 * 1024;
    const HARD_LINE_LIMIT: usize = 1_000;
    const HARD_BYTE_LIMIT: usize = 256 * 1024;

    let (start, end, byte_limit) = if let Some(selector) = selector {
        let (start, end) = selector
            .split_once('-')
            .ok_or_else(|| DriveError::InvalidArgs(format!("invalid selector {selector}")))?;
        let start = start
            .parse::<usize>()
            .map_err(|_| DriveError::InvalidArgs(format!("invalid selector {selector}")))?;
        let end = end
            .parse::<usize>()
            .map_err(|_| DriveError::InvalidArgs(format!("invalid selector {selector}")))?;
        if start == 0 || end < start || end - start + 1 > HARD_LINE_LIMIT {
            return Err(DriveError::InvalidArgs(format!(
                "selector {selector} exceeds the 1000-line paging limit"
            )));
        }
        (start, end, HARD_BYTE_LIMIT)
    } else {
        if content.len() <= DEFAULT_BYTE_LIMIT
            && content.lines().take(DEFAULT_LINE_LIMIT + 1).count() <= DEFAULT_LINE_LIMIT
        {
            return Ok((content.to_string(), false));
        }
        (1, DEFAULT_LINE_LIMIT, DEFAULT_BYTE_LIMIT)
    };

    let mut selected = String::new();
    let mut has_more = false;
    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        if line_number < start {
            continue;
        }
        if line_number > end {
            has_more = true;
            break;
        }
        let separator_bytes = usize::from(!selected.is_empty());
        if selected.len() + separator_bytes + line.len() > byte_limit {
            if separator_bytes == 1 && selected.len() < byte_limit {
                selected.push('\n');
            }
            let remaining = byte_limit.saturating_sub(selected.len());
            let boundary = line
                .char_indices()
                .map(|(index, _)| index)
                .take_while(|index| *index <= remaining)
                .last()
                .unwrap_or(0);
            let boundary = if line.len() <= remaining {
                line.len()
            } else {
                boundary
            };
            selected.push_str(&line[..boundary]);
            has_more = true;
            break;
        }
        if separator_bytes == 1 {
            selected.push('\n');
        }
        selected.push_str(line);
    }
    Ok((selected, has_more))
}

pub(crate) fn recency_score(entry: &DriveEntry) -> f32 {
    let age = Utc::now()
        .signed_duration_since(entry.updated_at)
        .num_days()
        .max(0) as f32;
    1.0 / (1.0 + age)
}

pub(crate) fn lexical_score(entry: &DriveEntry, terms: &[String]) -> f32 {
    let haystack = format!(
        "{} {} {} {} {} {}",
        entry.path,
        entry.title.clone().unwrap_or_default(),
        entry.summary.clone().unwrap_or_default(),
        entry.doc_kind.clone().unwrap_or_default(),
        entry.project.clone().unwrap_or_default(),
        entry.tags.join(" ")
    )
    .to_ascii_lowercase();
    terms
        .iter()
        .map(|term| {
            if haystack.contains(term) {
                if entry
                    .title
                    .as_ref()
                    .is_some_and(|title| title.to_ascii_lowercase().contains(term))
                {
                    4.0
                } else if entry.path.to_ascii_lowercase().contains(term) {
                    3.0
                } else {
                    1.0
                }
            } else {
                0.0
            }
        })
        .sum()
}

pub(crate) fn snippet_for(entry: &DriveEntry, query: Option<&str>) -> String {
    let summary = entry
        .summary
        .clone()
        .or_else(|| entry.title.clone())
        .unwrap_or_else(|| entry.path.clone());
    if let Some(query) = query {
        let query = query.to_ascii_lowercase();
        for line in summary.lines() {
            if line.to_ascii_lowercase().contains(&query) {
                return preview(line, 240);
            }
        }
    }
    preview(&summary, 240)
}

pub(crate) fn drive_not_found(
    inner: &Inner,
    requested_display: &str,
    normalized_path: &str,
) -> DriveError {
    let suggestions = nearby_drive_paths(inner, normalized_path);
    if suggestions.is_empty() {
        DriveError::NotFound(requested_display.to_string())
    } else {
        DriveError::NotFound(format!(
            "{requested_display}; nearby paths: {}",
            suggestions.join(", ")
        ))
    }
}

pub(crate) fn nearby_drive_paths(inner: &Inner, normalized_path: &str) -> Vec<String> {
    let requested_lower = normalized_path.to_ascii_lowercase();
    let requested_name = normalized_path
        .rsplit('/')
        .next()
        .unwrap_or(normalized_path)
        .to_ascii_lowercase();
    let parent_prefix = normalized_path
        .rsplit_once('/')
        .map(|(parent, _)| format!("{parent}/"));
    let mut scored = inner
        .path_to_id
        .keys()
        .filter_map(|path| {
            let path_lower = path.to_ascii_lowercase();
            let path_name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
            let mut score = 0usize;
            if parent_prefix
                .as_ref()
                .is_some_and(|prefix| path.starts_with(prefix))
            {
                score += 100;
            }
            if path_lower.contains(&requested_lower) || requested_lower.contains(&path_lower) {
                score += 50;
            }
            score += common_prefix_chars(&requested_name, &path_name).min(30);
            (score > 0).then(|| (score, path.clone()))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left_path), (right_score, right_path)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_path.cmp(right_path))
    });
    scored.into_iter().map(|(_, path)| path).take(3).collect()
}

pub(crate) fn common_prefix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tm_host::{HostError, HostFn, InvocationCtx, ToolDocs};

use super::authority::{
    DriveAuthority, cross_project_error, drive_authority, global_project_error,
};
use crate::{
    DriveSearchOptions, SharedDriveStore,
    store::{core::drive_error_to_host, docs::research_drive_docs},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResearchDriveArgs {
    #[serde(default)]
    query: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    doc_kind: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    max_docs: Option<usize>,
    #[serde(default)]
    max_snippets: Option<usize>,
    #[serde(default)]
    max_bytes_per_doc: Option<usize>,
    #[serde(default)]
    max_digest_bytes: Option<usize>,
    #[serde(default)]
    max_workers: Option<usize>,
    #[serde(default)]
    worker_timeout_ms: Option<u64>,
    #[serde(default)]
    total_timeout_ms: Option<u64>,
}

pub(super) struct ResearchDriveFn {
    docs: ToolDocs,
    store: SharedDriveStore,
}

impl ResearchDriveFn {
    pub(super) fn new(store: SharedDriveStore) -> Self {
        Self {
            docs: research_drive_docs(),
            store,
        }
    }
}

#[async_trait]
impl HostFn for ResearchDriveFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: ResearchDriveArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let authority = drive_authority(ctx)?;
        let max_docs = args.max_docs.or(args.limit).unwrap_or(5).clamp(1, 10);
        let max_snippets = args.max_snippets.unwrap_or(max_docs).clamp(1, max_docs);
        let max_bytes_per_doc = args.max_bytes_per_doc.unwrap_or(2_000).clamp(1, 8_000);
        let max_digest_bytes = args.max_digest_bytes.unwrap_or(600).clamp(32, 2_000);
        let max_workers = args.max_workers.unwrap_or(max_docs).min(max_docs);
        let requested_worker_timeout_ms =
            args.worker_timeout_ms.unwrap_or(30_000).clamp(100, 120_000);
        let default_total_timeout_ms = requested_worker_timeout_ms
            .saturating_mul(u64::try_from(max_workers.max(max_docs)).unwrap_or(u64::MAX));
        let total_timeout_ms = args
            .total_timeout_ms
            .unwrap_or(default_total_timeout_ms)
            .clamp(100, 300_000);
        let worker_timeout_ms = requested_worker_timeout_ms.min(total_timeout_ms);

        let mut project = args.project;
        match &authority {
            DriveAuthority::Project(authorized) => {
                if project
                    .as_deref()
                    .is_some_and(|requested| crate::slug(requested) != crate::slug(authorized))
                {
                    return Err(cross_project_error("research.drive", authorized));
                }
                project = Some(authorized.clone());
            }
            DriveAuthority::Global if project.is_some() => {
                return Err(global_project_error("research.drive"));
            }
            DriveAuthority::Global | DriveAuthority::Trusted => {}
        }

        let mut hits = self
            .store
            .search(DriveSearchOptions {
                query: Some(args.query.clone()),
                project,
                doc_kind: args.doc_kind,
                tags: args.tags,
                limit: max_docs,
                return_snippets: true,
                ..DriveSearchOptions::default()
            })
            .await
            .map_err(drive_error_to_host)?;
        hits.retain(|hit| authority.permits_project(hit.project.as_deref()));
        hits.truncate(max_docs.min(max_snippets));

        let mut corpus = Vec::with_capacity(hits.len());
        let mut digests = Vec::with_capacity(hits.len());
        let mut citations = Vec::with_capacity(hits.len());
        for hit in hits {
            let selector = args
                .selector
                .clone()
                .or(hit.selector.clone())
                .unwrap_or_else(|| "1-20".to_string());
            let read = self
                .store
                .resource_content(&hit.uri, Some(&selector))
                .await
                .map_err(drive_error_to_host)?;
            let content = truncate_utf8(&read.content, max_bytes_per_doc);
            let fallback = hit
                .snippet
                .as_deref()
                .or(hit.title.as_deref())
                .unwrap_or(&hit.uri);
            let summary_source = first_nonempty_lines(&content, 3);
            let summary = truncate_utf8(
                if summary_source.is_empty() {
                    fallback
                } else {
                    &summary_source
                },
                max_digest_bytes,
            );
            let citation = json!({
                "uri": hit.uri,
                "sourceKind": "drive",
                "selector": selector,
                "contentHash": hit.content_hash,
            });
            corpus.push(json!({
                "uri": hit.uri,
                "sourceKind": "drive",
                "selector": selector,
                "contentHash": hit.content_hash,
                "title": hit.title.or(Some(hit.path)),
                "snippet": hit.snippet.unwrap_or_else(|| first_nonempty_lines(&content, 3)),
                "sizeBytes": read.size_bytes,
            }));
            digests.push(json!({
                "uri": hit.uri,
                "selector": selector,
                "contentHash": hit.content_hash,
                "summary": summary,
                "actorId": Value::Null,
                "artifactUri": Value::Null,
                "historyUri": Value::Null,
                "citations": [citation.clone()],
            }));
            citations.push(citation);
        }
        let answer = digests
            .iter()
            .map(|digest| {
                format!(
                    "[{}#{}] {}",
                    digest["uri"].as_str().unwrap_or_default(),
                    digest["selector"].as_str().unwrap_or_default(),
                    digest["summary"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let selected_docs = corpus.len();
        Ok(json!({
            "query": args.query,
            "corpus": corpus,
            "digests": digests,
            "citations": citations,
            "workerFailures": [],
            "answer": answer,
            "budget": {
                "maxDocs": max_docs,
                "maxSnippets": max_snippets,
                "maxBytesPerDoc": max_bytes_per_doc,
                "maxDigestBytes": max_digest_bytes,
                "maxWorkers": max_workers,
                "workerTimeoutMs": worker_timeout_ms,
                "totalTimeoutMs": total_timeout_ms,
                "selectedDocs": selected_docs,
                "agentDocs": 0,
                "agentDocsCompleted": 0,
                "workerFailures": 0,
            }
        }))
    }
}

fn first_nonempty_lines(text: &str, max_lines: usize) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(max_lines)
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

use std::sync::Arc;

use async_trait::async_trait;
use tm_artifacts::{ResourceContent, preview};
use tm_host::{HostError, InvocationCtx, ResourceEntry, ResourceHandler, Result as HostResult};
use uuid::Uuid;

use crate::store::{ProfileFactRecord, RecallChunkRecord};
use crate::{ServerError, Store};

use super::util::{decode_memory_segment, encode_memory_segment, short_id};
use super::{
    DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS, DEFAULT_MEMORY_RESOURCE_PREVIEW_BYTES,
    DEFAULT_MEMORY_RESOURCE_RECALL_LIMIT, MemoryContext,
};

#[derive(Clone)]
pub struct MemoryResourceHandler<S> {
    store: Arc<S>,
    subject: String,
    scope: String,
    recall_limit: usize,
    prompt_budget_tokens: usize,
    preview_bytes: usize,
}

impl<S> MemoryResourceHandler<S> {
    pub fn new(store: Arc<S>, subject: impl Into<String>, scope: impl Into<String>) -> Self {
        Self {
            store,
            subject: subject.into(),
            scope: scope.into(),
            recall_limit: DEFAULT_MEMORY_RESOURCE_RECALL_LIMIT,
            prompt_budget_tokens: DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS,
            preview_bytes: DEFAULT_MEMORY_RESOURCE_PREVIEW_BYTES,
        }
    }

    pub fn with_recall_limit(mut self, recall_limit: usize) -> Self {
        self.recall_limit = recall_limit;
        self
    }

    pub fn with_prompt_budget_tokens(mut self, prompt_budget_tokens: usize) -> Self {
        self.prompt_budget_tokens = prompt_budget_tokens;
        self
    }

    pub fn with_preview_bytes(mut self, preview_bytes: usize) -> Self {
        self.preview_bytes = preview_bytes;
        self
    }
}

#[async_trait]
impl<S> ResourceHandler for MemoryResourceHandler<S>
where
    S: Store,
{
    fn scheme(&self) -> &str {
        "memory"
    }

    fn capability(&self) -> &str {
        "resources.read:memory"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> HostResult<ResourceContent> {
        match parse_memory_uri(uri)? {
            MemoryUri::Root => self.root_resource(selector).await,
            MemoryUri::UserModel => self.user_model_resource(selector).await,
            MemoryUri::ProfileFact { subject, id } => {
                let fact = self
                    .store
                    .profile_fact(&subject, id)
                    .await
                    .map_err(map_memory_store_error)?;
                self.text_resource(
                    &profile_fact_uri(&fact),
                    "memory_profile_fact",
                    Some(format!("profile fact {}", short_id(fact.id))),
                    render_profile_fact(&fact),
                    selector,
                )
            }
            MemoryUri::RecallChunk { scope, id } => {
                let chunk = self
                    .store
                    .recall_chunk(&scope, id)
                    .await
                    .map_err(map_memory_store_error)?;
                self.text_resource(
                    &recall_chunk_uri(&chunk),
                    "memory_recall_chunk",
                    Some(format!("recall chunk {}", short_id(chunk.id))),
                    render_recall_chunk(&chunk),
                    selector,
                )
            }
        }
    }

    async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> HostResult<ResourceContent> {
        let mut content = self.read(uri, None, ctx).await?;
        content.preview = preview(&content.content, self.preview_bytes);
        content.has_more = content.has_more || content.content.len() > self.preview_bytes;
        content.content.clear();
        Ok(content)
    }

    async fn list(
        &self,
        uri: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> HostResult<Vec<ResourceEntry>> {
        let uri = uri.unwrap_or("memory://root");
        match parse_memory_list_uri(uri)? {
            MemoryListUri::Root => self.root_entries().await,
            MemoryListUri::UserModel => self.profile_fact_entries().await,
            MemoryListUri::ScopeChunks { scope } => self.recall_chunk_entries(&scope).await,
        }
    }
}

impl<S> MemoryResourceHandler<S>
where
    S: Store,
{
    async fn root_resource(&self, selector: Option<&str>) -> HostResult<ResourceContent> {
        let facts = self
            .store
            .profile_facts(&self.subject)
            .await
            .map_err(map_memory_store_error)?;
        let chunks = self
            .store
            .recall_chunks(&self.scope, "", self.recall_limit)
            .await
            .map_err(map_memory_store_error)?;
        let context = MemoryContext::from_records(
            &self.subject,
            &self.scope,
            facts.clone(),
            chunks.clone(),
            self.prompt_budget_tokens,
        );
        let content = render_memory_root(&self.subject, &self.scope, &context, &facts, &chunks);
        self.text_resource(
            "memory://root",
            "memory_root",
            Some("Memory root".to_string()),
            content,
            selector,
        )
    }

    async fn user_model_resource(&self, selector: Option<&str>) -> HostResult<ResourceContent> {
        let facts = self
            .store
            .profile_facts(&self.subject)
            .await
            .map_err(map_memory_store_error)?;
        self.text_resource(
            "memory://user-model",
            "memory_user_model",
            Some(format!("{} user model", self.subject)),
            render_user_model(&self.subject, &facts),
            selector,
        )
    }

    async fn root_entries(&self) -> HostResult<Vec<ResourceEntry>> {
        let mut entries = vec![
            ResourceEntry {
                uri: "memory://root".to_string(),
                name: "root".to_string(),
                kind: "memory_root".to_string(),
                title: Some("Memory root".to_string()),
                size_bytes: None,
                modified_at: None,
            },
            ResourceEntry {
                uri: "memory://user-model".to_string(),
                name: "user-model".to_string(),
                kind: "memory_user_model".to_string(),
                title: Some(format!("{} user model", self.subject)),
                size_bytes: None,
                modified_at: None,
            },
        ];
        entries.extend(self.profile_fact_entries().await?);
        entries.extend(self.recall_chunk_entries(&self.scope).await?);
        Ok(entries)
    }

    async fn profile_fact_entries(&self) -> HostResult<Vec<ResourceEntry>> {
        let facts = self
            .store
            .profile_facts(&self.subject)
            .await
            .map_err(map_memory_store_error)?;
        Ok(facts
            .into_iter()
            .map(|fact| ResourceEntry {
                uri: profile_fact_uri(&fact),
                name: short_id(fact.id),
                kind: "memory_profile_fact".to_string(),
                title: Some(format!(
                    "{} {} {}",
                    fact.subject, fact.predicate, fact.object
                )),
                size_bytes: None,
                modified_at: Some(fact.valid_from.to_rfc3339()),
            })
            .collect())
    }

    async fn recall_chunk_entries(&self, scope: &str) -> HostResult<Vec<ResourceEntry>> {
        let chunks = self
            .store
            .recall_chunks(scope, "", self.recall_limit)
            .await
            .map_err(map_memory_store_error)?;
        Ok(chunks
            .into_iter()
            .map(|chunk| ResourceEntry {
                uri: recall_chunk_uri(&chunk),
                name: short_id(chunk.id),
                kind: "memory_recall_chunk".to_string(),
                title: Some(preview(&chunk.text, 120)),
                size_bytes: Some(chunk.text.len()),
                modified_at: Some(chunk.created_at.to_rfc3339()),
            })
            .collect())
    }

    fn text_resource(
        &self,
        uri: &str,
        kind: &str,
        title: Option<String>,
        content: String,
        selector: Option<&str>,
    ) -> HostResult<ResourceContent> {
        let size_bytes = content.len();
        let (selected, has_more) = select_memory_text(&content, selector)?;
        Ok(ResourceContent {
            uri: uri.to_string(),
            kind: kind.to_string(),
            mime: "text/plain".to_string(),
            title,
            size_bytes,
            selector: selector.map(str::to_string),
            has_more,
            preview: preview(&selected, self.preview_bytes),
            content: selected,
        })
    }
}

enum MemoryUri {
    Root,
    UserModel,
    ProfileFact { subject: String, id: Uuid },
    RecallChunk { scope: String, id: Uuid },
}

enum MemoryListUri {
    Root,
    UserModel,
    ScopeChunks { scope: String },
}

fn parse_memory_uri(uri: &str) -> HostResult<MemoryUri> {
    let path = uri
        .strip_prefix("memory://")
        .ok_or_else(|| unsupported_memory_uri(uri))?;
    if path.is_empty() || path == "root" {
        return Ok(MemoryUri::Root);
    }
    if path == "user-model" {
        return Ok(MemoryUri::UserModel);
    }
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["profile", subject, "facts", id] => Ok(MemoryUri::ProfileFact {
            subject: decode_memory_segment(subject)?,
            id: parse_memory_uuid(id, uri)?,
        }),
        ["scopes", scope, "chunks", id] => Ok(MemoryUri::RecallChunk {
            scope: decode_memory_segment(scope)?,
            id: parse_memory_uuid(id, uri)?,
        }),
        _ => Err(unsupported_memory_uri(uri)),
    }
}

fn parse_memory_list_uri(uri: &str) -> HostResult<MemoryListUri> {
    let path = uri
        .strip_prefix("memory://")
        .ok_or_else(|| unsupported_memory_uri(uri))?;
    if path.is_empty() || path == "root" {
        return Ok(MemoryListUri::Root);
    }
    if path == "user-model" {
        return Ok(MemoryListUri::UserModel);
    }
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["scopes", scope, "chunks"] => Ok(MemoryListUri::ScopeChunks {
            scope: decode_memory_segment(scope)?,
        }),
        _ => Err(unsupported_memory_uri(uri)),
    }
}

fn unsupported_memory_uri(uri: &str) -> HostError {
    HostError::InvalidPath(format!("unsupported memory uri {uri}"))
}

fn parse_memory_uuid(value: &str, uri: &str) -> HostResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| HostError::InvalidPath(format!("invalid memory uri {uri}")))
}

fn profile_fact_uri(fact: &ProfileFactRecord) -> String {
    format!(
        "memory://profile/{}/facts/{}",
        encode_memory_segment(&fact.subject),
        fact.id
    )
}

fn recall_chunk_uri(chunk: &RecallChunkRecord) -> String {
    format!(
        "memory://scopes/{}/chunks/{}",
        encode_memory_segment(&chunk.scope),
        chunk.id
    )
}

fn render_memory_root(
    subject: &str,
    scope: &str,
    context: &MemoryContext,
    facts: &[ProfileFactRecord],
    chunks: &[RecallChunkRecord],
) -> String {
    let mut lines = vec![
        "Memory root".to_string(),
        format!("Subject: {subject}"),
        format!("Scope: {scope}"),
        format!("User model: memory://user-model"),
        format!(
            "Budget: {}/{} estimated tokens; profile facts: {}/{}; scoped recall: {}/{}; truncated: {}",
            context.budget.used_estimated_tokens,
            context.budget.max_tokens,
            context.budget.included_profile_facts,
            context.budget.available_profile_facts,
            context.budget.included_recall_chunks,
            context.budget.available_recall_chunks,
            context.budget.truncated
        ),
    ];
    if facts.is_empty() {
        lines.push("Profile facts: none".to_string());
    } else {
        lines.push("Profile facts:".to_string());
        lines.extend(facts.iter().map(|fact| {
            format!(
                "- {} :: {} {} {} (confidence {:.2}; provenance: {})",
                profile_fact_uri(fact),
                fact.subject,
                fact.predicate,
                fact.object,
                fact.confidence,
                fact.provenance
            )
        }));
    }
    if chunks.is_empty() {
        lines.push("Scoped recall chunks: none".to_string());
    } else {
        lines.push("Scoped recall chunks:".to_string());
        lines.extend(chunks.iter().map(|chunk| {
            format!(
                "- {} :: {} (source: {})",
                recall_chunk_uri(chunk),
                chunk.text,
                chunk.source
            )
        }));
    }
    lines.join("\n")
}

fn render_user_model(subject: &str, facts: &[ProfileFactRecord]) -> String {
    let mut lines = vec![
        format!("User model"),
        format!("Subject: {subject}"),
        format!("Facts: {}", facts.len()),
    ];
    if facts.is_empty() {
        lines.push("No active profile facts.".to_string());
    } else {
        lines.extend(facts.iter().map(|fact| {
            format!(
                "- {} :: {} {} {} (confidence {:.2}; provenance: {}; valid from: {})",
                profile_fact_uri(fact),
                fact.subject,
                fact.predicate,
                fact.object,
                fact.confidence,
                fact.provenance,
                fact.valid_from.to_rfc3339()
            )
        }));
    }
    lines.join("\n")
}

fn render_profile_fact(fact: &ProfileFactRecord) -> String {
    [
        format!("Profile fact {}", fact.id),
        format!("URI: {}", profile_fact_uri(fact)),
        format!("Subject: {}", fact.subject),
        format!("Predicate: {}", fact.predicate),
        format!("Object: {}", fact.object),
        format!("Confidence: {:.2}", fact.confidence),
        format!("Provenance: {}", fact.provenance),
        format!("Valid from: {}", fact.valid_from.to_rfc3339()),
        format!(
            "Valid to: {}",
            fact.valid_to
                .map(|time| time.to_rfc3339())
                .unwrap_or_else(|| "active".to_string())
        ),
    ]
    .join("\n")
}

fn render_recall_chunk(chunk: &RecallChunkRecord) -> String {
    [
        format!("Scoped recall chunk {}", chunk.id),
        format!("URI: {}", recall_chunk_uri(chunk)),
        format!("Scope: {}", chunk.scope),
        format!("Source: {}", chunk.source),
        format!("Created at: {}", chunk.created_at.to_rfc3339()),
        format!("Text: {}", chunk.text),
    ]
    .join("\n")
}

fn select_memory_text(content: &str, selector: Option<&str>) -> HostResult<(String, bool)> {
    let Some(selector) = selector else {
        return Ok((content.to_string(), false));
    };
    let (start, end) = selector
        .split_once('-')
        .ok_or_else(|| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let start: usize = start
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let end: usize = end
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    if start == 0 || end < start {
        return Err(HostError::InvalidArgs(format!(
            "invalid selector {selector}"
        )));
    }
    let lines = content.lines().collect::<Vec<_>>();
    let selected = lines
        .iter()
        .skip(start - 1)
        .take(end - start + 1)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Ok((selected, end < lines.len()))
}

fn map_memory_store_error(err: ServerError) -> HostError {
    match err {
        ServerError::NotFound(target) => HostError::NotFound(target),
        ServerError::Policy(message) => HostError::InvalidPath(message),
        ServerError::Forbidden => HostError::InvalidPath("forbidden memory resource".to_string()),
        ServerError::InvalidRequest(message) => HostError::InvalidArgs(message),
        ServerError::Unauthorized => {
            HostError::CapabilityDenied("resources.read:memory".to_string())
        }
        ServerError::Store(message) | ServerError::Backend(message) => HostError::HostCall(message),
    }
}

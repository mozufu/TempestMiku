use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tm_artifacts::{ResourceContent, preview};
use tm_host::{HostError, InvocationCtx, ResourceEntry, ResourceHandler, Result as HostResult};
use uuid::Uuid;

use crate::{
    Result, ServerError, Store,
    store::{ProfileFactRecord, RecallChunkRecord},
};

pub const DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS: usize = 1_600;
pub const DEFAULT_MEMORY_RESOURCE_PREVIEW_BYTES: usize = 512;
pub const DEFAULT_MEMORY_RESOURCE_RECALL_LIMIT: usize = 20;
const MAX_STATE_CAPTURE_PROPOSALS_PER_TURN: usize = 6;
const STATE_CAPTURE_MAX_TEXT_CHARS: usize = 280;
const STATE_CAPTURE_PROVENANCE_LABEL: &str = "personal-assistant-state-capture";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryContext {
    pub subject: String,
    pub scope: String,
    pub profile_facts: Vec<MemoryPromptItem>,
    pub recall_chunks: Vec<MemoryPromptItem>,
    pub budget: MemoryPromptBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPromptItem {
    pub id: Uuid,
    pub label: String,
    pub text: String,
    pub provenance_label: String,
    pub source_uri: String,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPromptBudget {
    pub max_tokens: usize,
    pub used_estimated_tokens: usize,
    pub available_profile_facts: usize,
    pub included_profile_facts: usize,
    pub available_recall_chunks: usize,
    pub included_recall_chunks: usize,
    pub truncated: bool,
}

impl MemoryContext {
    pub fn from_records(
        subject: &str,
        scope: &str,
        facts: Vec<ProfileFactRecord>,
        chunks: Vec<RecallChunkRecord>,
        max_tokens: usize,
    ) -> Self {
        let available_profile_facts = facts.len();
        let available_recall_chunks = chunks.len();
        let fact_items = facts.into_iter().map(profile_fact_prompt_item);
        let chunk_items = chunks.into_iter().map(recall_chunk_prompt_item);
        let mut context = Self {
            subject: subject.to_string(),
            scope: scope.to_string(),
            profile_facts: Vec::new(),
            recall_chunks: Vec::new(),
            budget: MemoryPromptBudget {
                max_tokens,
                used_estimated_tokens: 0,
                available_profile_facts,
                included_profile_facts: 0,
                available_recall_chunks,
                included_recall_chunks: 0,
                truncated: false,
            },
        };

        for item in fact_items {
            if context.fits_with(&item, true) {
                context.profile_facts.push(item);
            } else {
                context.budget.truncated = true;
            }
        }
        for item in chunk_items {
            if context.fits_with(&item, false) {
                context.recall_chunks.push(item);
            } else {
                context.budget.truncated = true;
            }
        }
        context.refresh_budget();
        context
    }

    pub fn is_empty(&self) -> bool {
        self.budget.available_profile_facts == 0 && self.budget.available_recall_chunks == 0
    }

    pub fn render_prompt_block(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "Memory context (subject: {}; scope: {}; budget: {}/{} est. tokens; profile facts: {}/{}; scoped recall: {}/{}; truncated: {}).",
            self.subject,
            self.scope,
            self.budget.used_estimated_tokens,
            self.budget.max_tokens,
            self.budget.included_profile_facts,
            self.budget.available_profile_facts,
            self.budget.included_recall_chunks,
            self.budget.available_recall_chunks,
            self.budget.truncated
        ));
        if !self.profile_facts.is_empty() {
            lines.push("Profile facts:".to_string());
            lines.extend(self.profile_facts.iter().map(|fact| {
                format!(
                    "- [{}; provenance: {}] {}",
                    fact.label, fact.provenance_label, fact.text
                )
            }));
        }
        if !self.recall_chunks.is_empty() {
            lines.push("Scoped recall chunks:".to_string());
            lines.extend(self.recall_chunks.iter().map(|chunk| {
                format!(
                    "- [{}; provenance: {}] {}",
                    chunk.label, chunk.provenance_label, chunk.text
                )
            }));
        }
        if self.profile_facts.is_empty()
            && self.recall_chunks.is_empty()
            && (self.budget.available_profile_facts > 0 || self.budget.available_recall_chunks > 0)
        {
            lines.push("No memory items fit this prompt budget.".to_string());
        }
        lines.join("\n")
    }

    fn fits_with(&self, item: &MemoryPromptItem, is_profile_fact: bool) -> bool {
        let mut next = self.clone();
        if is_profile_fact {
            next.profile_facts.push(item.clone());
        } else {
            next.recall_chunks.push(item.clone());
        }
        next.refresh_budget();
        next.budget.used_estimated_tokens <= next.budget.max_tokens
    }

    fn refresh_budget(&mut self) {
        self.budget.included_profile_facts = self.profile_facts.len();
        self.budget.included_recall_chunks = self.recall_chunks.len();
        self.budget.truncated |= self.budget.included_profile_facts
            < self.budget.available_profile_facts
            || self.budget.included_recall_chunks < self.budget.available_recall_chunks;
        self.budget.used_estimated_tokens = estimate_tokens(&self.render_without_budget_numbers());
    }

    fn render_without_budget_numbers(&self) -> String {
        let mut lines = vec![format!(
            "Memory context (subject: {}; scope: {}; budget metadata present).",
            self.subject, self.scope
        )];
        if !self.profile_facts.is_empty() {
            lines.push("Profile facts:".to_string());
            lines.extend(self.profile_facts.iter().map(|fact| {
                format!(
                    "- [{}; provenance: {}] {}",
                    fact.label, fact.provenance_label, fact.text
                )
            }));
        }
        if !self.recall_chunks.is_empty() {
            lines.push("Scoped recall chunks:".to_string());
            lines.extend(self.recall_chunks.iter().map(|chunk| {
                format!(
                    "- [{}; provenance: {}] {}",
                    chunk.label, chunk.provenance_label, chunk.text
                )
            }));
        }
        if self.profile_facts.is_empty()
            && self.recall_chunks.is_empty()
            && (self.budget.available_profile_facts > 0 || self.budget.available_recall_chunks > 0)
        {
            lines.push("No memory items fit this prompt budget.".to_string());
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryWriteKind {
    ProfileFact,
    RecallChunk,
}

impl MemoryWriteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProfileFact => "profile_fact",
            Self::RecallChunk => "recall_chunk",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryWriteStatus {
    Pending,
    Approved,
    Denied,
    TimedOut,
    Cancelled,
}

impl MemoryWriteStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryWriteProposal {
    pub proposal_id: Uuid,
    pub memory_kind: MemoryWriteKind,
    pub subject: String,
    pub scope: String,
    pub text: String,
    pub predicate: Option<String>,
    pub object: Option<String>,
    pub confidence: Option<f32>,
    pub source: String,
    pub provenance_label: String,
    pub provenance: Value,
    pub dedupe_key: String,
    pub record_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecordRef {
    pub id: Uuid,
    pub uri: String,
    pub kind: MemoryWriteKind,
}

impl MemoryWriteProposal {
    pub fn profile_fact(
        subject: String,
        predicate: String,
        object: String,
        confidence: f32,
        source: String,
        provenance_label: String,
        provenance: Value,
        created_at: DateTime<Utc>,
    ) -> Result<Self> {
        let subject = clean_required("subject", subject)?;
        let predicate = clean_required("predicate", predicate)?;
        let object = clean_required("object", object)?;
        let text = format!("{subject} {predicate} {object}");
        let dedupe_key = memory_dedupe_key(&["profile_fact", &subject, &predicate, &object]);
        Ok(Self {
            proposal_id: Uuid::new_v4(),
            memory_kind: MemoryWriteKind::ProfileFact,
            subject,
            scope: "global".to_string(),
            text,
            predicate: Some(predicate),
            object: Some(object),
            confidence: Some(confidence.clamp(0.0, 1.0)),
            source,
            provenance_label,
            provenance,
            record_id: memory_record_id("profile_fact", &dedupe_key),
            dedupe_key,
            created_at,
        })
    }

    pub fn recall_chunk(
        subject: String,
        scope: String,
        text: String,
        source: String,
        provenance_label: String,
        provenance: Value,
        created_at: DateTime<Utc>,
    ) -> Result<Self> {
        let subject = clean_required("subject", subject)?;
        let scope = clean_required("scope", scope)?;
        let text = clean_required("text", text)?;
        let dedupe_key = memory_dedupe_key(&["recall_chunk", &scope, &text]);
        Ok(Self {
            proposal_id: Uuid::new_v4(),
            memory_kind: MemoryWriteKind::RecallChunk,
            subject,
            scope,
            text,
            predicate: None,
            object: None,
            confidence: None,
            source,
            provenance_label,
            provenance,
            record_id: memory_record_id("recall_chunk", &dedupe_key),
            dedupe_key,
            created_at,
        })
    }

    pub fn event_payload(
        &self,
        status: MemoryWriteStatus,
        record: Option<&MemoryRecordRef>,
    ) -> Value {
        json!({
            "kind": "memory",
            "proposalId": self.proposal_id,
            "memoryKind": self.memory_kind,
            "status": status,
            "subject": self.subject,
            "scope": self.scope,
            "text": self.text,
            "predicate": self.predicate,
            "object": self.object,
            "confidence": self.confidence,
            "source": self.source,
            "provenanceLabel": self.provenance_label,
            "provenance": self.provenance,
            "dedupeKey": self.dedupe_key,
            "recordId": self.record_id,
            "record": record,
            "createdAt": self.created_at,
        })
    }

    pub fn approval_scope(&self) -> Value {
        json!({
            "kind": "memory",
            "proposalId": self.proposal_id,
            "memoryKind": self.memory_kind,
            "subject": self.subject,
            "scope": self.scope,
            "text": self.text,
            "predicate": self.predicate,
            "object": self.object,
            "confidence": self.confidence,
            "provenanceLabel": self.provenance_label,
            "dedupeKey": self.dedupe_key,
            "recordId": self.record_id,
        })
    }

    pub fn record_ref(&self) -> MemoryRecordRef {
        MemoryRecordRef {
            id: self.record_id,
            uri: match self.memory_kind {
                MemoryWriteKind::ProfileFact => {
                    format!(
                        "memory://profile/{}/facts/{}",
                        encode_memory_segment(&self.subject),
                        self.record_id
                    )
                }
                MemoryWriteKind::RecallChunk => {
                    format!(
                        "memory://scopes/{}/chunks/{}",
                        encode_memory_segment(&self.scope),
                        self.record_id
                    )
                }
            },
            kind: self.memory_kind,
        }
    }
}

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

fn encode_memory_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '~') {
            encoded.push(ch);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn decode_memory_segment(value: &str) -> HostResult<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(HostError::InvalidPath(format!(
                    "invalid memory uri segment {value}"
                )));
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|_| {
                HostError::InvalidPath(format!("invalid memory uri segment {value}"))
            })?;
            let byte = u8::from_str_radix(hex, 16).map_err(|_| {
                HostError::InvalidPath(format!("invalid memory uri segment {value}"))
            })?;
            decoded.push(byte);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| HostError::InvalidPath(format!("invalid memory uri segment {value}")))
}

#[async_trait]
pub trait MemoryProvider: Send + Sync + 'static {
    async fn context_for_turn(
        &self,
        subject: &str,
        scope: &str,
        query: &str,
    ) -> Result<MemoryContext>;
}

#[derive(Clone)]
pub struct StoreMemoryProvider<S> {
    store: Arc<S>,
    recall_limit: usize,
    prompt_budget_tokens: usize,
}

impl<S> StoreMemoryProvider<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            recall_limit: 5,
            prompt_budget_tokens: DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS,
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
}

#[async_trait]
impl<S> MemoryProvider for StoreMemoryProvider<S>
where
    S: Store,
{
    async fn context_for_turn(
        &self,
        subject: &str,
        scope: &str,
        query: &str,
    ) -> Result<MemoryContext> {
        let facts = self.store.profile_facts(subject).await?;
        let chunks = self
            .store
            .recall_chunks(scope, query, self.recall_limit)
            .await?;
        Ok(MemoryContext::from_records(
            subject,
            scope,
            facts,
            chunks,
            self.prompt_budget_tokens,
        ))
    }
}

pub fn profile_fact_record(proposal: &MemoryWriteProposal) -> Result<ProfileFactRecord> {
    if proposal.memory_kind != MemoryWriteKind::ProfileFact {
        return Err(ServerError::InvalidRequest(
            "memory proposal is not a profile fact".to_string(),
        ));
    }
    Ok(ProfileFactRecord {
        id: proposal.record_id,
        subject: proposal.subject.clone(),
        predicate: proposal.predicate.clone().ok_or_else(|| {
            ServerError::InvalidRequest("profile fact proposal is missing predicate".to_string())
        })?,
        object: proposal.object.clone().ok_or_else(|| {
            ServerError::InvalidRequest("profile fact proposal is missing object".to_string())
        })?,
        confidence: proposal.confidence.unwrap_or(0.8),
        provenance: proposal.provenance_label.clone(),
        valid_from: proposal.created_at,
        valid_to: None,
    })
}

pub fn recall_chunk_record(proposal: &MemoryWriteProposal) -> Result<RecallChunkRecord> {
    if proposal.memory_kind != MemoryWriteKind::RecallChunk {
        return Err(ServerError::InvalidRequest(
            "memory proposal is not a recall chunk".to_string(),
        ));
    }
    Ok(RecallChunkRecord {
        id: proposal.record_id,
        scope: proposal.scope.clone(),
        text: proposal.text.clone(),
        source: proposal.source.clone(),
        created_at: proposal.created_at,
    })
}

pub fn personal_assistant_state_capture_proposals(
    subject: &str,
    scope: &str,
    session_id: Uuid,
    user_content: &str,
    created_at: DateTime<Utc>,
) -> Result<Vec<MemoryWriteProposal>> {
    if should_skip_state_capture_input(user_content) {
        return Ok(Vec::new());
    }

    let source = format!("session:{session_id}:state-capture");
    let mut seen = BTreeSet::new();
    let mut proposals = Vec::new();
    for unit in state_capture_units(user_content) {
        if proposals.len() >= MAX_STATE_CAPTURE_PROPOSALS_PER_TURN {
            break;
        }
        if should_skip_state_capture_unit(&unit) {
            continue;
        }
        let Some(candidate) = state_capture_candidate(&unit) else {
            continue;
        };
        let dedupe = format!(
            "{}:{}",
            candidate.category.as_str(),
            normalize_for_dedupe(candidate.body.as_str())
        );
        if !seen.insert(dedupe) {
            continue;
        }
        let provenance = json!({
            "label": STATE_CAPTURE_PROVENANCE_LABEL,
            "source": source,
            "sourceSession": session_id,
            "sourceTurn": "user",
            "mode": "personal_assistant",
            "scope": scope,
            "capturedCategory": candidate.category.as_str(),
            "proposedAt": created_at,
        });
        match candidate.category {
            StateCaptureCategory::StablePreference => {
                proposals.push(MemoryWriteProposal::profile_fact(
                    subject.to_string(),
                    "prefers".to_string(),
                    candidate.body,
                    0.82,
                    source.clone(),
                    STATE_CAPTURE_PROVENANCE_LABEL.to_string(),
                    provenance,
                    created_at,
                )?);
            }
            _ => {
                proposals.push(MemoryWriteProposal::recall_chunk(
                    subject.to_string(),
                    scope.to_string(),
                    candidate.recall_text(),
                    source.clone(),
                    STATE_CAPTURE_PROVENANCE_LABEL.to_string(),
                    provenance,
                    created_at,
                )?);
            }
        }
    }
    Ok(proposals)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateCaptureCategory {
    StablePreference,
    ActiveProjectOpenLoop,
    CommitmentDeadline,
    Decision,
    ShippedArtifact,
    ReusableWorkflow,
    RecurringBlindSpot,
}

impl StateCaptureCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::StablePreference => "stable_preference",
            Self::ActiveProjectOpenLoop => "active_project_open_loop",
            Self::CommitmentDeadline => "commitment_deadline",
            Self::Decision => "decision",
            Self::ShippedArtifact => "shipped_artifact",
            Self::ReusableWorkflow => "reusable_workflow",
            Self::RecurringBlindSpot => "recurring_blind_spot",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::StablePreference => "Preference",
            Self::ActiveProjectOpenLoop => "Open loop",
            Self::CommitmentDeadline => "Commitment/deadline",
            Self::Decision => "Decision",
            Self::ShippedArtifact => "Shipped artifact",
            Self::ReusableWorkflow => "Reusable workflow",
            Self::RecurringBlindSpot => "Recurring blind spot",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StateCaptureCandidate {
    category: StateCaptureCategory,
    body: String,
}

impl StateCaptureCandidate {
    fn recall_text(&self) -> String {
        format!("{}: {}", self.category.label(), self.body)
    }
}

fn state_capture_candidate(unit: &str) -> Option<StateCaptureCandidate> {
    let lower = unit.to_lowercase();
    if let Some(body) = extract_preference_body(unit) {
        return Some(StateCaptureCandidate {
            category: StateCaptureCategory::StablePreference,
            body,
        });
    }
    if contains_any(
        &lower,
        &[
            "workflow:",
            "playbook:",
            "reusable workflow",
            "reuse this workflow",
            "process:",
            "my process is",
            "our process is",
            "when i ask",
        ],
    ) {
        return recall_candidate(StateCaptureCategory::ReusableWorkflow, unit);
    }
    if contains_commitment_deadline_signal(&lower)
        || (contains_any(&lower, &["i will ", "i need to ", "i have to "])
            && contains_any(&lower, &[" by ", " before ", " deadline", " due"]))
    {
        return recall_candidate(StateCaptureCategory::CommitmentDeadline, unit);
    }
    if contains_any(
        &lower,
        &[
            "decision:",
            "decided:",
            "i decided",
            "we decided",
            "final decision",
            "decision is",
            "going with",
        ],
    ) {
        return recall_candidate(StateCaptureCategory::Decision, unit);
    }
    if contains_any(
        &lower,
        &[
            "shipped",
            "released",
            "launched",
            "published",
            "delivered",
            "artifact://",
            "workspace://session",
        ],
    ) {
        return recall_candidate(StateCaptureCategory::ShippedArtifact, unit);
    }
    if contains_any(
        &lower,
        &[
            "open loop",
            "todo:",
            "to-do:",
            "follow up",
            "follow-up",
            "active project",
            "currently working on",
            "i'm working on",
            "i am working on",
            "track this",
        ],
    ) {
        return recall_candidate(StateCaptureCategory::ActiveProjectOpenLoop, unit);
    }
    if contains_any(
        &lower,
        &[
            "recurring blind spot",
            "i keep forgetting",
            "i keep missing",
            "i often forget",
        ],
    ) {
        return recall_candidate(StateCaptureCategory::RecurringBlindSpot, unit);
    }
    None
}

fn recall_candidate(category: StateCaptureCategory, unit: &str) -> Option<StateCaptureCandidate> {
    let body = clean_state_capture_body(unit)?;
    Some(StateCaptureCandidate { category, body })
}

fn extract_preference_body(unit: &str) -> Option<String> {
    let lower = unit.to_lowercase();
    for marker in [
        "please remember that i prefer ",
        "please remember i prefer ",
        "remember that i prefer ",
        "remember i prefer ",
        "my preference is ",
        "my default is ",
        "i prefer ",
        "i'd rather ",
        "i would rather ",
    ] {
        if let Some(index) = lower.find(marker) {
            let start = index + marker.len();
            return clean_state_capture_body(&unit[start..]);
        }
    }
    None
}

fn should_skip_state_capture_input(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty()
        || trimmed.chars().count() > 2_000
        || trimmed.lines().count() > 20
        || contains_secret_signal(trimmed)
        || contains_sensitive_pii_signal(trimmed)
        || looks_like_raw_log(trimmed)
        || ((!has_durable_capture_signal(trimmed))
            && (contains_transient_mood(trimmed) || contains_one_off_complaint(trimmed)))
}

fn should_skip_state_capture_unit(unit: &str) -> bool {
    contains_secret_signal(unit)
        || contains_sensitive_pii_signal(unit)
        || looks_like_raw_log(unit)
        || looks_like_project_command(unit)
        || ((!has_durable_capture_signal(unit))
            && (contains_transient_mood(unit) || contains_one_off_complaint(unit)))
}

fn has_durable_capture_signal(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "remember",
            "i prefer",
            "preference",
            "default is",
            "open loop",
            "todo:",
            "to-do:",
            "follow up",
            "follow-up",
            "active project",
            "working on",
            "commitment",
            "deadline",
            "due:",
            "due by",
            "due on",
            "due tomorrow",
            "due monday",
            "due tuesday",
            "due wednesday",
            "due thursday",
            "due friday",
            "due saturday",
            "due sunday",
            " by ",
            "decision",
            "decided",
            "shipped",
            "released",
            "launched",
            "published",
            "delivered",
            "artifact://",
            "workflow",
            "playbook",
            "process:",
            "recurring blind spot",
            "i keep forgetting",
            "i keep missing",
        ],
    )
}

fn contains_commitment_deadline_signal(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "deadline:",
            "due:",
            "commitment:",
            "i promised",
            "i committed",
            "due by",
            "due on",
            "due tomorrow",
            "due monday",
            "due tuesday",
            "due wednesday",
            "due thursday",
            "due friday",
            "due saturday",
            "due sunday",
            " by tomorrow",
            " by monday",
            " by tuesday",
            " by wednesday",
            " by thursday",
            " by friday",
            " by saturday",
            " by sunday",
        ],
    )
}

fn contains_secret_signal(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "password",
            "passphrase",
            "api key",
            "apikey",
            "access token",
            "auth token",
            "bearer ",
            "secret",
            "private key",
            "begin private key",
            "credential",
            "oauth",
        ],
    )
}

fn contains_sensitive_pii_signal(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "ssn",
            "social security",
            "passport",
            "driver license",
            "driver's license",
            "home address",
            "phone number",
            "date of birth",
            "birthdate",
            "bank account",
            "credit card",
        ],
    )
}

fn looks_like_raw_log(text: &str) -> bool {
    if text.contains("```") {
        return true;
    }
    let mut log_markers = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();
        if trimmed.starts_with("at ")
            || trimmed.starts_with("thread '")
            || lower.contains("traceback")
            || lower.contains("stack backtrace")
            || lower.contains("error:")
            || lower.contains("warn:")
            || lower.contains("info:")
            || lower.contains("debug:")
            || lower.contains("exception")
            || trimmed.starts_with("[INFO")
            || trimmed.starts_with("[WARN")
            || trimmed.starts_with("[ERROR")
            || trimmed.starts_with("[DEBUG")
            || trimmed
                .get(0..4)
                .is_some_and(|prefix| prefix.chars().all(|c| c.is_ascii_digit()))
                && trimmed.get(4..5) == Some("-")
        {
            log_markers += 1;
        }
    }
    log_markers >= 2
}

fn looks_like_project_command(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "cargo ", "npm ", "pnpm ", "yarn ", "git ", "make ", "docker ", "kubectl ", "deno ",
            "python ",
        ],
    )
}

fn contains_transient_mood(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "i feel ",
            "i'm sad",
            "i am sad",
            "i'm tired",
            "i am tired",
            "i'm exhausted",
            "i am exhausted",
            "overwhelmed",
            "spiraling",
            "self-deprecating",
            "i'm useless",
            "i am useless",
            "grumpy",
            "bad mood",
        ],
    )
}

fn contains_one_off_complaint(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "one-off complaint",
            "just venting",
            "rant:",
            "i hate ",
            "that sucked",
            "was annoying",
            "is annoying",
            "annoyed me",
        ],
    )
}

fn state_capture_units(text: &str) -> Vec<String> {
    let mut units = Vec::new();
    for line in text.lines() {
        let line = trim_state_capture_unit(line);
        if line.is_empty() {
            continue;
        }
        if line.chars().count() <= STATE_CAPTURE_MAX_TEXT_CHARS {
            units.push(line);
            continue;
        }
        for sentence in line.split(['.', '!', '?']) {
            let sentence = trim_state_capture_unit(sentence);
            if !sentence.is_empty() {
                units.push(sentence);
            }
        }
    }
    if units.is_empty() {
        let unit = trim_state_capture_unit(text);
        if !unit.is_empty() {
            units.push(unit);
        }
    }
    units
}

fn trim_state_capture_unit(text: &str) -> String {
    text.trim()
        .trim_start_matches(['-', '*', '•'])
        .trim_start()
        .trim_end_matches(['.', '!', '?'])
        .trim()
        .to_string()
}

fn clean_state_capture_body(text: &str) -> Option<String> {
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned
        .trim()
        .trim_matches(['.', ',', ';', ':', '!', '?', '"'])
        .trim()
        .to_string();
    if cleaned.is_empty()
        || cleaned.chars().count() > STATE_CAPTURE_MAX_TEXT_CHARS
        || contains_secret_signal(&cleaned)
        || contains_sensitive_pii_signal(&cleaned)
        || looks_like_raw_log(&cleaned)
    {
        return None;
    }
    Some(cleaned)
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn profile_fact_prompt_item(fact: ProfileFactRecord) -> MemoryPromptItem {
    let text = format!(
        "{} {} {} (confidence {:.2})",
        fact.subject, fact.predicate, fact.object, fact.confidence
    );
    MemoryPromptItem {
        id: fact.id,
        label: format!("profile:{}", short_id(fact.id)),
        estimated_tokens: estimate_tokens(&text),
        text,
        provenance_label: fact.provenance,
        source_uri: format!("memory://profile/{}/facts/{}", fact.subject, fact.id),
    }
}

fn recall_chunk_prompt_item(chunk: RecallChunkRecord) -> MemoryPromptItem {
    MemoryPromptItem {
        id: chunk.id,
        label: format!("recall:{}; scope: {}", short_id(chunk.id), chunk.scope),
        estimated_tokens: estimate_tokens(&chunk.text),
        source_uri: format!("memory://scopes/{}/chunks/{}", chunk.scope, chunk.id),
        provenance_label: chunk.source,
        text: chunk.text,
    }
}

fn short_id(id: Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    chars.div_ceil(4).max(1)
}

fn clean_required(field: &str, value: String) -> Result<String> {
    let cleaned = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        Err(ServerError::InvalidRequest(format!(
            "memory {field} cannot be empty"
        )))
    } else {
        Ok(cleaned)
    }
}

fn memory_dedupe_key(parts: &[&str]) -> String {
    let normalized = parts
        .iter()
        .map(|part| normalize_for_dedupe(part))
        .collect::<Vec<_>>()
        .join("\x1f");
    let digest = Sha256::digest(normalized.as_bytes());
    format!("sha256:{}", hex::encode(&digest[..16]))
}

fn memory_record_id(namespace: &str, dedupe_key: &str) -> Uuid {
    let digest = Sha256::digest(format!("{namespace}:{dedupe_key}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn normalize_for_dedupe(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_context_renders_provenance_and_budget_metadata() {
        let fact_id = Uuid::new_v4();
        let chunk_id = Uuid::new_v4();
        let context = MemoryContext::from_records(
            "brian",
            "project:tempestmiku",
            vec![ProfileFactRecord {
                id: fact_id,
                subject: "brian".to_string(),
                predicate: "prefers".to_string(),
                object: "boring Rust".to_string(),
                confidence: 0.9,
                provenance: "memory://turns/1".to_string(),
                valid_from: Utc::now(),
                valid_to: None,
            }],
            vec![RecallChunkRecord {
                id: chunk_id,
                scope: "project:tempestmiku".to_string(),
                text: "Keep approval writes replayable.".to_string(),
                source: "session:abc:assistant".to_string(),
                created_at: Utc::now(),
            }],
            1_600,
        );

        let rendered = context.render_prompt_block();
        assert!(rendered.contains("budget:"));
        assert!(rendered.contains("profile facts: 1/1"));
        assert!(rendered.contains("scoped recall: 1/1"));
        assert!(rendered.contains("provenance: memory://turns/1"));
        assert!(rendered.contains("scope: project:tempestmiku"));
        assert!(rendered.contains("boring Rust"));
        assert_eq!(context.profile_facts[0].id, fact_id);
        assert_eq!(context.recall_chunks[0].id, chunk_id);
    }

    #[test]
    fn memory_context_trims_to_budget_and_reports_truncation() {
        let context = MemoryContext::from_records(
            "brian",
            "global",
            vec![ProfileFactRecord {
                id: Uuid::new_v4(),
                subject: "brian".to_string(),
                predicate: "prefers".to_string(),
                object: "a very long durable fact that will not fit the tiny prompt budget"
                    .to_string(),
                confidence: 0.9,
                provenance: "test".to_string(),
                valid_from: Utc::now(),
                valid_to: None,
            }],
            Vec::new(),
            4,
        );

        assert!(context.budget.truncated);
        assert_eq!(context.budget.included_profile_facts, 0);
        assert!(
            context
                .render_prompt_block()
                .contains("No memory items fit")
        );
    }

    #[test]
    fn memory_write_proposals_use_stable_record_ids_for_idempotency() {
        let a = MemoryWriteProposal::recall_chunk(
            "brian".to_string(),
            "global".to_string(),
            "Remember the same thing".to_string(),
            "session:a".to_string(),
            "manual".to_string(),
            json!({ "source": "test" }),
            Utc::now(),
        )
        .unwrap();
        let b = MemoryWriteProposal::recall_chunk(
            "brian".to_string(),
            "global".to_string(),
            "  remember   the SAME thing ".to_string(),
            "session:b".to_string(),
            "manual".to_string(),
            json!({ "source": "test" }),
            Utc::now(),
        )
        .unwrap();

        assert_eq!(a.dedupe_key, b.dedupe_key);
        assert_eq!(a.record_id, b.record_id);
        assert_ne!(a.proposal_id, b.proposal_id);
    }

    #[test]
    fn personal_assistant_state_capture_captures_durable_state_categories() {
        let proposals = personal_assistant_state_capture_proposals(
            "brian",
            "project:tempestmiku",
            Uuid::new_v4(),
            "Remember that I prefer short approval summaries.\n\
             Active project: TempestMiku P2.5 state capture.\n\
             Deadline: send the P2 notes by Friday.\n\
             Decision: keep memory writes approval-backed.\n\
             Shipped: artifact://0 has the capture fixture.\n\
             Workflow: for release notes, gather commits then draft concise bullets.",
            Utc::now(),
        )
        .unwrap();

        assert_eq!(proposals.len(), 6);
        let fact = proposals
            .iter()
            .find(|proposal| proposal.memory_kind == MemoryWriteKind::ProfileFact)
            .unwrap();
        assert_eq!(fact.predicate.as_deref(), Some("prefers"));
        assert_eq!(fact.object.as_deref(), Some("short approval summaries"));
        assert_eq!(fact.provenance_label, STATE_CAPTURE_PROVENANCE_LABEL);

        let recall_text = proposals
            .iter()
            .filter(|proposal| proposal.memory_kind == MemoryWriteKind::RecallChunk)
            .map(|proposal| proposal.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(recall_text.contains("Open loop: Active project: TempestMiku P2.5 state capture"));
        assert!(recall_text.contains("Commitment/deadline: Deadline: send the P2 notes by Friday"));
        assert!(recall_text.contains("Decision: Decision: keep memory writes approval-backed"));
        assert!(
            recall_text.contains("Shipped artifact: Shipped: artifact://0 has the capture fixture")
        );
        assert!(recall_text.contains("Reusable workflow: Workflow: for release notes"));
    }

    #[test]
    fn personal_assistant_state_capture_does_not_capture_noise_or_sensitive_content() {
        for content in [
            "I'm overwhelmed and sad tonight.",
            "Just venting: that meeting was annoying.",
            "Please remember my password is hunter2.",
            "Remember my passport number is X1234567.",
            "2026-07-03 ERROR failed to connect\n2026-07-03 WARN retrying\nstack backtrace:",
            "```text\nINFO raw logs should stay out of memory\nERROR nope\n```",
        ] {
            let proposals = personal_assistant_state_capture_proposals(
                "brian",
                "global",
                Uuid::new_v4(),
                content,
                Utc::now(),
            )
            .unwrap();
            assert!(proposals.is_empty(), "{content}");
        }
    }
}

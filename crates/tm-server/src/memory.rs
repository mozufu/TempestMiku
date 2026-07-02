use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    Result, ServerError, Store,
    store::{ProfileFactRecord, RecallChunkRecord},
};

pub const DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS: usize = 1_600;

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
                    format!("memory://profile/{}/facts/{}", self.subject, self.record_id)
                }
                MemoryWriteKind::RecallChunk => {
                    format!("memory://scopes/{}/chunks/{}", self.scope, self.record_id)
                }
            },
            kind: self.memory_kind,
        }
    }
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
}

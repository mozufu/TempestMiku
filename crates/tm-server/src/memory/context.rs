use serde::{Deserialize, Serialize};
use tm_memory::MemorySummaryRecord;
use uuid::Uuid;

use crate::store::{ProfileFactRecord, RecallChunkRecord};

use super::util::{estimate_tokens, short_id};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryContext {
    pub subject: String,
    pub scope: String,
    pub profile_facts: Vec<MemoryPromptItem>,
    pub summaries: Vec<MemoryPromptItem>,
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
    pub available_summaries: usize,
    pub included_summaries: usize,
    pub available_recall_chunks: usize,
    pub included_recall_chunks: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy)]
enum MemoryPromptBucket {
    ProfileFact,
    Summary,
    RecallChunk,
}

impl MemoryContext {
    pub fn from_records(
        subject: &str,
        scope: &str,
        facts: Vec<ProfileFactRecord>,
        chunks: Vec<RecallChunkRecord>,
        max_tokens: usize,
    ) -> Self {
        Self::from_records_with_summaries(subject, scope, facts, Vec::new(), chunks, max_tokens)
    }

    pub fn from_records_with_summaries(
        subject: &str,
        scope: &str,
        facts: Vec<ProfileFactRecord>,
        summaries: Vec<MemorySummaryRecord>,
        chunks: Vec<RecallChunkRecord>,
        max_tokens: usize,
    ) -> Self {
        let available_profile_facts = facts.len();
        let available_summaries = summaries.len();
        let available_recall_chunks = chunks.len();
        let fact_items = facts.into_iter().map(profile_fact_prompt_item);
        let summary_items = summaries.into_iter().map(summary_prompt_item);
        let chunk_items = chunks.into_iter().map(recall_chunk_prompt_item);
        let mut context = Self {
            subject: subject.to_string(),
            scope: scope.to_string(),
            profile_facts: Vec::new(),
            summaries: Vec::new(),
            recall_chunks: Vec::new(),
            budget: MemoryPromptBudget {
                max_tokens,
                used_estimated_tokens: 0,
                available_profile_facts,
                included_profile_facts: 0,
                available_summaries,
                included_summaries: 0,
                available_recall_chunks,
                included_recall_chunks: 0,
                truncated: false,
            },
        };

        for item in fact_items {
            if context.fits_with(&item, MemoryPromptBucket::ProfileFact) {
                context.profile_facts.push(item);
            } else {
                context.budget.truncated = true;
            }
        }
        for item in summary_items {
            if context.fits_with(&item, MemoryPromptBucket::Summary) {
                context.summaries.push(item);
            } else {
                context.budget.truncated = true;
            }
        }
        for item in chunk_items {
            if context.fits_with(&item, MemoryPromptBucket::RecallChunk) {
                context.recall_chunks.push(item);
            } else {
                context.budget.truncated = true;
            }
        }
        context.refresh_budget();
        context
    }

    pub fn is_empty(&self) -> bool {
        self.budget.available_profile_facts == 0
            && self.budget.available_summaries == 0
            && self.budget.available_recall_chunks == 0
    }

    pub fn render_prompt_block(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "Memory context (subject: {}; scope: {}; budget: {}/{} est. tokens; profile facts: {}/{}; summaries: {}/{}; scoped recall: {}/{}; truncated: {}).",
            self.subject,
            self.scope,
            self.budget.used_estimated_tokens,
            self.budget.max_tokens,
            self.budget.included_profile_facts,
            self.budget.available_profile_facts,
            self.budget.included_summaries,
            self.budget.available_summaries,
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
        if !self.summaries.is_empty() {
            lines.push("Recent summaries:".to_string());
            lines.extend(self.summaries.iter().map(|summary| {
                format!(
                    "- [{}; provenance: {}; source: {}] {}",
                    summary.label, summary.provenance_label, summary.source_uri, summary.text
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
            && self.summaries.is_empty()
            && self.recall_chunks.is_empty()
            && (self.budget.available_profile_facts > 0
                || self.budget.available_summaries > 0
                || self.budget.available_recall_chunks > 0)
        {
            lines.push("No memory items fit this prompt budget.".to_string());
        }
        lines.join("\n")
    }

    fn fits_with(&self, item: &MemoryPromptItem, bucket: MemoryPromptBucket) -> bool {
        let mut next = self.clone();
        match bucket {
            MemoryPromptBucket::ProfileFact => next.profile_facts.push(item.clone()),
            MemoryPromptBucket::Summary => next.summaries.push(item.clone()),
            MemoryPromptBucket::RecallChunk => next.recall_chunks.push(item.clone()),
        }
        next.refresh_budget();
        next.budget.used_estimated_tokens <= next.budget.max_tokens
    }

    fn refresh_budget(&mut self) {
        self.budget.included_profile_facts = self.profile_facts.len();
        self.budget.included_summaries = self.summaries.len();
        self.budget.included_recall_chunks = self.recall_chunks.len();
        self.budget.truncated |= self.budget.included_profile_facts
            < self.budget.available_profile_facts
            || self.budget.included_summaries < self.budget.available_summaries
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
        if !self.summaries.is_empty() {
            lines.push("Recent summaries:".to_string());
            lines.extend(self.summaries.iter().map(|summary| {
                format!(
                    "- [{}; provenance: {}; source: {}] {}",
                    summary.label, summary.provenance_label, summary.source_uri, summary.text
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
            && self.summaries.is_empty()
            && self.recall_chunks.is_empty()
            && (self.budget.available_profile_facts > 0
                || self.budget.available_summaries > 0
                || self.budget.available_recall_chunks > 0)
        {
            lines.push("No memory items fit this prompt budget.".to_string());
        }
        lines.join("\n")
    }
}

fn summary_prompt_item(summary: MemorySummaryRecord) -> MemoryPromptItem {
    let source_uri = format!("memory://summaries/{}", summary.id);
    let provenance_label = summary
        .source_session_id
        .map(|session_id| {
            format!(
                "dream:{}; session:{}",
                short_id(summary.source_dream_id),
                short_id(session_id)
            )
        })
        .unwrap_or_else(|| format!("dream:{}", short_id(summary.source_dream_id)));
    let text = format!(
        "{} summary: {}\n{}",
        summary.kind, summary.title, summary.body
    );
    MemoryPromptItem {
        id: summary.id,
        label: format!(
            "summary:{}; kind: {}; scope: {}",
            short_id(summary.id),
            summary.kind,
            summary.scope
        ),
        estimated_tokens: estimate_tokens(&text),
        text,
        provenance_label,
        source_uri,
    }
}

fn profile_fact_prompt_item(fact: ProfileFactRecord) -> MemoryPromptItem {
    let text = format!(
        "{} {} {} (confidence {:.2}; importance {:.2})",
        fact.subject, fact.predicate, fact.object, fact.confidence, fact.importance
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
        text: format!("{} (importance {:.2})", chunk.text, chunk.importance),
    }
}

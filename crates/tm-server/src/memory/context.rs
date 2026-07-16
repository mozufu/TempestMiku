use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tm_memory::{
    HybridMemoryCandidate, MemoryEvidenceSource, MemoryRecordKind, MemoryRecordResource,
    MemoryRecordStatus, MemorySummaryRecord,
};
use uuid::Uuid;

use crate::store::{ProfileFactRecord, RecallChunkRecord};

use super::util::{estimate_tokens, short_id};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryContext {
    pub subject: String,
    pub scope: String,
    pub profile_facts: Vec<MemoryPromptItem>,
    pub summaries: Vec<MemoryPromptItem>,
    pub recall_chunks: Vec<MemoryPromptItem>,
    #[serde(default)]
    pub hybrid_recall: Vec<MemoryPromptItem>,
    pub budget: MemoryPromptBudget,
    #[serde(default)]
    pub retrieval: MemoryRetrievalTrace,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRetrievalMode {
    #[default]
    LegacyLexical,
    Hybrid,
    LexicalFallback,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRetrievalTrace {
    pub mode: MemoryRetrievalMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    #[serde(default)]
    pub candidates: Vec<MemoryCandidateTrace>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCandidateTrace {
    pub id: Uuid,
    pub kind: MemoryRecordKind,
    pub status: MemoryRecordStatus,
    pub source_uri: String,
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lexical_rank: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lexical_score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dense_rank: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dense_score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_version: Option<String>,
    pub rrf_score: f32,
    pub estimated_tokens: usize,
    pub included: bool,
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
    #[serde(default)]
    pub available_hybrid_recall: usize,
    #[serde(default)]
    pub included_hybrid_recall: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryPromptBucket {
    ProfileFact,
    Summary,
    RecallChunk,
    HybridRecall,
}

const MAX_MEMORY_PROMPT_ITEMS: usize = 5;
const RESERVED_PROFILE_FACTS: usize = 2;
const RESERVED_HYBRID_RECALL: usize = 2;
const RESERVED_SUMMARIES: usize = 1;

#[derive(Debug, Clone)]
struct SelectedPromptItem {
    item: MemoryPromptItem,
    bucket: MemoryPromptBucket,
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
            hybrid_recall: Vec::new(),
            budget: MemoryPromptBudget {
                max_tokens,
                used_estimated_tokens: 0,
                available_profile_facts,
                included_profile_facts: 0,
                available_summaries,
                included_summaries: 0,
                available_recall_chunks,
                included_recall_chunks: 0,
                available_hybrid_recall: 0,
                included_hybrid_recall: 0,
                truncated: false,
            },
            retrieval: MemoryRetrievalTrace::default(),
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

    pub fn from_hybrid_candidates_with_summaries(
        subject: &str,
        scope: &str,
        summaries: Vec<MemorySummaryRecord>,
        candidates: Vec<HybridMemoryCandidate>,
        max_tokens: usize,
        embedding_version: Option<String>,
    ) -> Self {
        Self::from_hybrid_candidates_with_profile_facts_and_summaries(
            subject,
            scope,
            Vec::new(),
            summaries,
            candidates,
            max_tokens,
            embedding_version,
        )
    }

    pub fn from_hybrid_candidates_with_profile_facts_and_summaries(
        subject: &str,
        scope: &str,
        facts: Vec<ProfileFactRecord>,
        summaries: Vec<MemorySummaryRecord>,
        candidates: Vec<HybridMemoryCandidate>,
        max_tokens: usize,
        embedding_version: Option<String>,
    ) -> Self {
        // The grouped fact/summary views remain the prompt representation for their mirrored
        // typed records. Remove only the duplicate ranked candidate, keyed by kind plus id so an
        // episodic and semantic record cannot accidentally suppress each other.
        let mirrored_identities = facts
            .iter()
            .map(|fact| (MemoryRecordKind::Semantic, fact.id))
            .chain(
                summaries
                    .iter()
                    .map(|summary| (MemoryRecordKind::Episodic, summary.id)),
            )
            .collect::<HashSet<_>>();
        let candidates = candidates
            .into_iter()
            .filter(|candidate| {
                !mirrored_identities.contains(&(candidate.record.kind(), candidate.record.id()))
            })
            .collect::<Vec<_>>();
        let available_profile_facts = facts.len();
        let available_summaries = summaries.len();
        let available_hybrid_recall = candidates.len();
        let candidate_items = candidates
            .iter()
            .map(hybrid_candidate_prompt_item)
            .collect::<Vec<_>>();
        let candidate_traces = candidates
            .into_iter()
            .zip(candidate_items.iter())
            .map(|(candidate, item)| hybrid_candidate_trace(candidate, item.estimated_tokens))
            .collect();
        let mut context = Self {
            subject: subject.to_string(),
            scope: scope.to_string(),
            profile_facts: Vec::new(),
            summaries: Vec::new(),
            recall_chunks: Vec::new(),
            hybrid_recall: Vec::new(),
            budget: MemoryPromptBudget {
                max_tokens,
                used_estimated_tokens: 0,
                available_profile_facts,
                included_profile_facts: 0,
                available_summaries,
                included_summaries: 0,
                available_recall_chunks: 0,
                included_recall_chunks: 0,
                available_hybrid_recall,
                included_hybrid_recall: 0,
                truncated: false,
            },
            retrieval: MemoryRetrievalTrace {
                mode: MemoryRetrievalMode::Hybrid,
                embedding_version,
                degraded_reason: None,
                candidates: candidate_traces,
            },
        };

        let fact_items = facts
            .into_iter()
            .map(profile_fact_prompt_item)
            .collect::<Vec<_>>();
        let summary_items = summaries
            .into_iter()
            .map(summary_prompt_item)
            .collect::<Vec<_>>();
        let selected = select_balanced_prompt_items(&fact_items, &candidate_items, &summary_items);

        // Query-ranked candidates receive first admission, but facts and a summary remain
        // interleaved so a long first bucket cannot consume the complete token budget.
        for selected in selected {
            if context.fits_with(&selected.item, selected.bucket) {
                match selected.bucket {
                    MemoryPromptBucket::ProfileFact => context.profile_facts.push(selected.item),
                    MemoryPromptBucket::Summary => context.summaries.push(selected.item),
                    MemoryPromptBucket::HybridRecall => context.hybrid_recall.push(selected.item),
                    MemoryPromptBucket::RecallChunk => unreachable!(
                        "balanced hybrid allocation does not select legacy recall chunks"
                    ),
                }
            } else {
                context.budget.truncated = true;
            }
        }
        context.refresh_budget();
        context
    }

    pub fn mark_lexical_fallback(mut self, reason: impl Into<String>) -> Self {
        self.retrieval.mode = MemoryRetrievalMode::LexicalFallback;
        self.retrieval.degraded_reason = Some(reason.into());
        self
    }

    pub fn requires_durable_trace(&self) -> bool {
        self.retrieval.mode != MemoryRetrievalMode::LegacyLexical
    }

    pub fn is_empty(&self) -> bool {
        self.budget.available_profile_facts == 0
            && self.budget.available_summaries == 0
            && self.budget.available_recall_chunks == 0
            && self.budget.available_hybrid_recall == 0
    }

    pub fn render_prompt_block(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "Memory context (subject: {}; scope: {}; retrieval: {:?}; degraded: {}; budget: {}/{} est. tokens; profile facts: {}/{}; summaries: {}/{}; scoped recall: {}/{}; hybrid recall: {}/{}; truncated: {}).",
            self.subject,
            self.scope,
            self.retrieval.mode,
            self.retrieval.degraded_reason.as_deref().unwrap_or("none"),
            self.budget.used_estimated_tokens,
            self.budget.max_tokens,
            self.budget.included_profile_facts,
            self.budget.available_profile_facts,
            self.budget.included_summaries,
            self.budget.available_summaries,
            self.budget.included_recall_chunks,
            self.budget.available_recall_chunks,
            self.budget.included_hybrid_recall,
            self.budget.available_hybrid_recall,
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
        if !self.hybrid_recall.is_empty() {
            lines.push("Hybrid episodic/semantic recall:".to_string());
            lines.extend(self.hybrid_recall.iter().map(|item| {
                format!(
                    "- [{}; provenance: {}; source: {}] {}",
                    item.label, item.provenance_label, item.source_uri, item.text
                )
            }));
        }
        if self.profile_facts.is_empty()
            && self.summaries.is_empty()
            && self.recall_chunks.is_empty()
            && self.hybrid_recall.is_empty()
            && (self.budget.available_profile_facts > 0
                || self.budget.available_summaries > 0
                || self.budget.available_recall_chunks > 0
                || self.budget.available_hybrid_recall > 0)
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
            MemoryPromptBucket::HybridRecall => next.hybrid_recall.push(item.clone()),
        }
        next.refresh_budget();
        next.budget.used_estimated_tokens <= next.budget.max_tokens
    }

    fn refresh_budget(&mut self) {
        self.budget.included_profile_facts = self.profile_facts.len();
        self.budget.included_summaries = self.summaries.len();
        self.budget.included_recall_chunks = self.recall_chunks.len();
        self.budget.included_hybrid_recall = self.hybrid_recall.len();
        self.budget.truncated |= self.budget.included_profile_facts
            < self.budget.available_profile_facts
            || self.budget.included_summaries < self.budget.available_summaries
            || self.budget.included_recall_chunks < self.budget.available_recall_chunks
            || self.budget.included_hybrid_recall < self.budget.available_hybrid_recall;
        self.budget.used_estimated_tokens = estimate_tokens(&self.render_without_budget_numbers());
        for candidate in &mut self.retrieval.candidates {
            candidate.included = self
                .hybrid_recall
                .iter()
                .any(|item| item.id == candidate.id);
        }
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
        if !self.hybrid_recall.is_empty() {
            lines.push("Hybrid episodic/semantic recall:".to_string());
            lines.extend(self.hybrid_recall.iter().map(|item| {
                format!(
                    "- [{}; provenance: {}; source: {}] {}",
                    item.label, item.provenance_label, item.source_uri, item.text
                )
            }));
        }
        if self.profile_facts.is_empty()
            && self.summaries.is_empty()
            && self.recall_chunks.is_empty()
            && self.hybrid_recall.is_empty()
            && (self.budget.available_profile_facts > 0
                || self.budget.available_summaries > 0
                || self.budget.available_recall_chunks > 0
                || self.budget.available_hybrid_recall > 0)
        {
            lines.push("No memory items fit this prompt budget.".to_string());
        }
        lines.join("\n")
    }
}

fn select_balanced_prompt_items(
    facts: &[MemoryPromptItem],
    candidates: &[MemoryPromptItem],
    summaries: &[MemoryPromptItem],
) -> Vec<SelectedPromptItem> {
    let mut fact_count = facts.len().min(RESERVED_PROFILE_FACTS);
    let mut candidate_count = candidates.len().min(RESERVED_HYBRID_RECALL);
    let mut summary_count = summaries.len().min(RESERVED_SUMMARIES);
    let mut remaining =
        MAX_MEMORY_PROMPT_ITEMS.saturating_sub(fact_count + candidate_count + summary_count);

    for (available, selected) in [
        (candidates.len(), &mut candidate_count),
        (facts.len(), &mut fact_count),
        (summaries.len(), &mut summary_count),
    ] {
        let added = available.saturating_sub(*selected).min(remaining);
        *selected += added;
        remaining -= added;
    }

    let mut selected = Vec::with_capacity(fact_count + candidate_count + summary_count);
    for index in 0..MAX_MEMORY_PROMPT_ITEMS {
        let choice = match index {
            0 if candidate_count > 0 => Some((MemoryPromptBucket::HybridRecall, &candidates[0])),
            1 if fact_count > 0 => Some((MemoryPromptBucket::ProfileFact, &facts[0])),
            2 if summary_count > 0 => Some((MemoryPromptBucket::Summary, &summaries[0])),
            3 if candidate_count > 1 => Some((MemoryPromptBucket::HybridRecall, &candidates[1])),
            4 if fact_count > 1 => Some((MemoryPromptBucket::ProfileFact, &facts[1])),
            _ => None,
        };
        if let Some((bucket, item)) = choice {
            selected.push(SelectedPromptItem {
                item: item.clone(),
                bucket,
            });
        }
    }

    // Fill quota holes in the locked priority order while retaining the source order inside each
    // bucket. Items already emitted by the interleave above are skipped.
    for (bucket, items, count) in [
        (
            MemoryPromptBucket::HybridRecall,
            candidates,
            candidate_count,
        ),
        (MemoryPromptBucket::ProfileFact, facts, fact_count),
        (MemoryPromptBucket::Summary, summaries, summary_count),
    ] {
        for item in items.iter().take(count) {
            if !selected
                .iter()
                .any(|existing| existing.bucket == bucket && existing.item.id == item.id)
            {
                selected.push(SelectedPromptItem {
                    item: item.clone(),
                    bucket,
                });
            }
        }
    }
    selected.truncate(MAX_MEMORY_PROMPT_ITEMS);
    selected
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

fn hybrid_candidate_prompt_item(candidate: &HybridMemoryCandidate) -> MemoryPromptItem {
    let record = &candidate.record.resource;
    let source_uri = hybrid_record_uri(record.kind(), record.id());
    let provenance_label = record
        .evidence()
        .iter()
        .map(render_evidence_source)
        .collect::<Vec<_>>()
        .join("; ");
    let text = match record {
        MemoryRecordResource::Episodic(record) => format!(
            "{} (confidence {:.2}; importance {:.2})",
            record.text, record.confidence, record.importance
        ),
        MemoryRecordResource::Semantic(record) => format!(
            "{} {} {} (confidence {:.2}; importance {:.2})",
            record.semantic_subject,
            record.predicate,
            record.object,
            record.confidence,
            record.importance
        ),
    };
    MemoryPromptItem {
        id: record.id(),
        label: format!(
            "{}:{}; lexical={:?}; dense={:?}; rrf={:.6}",
            record.kind().as_str(),
            short_id(record.id()),
            candidate.lexical_rank,
            candidate.dense_rank,
            candidate.rrf_score
        ),
        estimated_tokens: estimate_tokens(&text),
        text,
        provenance_label,
        source_uri,
    }
}

fn hybrid_candidate_trace(
    candidate: HybridMemoryCandidate,
    estimated_tokens: usize,
) -> MemoryCandidateTrace {
    let record = &candidate.record.resource;
    MemoryCandidateTrace {
        id: record.id(),
        kind: record.kind(),
        status: record.status(),
        source_uri: hybrid_record_uri(record.kind(), record.id()),
        evidence: record
            .evidence()
            .iter()
            .map(render_evidence_source)
            .collect(),
        lexical_rank: candidate.lexical_rank,
        lexical_score: candidate.lexical_score,
        dense_rank: candidate.dense_rank,
        dense_score: candidate.dense_score,
        embedding_version: candidate.embedding_version,
        rrf_score: candidate.rrf_score,
        estimated_tokens,
        included: false,
    }
}

fn hybrid_record_uri(kind: MemoryRecordKind, id: Uuid) -> String {
    format!("memory://records/{}/{id}", kind.as_str())
}

fn render_evidence_source(evidence: &tm_memory::MemoryRecordEvidence) -> String {
    match &evidence.source {
        MemoryEvidenceSource::SessionEvent {
            session_id,
            event_seq,
        } => format!("{}=event:{session_id}:{event_seq}", evidence.label),
        MemoryEvidenceSource::SessionMessage {
            session_id,
            message_seq,
        } => format!("{}=message:{session_id}:{message_seq}", evidence.label),
        MemoryEvidenceSource::Resource { uri } => format!("{}={uri}", evidence.label),
    }
}

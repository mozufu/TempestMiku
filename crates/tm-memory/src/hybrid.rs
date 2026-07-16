use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::StoredMemoryRecord;

pub const DEFAULT_RRF_K: u32 = 60;
pub const DEFAULT_HYBRID_CANDIDATE_LIMIT: usize = 24;
pub const DEFAULT_HYBRID_TOP_K: usize = 5;
pub const MAX_HYBRID_CANDIDATE_LIMIT: usize = 128;

/// Server-owned authority for one bounded hybrid-recall request.
///
/// This is deliberately a retrieval contract, not a prompt/context contract. P8.4 owns putting
/// the resulting records into a turn, so P8.3 can prove ranking and authority independently.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HybridRecallRequest {
    pub owner_subject: String,
    pub memory_scope: String,
    pub candidate_limit: usize,
    pub top_k: usize,
    pub rrf_k: u32,
}

impl Default for HybridRecallRequest {
    fn default() -> Self {
        Self {
            owner_subject: "brian".to_string(),
            memory_scope: "global".to_string(),
            candidate_limit: DEFAULT_HYBRID_CANDIDATE_LIMIT,
            top_k: DEFAULT_HYBRID_TOP_K,
            rrf_k: DEFAULT_RRF_K,
        }
    }
}

impl HybridRecallRequest {
    pub fn validate(&self) -> Result<(), HybridRecallError> {
        if self.owner_subject.trim().is_empty() {
            return Err(HybridRecallError::MissingOwnerSubject);
        }
        if self.memory_scope != "global"
            && self
                .memory_scope
                .strip_prefix("project:")
                .is_none_or(|slug| slug.trim().is_empty())
        {
            return Err(HybridRecallError::InvalidScope);
        }
        if self.candidate_limit == 0 || self.candidate_limit > MAX_HYBRID_CANDIDATE_LIMIT {
            return Err(HybridRecallError::InvalidCandidateLimit);
        }
        if self.top_k == 0 || self.top_k > self.candidate_limit {
            return Err(HybridRecallError::InvalidTopK);
        }
        if self.rrf_k == 0 {
            return Err(HybridRecallError::InvalidRrfK);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RankedMemoryCandidate {
    pub record: StoredMemoryRecord,
    pub rank: u32,
    pub score: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HybridMemoryCandidate {
    pub record: StoredMemoryRecord,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DenseRecallQuery {
    pub embedding_version: String,
    pub snapshot_revision: i64,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DenseRecallStatus {
    NotRequested,
    Applied,
    GenerationChanged,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HybridRecallResult {
    pub candidates: Vec<HybridMemoryCandidate>,
    pub dense_status: DenseRecallStatus,
}

impl HybridMemoryCandidate {
    fn new(record: StoredMemoryRecord) -> Self {
        Self {
            record,
            lexical_rank: None,
            lexical_score: None,
            dense_rank: None,
            dense_score: None,
            embedding_version: None,
            rrf_score: 0.0,
        }
    }
}

/// Fuses independently-produced lexical and dense rankings with bounded deterministic RRF.
///
/// Candidate query implementations must already enforce correction/supersession links. The fuser
/// repeats the local status/effective-time and owner/scope checks so a malformed provider result
/// cannot carry an unsupported or cross-scope record into later context budgeting.
pub fn fuse_hybrid_candidates(
    request: &HybridRecallRequest,
    lexical: impl IntoIterator<Item = RankedMemoryCandidate>,
    dense: impl IntoIterator<Item = RankedMemoryCandidate>,
) -> Result<Vec<HybridMemoryCandidate>, HybridRecallError> {
    request.validate()?;
    let mut fused = BTreeMap::<String, HybridMemoryCandidate>::new();

    for candidate in lexical {
        insert_candidate(request, &mut fused, candidate, CandidateSource::Lexical)?;
    }
    for candidate in dense {
        insert_candidate(request, &mut fused, candidate, CandidateSource::Dense)?;
    }

    let mut fused = fused.into_values().collect::<Vec<_>>();
    fused.sort_by(|left, right| {
        right
            .rrf_score
            .total_cmp(&left.rrf_score)
            .then_with(|| left.lexical_rank.cmp(&right.lexical_rank))
            .then_with(|| left.dense_rank.cmp(&right.dense_rank))
            .then_with(|| left.record.kind().cmp(&right.record.kind()))
            .then_with(|| left.record.id().cmp(&right.record.id()))
    });
    fused.truncate(request.top_k);
    Ok(fused)
}

#[derive(Clone, Copy)]
enum CandidateSource {
    Lexical,
    Dense,
}

fn insert_candidate(
    request: &HybridRecallRequest,
    fused: &mut BTreeMap<String, HybridMemoryCandidate>,
    candidate: RankedMemoryCandidate,
    source: CandidateSource,
) -> Result<(), HybridRecallError> {
    if candidate.rank == 0 || !candidate.score.is_finite() {
        return Err(HybridRecallError::InvalidCandidate);
    }
    if candidate.record.resource.owner_subject() != request.owner_subject
        || candidate.record.resource.memory_scope() != request.memory_scope
        || !candidate.record.resource.status().is_retrievable()
        || candidate.record.resource.effective_to().is_some()
    {
        return Ok(());
    }

    let key = format!(
        "{}:{}",
        candidate.record.kind().as_str(),
        candidate.record.id()
    );
    let fused_candidate = fused
        .entry(key)
        .or_insert_with(|| HybridMemoryCandidate::new(candidate.record.clone()));
    let rrf_score = 1.0 / (request.rrf_k + candidate.rank) as f32;
    match source {
        CandidateSource::Lexical => {
            if fused_candidate.lexical_rank.is_some() {
                return Err(HybridRecallError::DuplicateCandidateRank);
            }
            fused_candidate.lexical_rank = Some(candidate.rank);
            fused_candidate.lexical_score = Some(candidate.score);
        }
        CandidateSource::Dense => {
            if fused_candidate.dense_rank.is_some() {
                return Err(HybridRecallError::DuplicateCandidateRank);
            }
            fused_candidate.dense_rank = Some(candidate.rank);
            fused_candidate.dense_score = Some(candidate.score);
            fused_candidate.embedding_version = candidate.embedding_version;
        }
    }
    fused_candidate.rrf_score += rrf_score;
    Ok(())
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HybridRecallError {
    #[error("hybrid recall owner subject must not be empty")]
    MissingOwnerSubject,
    #[error("hybrid recall scope must be global or a non-empty project scope")]
    InvalidScope,
    #[error("hybrid candidate limit must be between 1 and {MAX_HYBRID_CANDIDATE_LIMIT}")]
    InvalidCandidateLimit,
    #[error("hybrid top-k must be between 1 and the candidate limit")]
    InvalidTopK,
    #[error("hybrid reciprocal-rank constant must be positive")]
    InvalidRrfK,
    #[error("hybrid candidate rank must be positive and score finite")]
    InvalidCandidate,
    #[error("a candidate was repeated in one retrieval stream")]
    DuplicateCandidateRank,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::{
        EpisodicMemoryRecord, MEMORY_RECORD_SCHEMA_VERSION, MemoryEvidenceSource,
        MemoryRecordEvidence, MemoryRecordLinks, MemoryRecordResource, MemoryRecordStatus,
        StoredMemoryRecord,
    };

    fn record(
        id: u128,
        owner: &str,
        scope: &str,
        status: MemoryRecordStatus,
    ) -> StoredMemoryRecord {
        StoredMemoryRecord::new(MemoryRecordResource::Episodic(EpisodicMemoryRecord {
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
            id: Uuid::from_u128(id),
            owner_subject: owner.to_string(),
            memory_scope: scope.to_string(),
            text: format!("record {id}"),
            evidence: vec![MemoryRecordEvidence {
                schema_version: MEMORY_RECORD_SCHEMA_VERSION,
                label: "fixture".to_string(),
                source: MemoryEvidenceSource::Resource {
                    uri: "memory://fixture".to_string(),
                },
            }],
            confidence: 1.0,
            importance: 0.8,
            observed_at: Utc::now(),
            effective_from: Utc::now(),
            effective_to: None,
            status,
            links: MemoryRecordLinks::default(),
            created_at: Utc::now(),
        }))
        .unwrap()
    }

    #[test]
    fn rrf_is_deterministic_and_preserves_both_rank_components() {
        let target = record(1, "brian", "global", MemoryRecordStatus::Active);
        let lexical_only = record(2, "brian", "global", MemoryRecordStatus::Active);
        let request = HybridRecallRequest::default();
        let fused = fuse_hybrid_candidates(
            &request,
            [
                RankedMemoryCandidate {
                    record: lexical_only,
                    rank: 1,
                    score: 0.9,
                    embedding_version: None,
                },
                RankedMemoryCandidate {
                    record: target.clone(),
                    rank: 2,
                    score: 0.8,
                    embedding_version: None,
                },
            ],
            [RankedMemoryCandidate {
                record: target.clone(),
                rank: 1,
                score: 0.95,
                embedding_version: Some("emb-v1-fixture".to_string()),
            }],
        )
        .unwrap();

        assert_eq!(fused[0].record.id(), target.id());
        assert_eq!(fused[0].lexical_rank, Some(2));
        assert_eq!(fused[0].dense_rank, Some(1));
        assert_eq!(
            fused[0].embedding_version.as_deref(),
            Some("emb-v1-fixture")
        );
        assert!((fused[0].rrf_score - (1.0 / 62.0 + 1.0 / 61.0)).abs() < 0.00001);
    }

    #[test]
    fn fuser_drops_cross_scope_and_unsupported_candidates_before_budgeting() {
        let request = HybridRecallRequest::default();
        let cross_scope = record(3, "brian", "project:other", MemoryRecordStatus::Active);
        let unsupported = record(4, "brian", "global", MemoryRecordStatus::Unsupported);
        let fused = fuse_hybrid_candidates(
            &request,
            [
                RankedMemoryCandidate {
                    record: cross_scope,
                    rank: 1,
                    score: 1.0,
                    embedding_version: None,
                },
                RankedMemoryCandidate {
                    record: unsupported,
                    rank: 2,
                    score: 0.8,
                    embedding_version: None,
                },
            ],
            [],
        )
        .unwrap();
        assert!(fused.is_empty());
    }
}

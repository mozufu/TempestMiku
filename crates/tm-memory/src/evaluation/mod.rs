use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{ProfileFactRecord, RecallChunkRecord};

pub const RECALL_EVALUATION_SCHEMA_VERSION: u16 = 1;
pub const RECALL_EVALUATOR_VERSION: &str = "p8.1-recall-eval-v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RecallEvaluationSplit {
    Tune,
    HeldOut,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RecallFixtureCoverage {
    Relevant,
    Irrelevant,
    Unsupported,
    Stale,
    Corrected,
    Superseded,
    CrossScope,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecallRecordQuality {
    Supported,
    Unsupported,
    Stale,
    Corrected,
    Superseded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecallFixtureRecord {
    ProfileFact {
        quality: RecallRecordQuality,
        record: ProfileFactRecord,
    },
    RecallChunk {
        owner_subject: String,
        quality: RecallRecordQuality,
        record: RecallChunkRecord,
    },
}

impl RecallFixtureRecord {
    pub fn id(&self) -> Uuid {
        match self {
            Self::ProfileFact { record, .. } => record.id,
            Self::RecallChunk { record, .. } => record.id,
        }
    }

    pub fn quality(&self) -> RecallRecordQuality {
        match self {
            Self::ProfileFact { quality, .. } | Self::RecallChunk { quality, .. } => *quality,
        }
    }

    pub fn is_authorized_for(&self, owner_subject: &str, memory_scope: &str) -> bool {
        match self {
            Self::ProfileFact { record, .. } => record.subject == owner_subject,
            Self::RecallChunk {
                owner_subject: record_owner,
                record,
                ..
            } => record_owner == owner_subject && record.scope == memory_scope,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecallRelevanceJudgment {
    pub record_id: Uuid,
    pub grade: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecallEvaluationCase {
    pub id: String,
    pub split: RecallEvaluationSplit,
    pub owner_subject: String,
    pub memory_scope: String,
    pub query: String,
    pub coverage: Vec<RecallFixtureCoverage>,
    pub relevance: Vec<RecallRelevanceJudgment>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RecallFalseInclusionKind {
    Unsupported,
    Stale,
    Corrected,
    Superseded,
    AuthorityScopeLeak,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RecallAcceptanceCohort {
    Overall,
    HeldOut,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecallAcceptancePolicy {
    pub min_relative_ndcg_at_5_improvement: f64,
    pub preserve_recall_at_5: bool,
    pub required_cohorts: Vec<RecallAcceptanceCohort>,
    pub zero_false_inclusions: Vec<RecallFalseInclusionKind>,
    pub max_prompt_tokens: usize,
    pub max_final_recall_items: usize,
    pub latency_p95_ceiling_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecallEvaluationManifest {
    pub schema_version: u16,
    pub evaluator_version: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub top_k: usize,
    pub recall_limit: usize,
    pub prompt_budget_tokens: usize,
    pub latency_samples_per_case: usize,
    pub acceptance: RecallAcceptancePolicy,
    pub records: Vec<RecallFixtureRecord>,
    pub cases: Vec<RecallEvaluationCase>,
}

impl RecallEvaluationManifest {
    pub fn parse(json: &str) -> Result<Self, RecallEvaluationError> {
        let manifest: Self = serde_json::from_str(json)
            .map_err(|error| RecallEvaluationError::InvalidManifest(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), RecallEvaluationError> {
        if self.schema_version != RECALL_EVALUATION_SCHEMA_VERSION {
            return Err(RecallEvaluationError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        if self.evaluator_version != RECALL_EVALUATOR_VERSION {
            return Err(RecallEvaluationError::UnsupportedEvaluatorVersion(
                self.evaluator_version.clone(),
            ));
        }
        if self.top_k != 5 || self.recall_limit != 5 || self.acceptance.max_final_recall_items != 5
        {
            return Err(RecallEvaluationError::InvalidManifest(
                "P8.1 freezes evaluation, recall candidates, and final recall at k=5".to_string(),
            ));
        }
        if self.prompt_budget_tokens != 1_600 || self.acceptance.max_prompt_tokens != 1_600 {
            return Err(RecallEvaluationError::InvalidManifest(
                "P8.1 freezes the memory prompt budget at 1600 tokens".to_string(),
            ));
        }
        if self.latency_samples_per_case == 0 {
            return Err(RecallEvaluationError::InvalidManifest(
                "latencySamplesPerCase must be positive".to_string(),
            ));
        }
        let record_ids = self
            .records
            .iter()
            .map(RecallFixtureRecord::id)
            .collect::<BTreeSet<_>>();
        if record_ids.len() != self.records.len() {
            return Err(RecallEvaluationError::InvalidManifest(
                "fixture record ids must be unique".to_string(),
            ));
        }
        let mut case_ids = BTreeSet::new();
        let mut splits = BTreeSet::new();
        let mut coverage = BTreeSet::new();
        for case in &self.cases {
            if case.id.trim().is_empty() || !case_ids.insert(case.id.clone()) {
                return Err(RecallEvaluationError::InvalidManifest(
                    "fixture case ids must be non-empty and unique".to_string(),
                ));
            }
            if case.owner_subject.trim().is_empty()
                || case.memory_scope.trim().is_empty()
                || case.query.trim().is_empty()
            {
                return Err(RecallEvaluationError::InvalidManifest(format!(
                    "case {} has empty authority or query fields",
                    case.id
                )));
            }
            splits.insert(case.split);
            coverage.extend(case.coverage.iter().copied());
            for judgment in &case.relevance {
                if judgment.grade == 0 || !record_ids.contains(&judgment.record_id) {
                    return Err(RecallEvaluationError::InvalidManifest(format!(
                        "case {} has an invalid relevance judgment",
                        case.id
                    )));
                }
                let record = self
                    .records
                    .iter()
                    .find(|record| record.id() == judgment.record_id)
                    .expect("record id checked above");
                if record.quality() != RecallRecordQuality::Supported
                    || !record.is_authorized_for(&case.owner_subject, &case.memory_scope)
                {
                    return Err(RecallEvaluationError::InvalidManifest(format!(
                        "case {} judges an unsafe or unauthorized record as relevant",
                        case.id
                    )));
                }
            }
            if case.relevance.is_empty() {
                return Err(RecallEvaluationError::InvalidManifest(format!(
                    "case {} must judge at least one supported record as relevant",
                    case.id
                )));
            }
        }
        if splits != BTreeSet::from([RecallEvaluationSplit::Tune, RecallEvaluationSplit::HeldOut]) {
            return Err(RecallEvaluationError::InvalidManifest(
                "manifest must include tune and held_out cases".to_string(),
            ));
        }
        let required_coverage = BTreeSet::from([
            RecallFixtureCoverage::Relevant,
            RecallFixtureCoverage::Irrelevant,
            RecallFixtureCoverage::Unsupported,
            RecallFixtureCoverage::Stale,
            RecallFixtureCoverage::Corrected,
            RecallFixtureCoverage::Superseded,
            RecallFixtureCoverage::CrossScope,
        ]);
        if !required_coverage.is_subset(&coverage) {
            return Err(RecallEvaluationError::InvalidManifest(
                "manifest is missing required P8.1 fixture coverage".to_string(),
            ));
        }
        if (self.acceptance.min_relative_ndcg_at_5_improvement - 0.10).abs() > f64::EPSILON
            || !self.acceptance.preserve_recall_at_5
        {
            return Err(RecallEvaluationError::InvalidManifest(
                "P8.1 acceptance must require +10% nDCG@5 without Recall@5 regression".to_string(),
            ));
        }
        let required_false_inclusions = BTreeSet::from([
            RecallFalseInclusionKind::Unsupported,
            RecallFalseInclusionKind::Stale,
            RecallFalseInclusionKind::Corrected,
            RecallFalseInclusionKind::Superseded,
            RecallFalseInclusionKind::AuthorityScopeLeak,
        ]);
        if self
            .acceptance
            .zero_false_inclusions
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            != required_false_inclusions
        {
            return Err(RecallEvaluationError::InvalidManifest(
                "P8.1 acceptance must reject every unsafe false-inclusion class".to_string(),
            ));
        }
        if self
            .acceptance
            .required_cohorts
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            != BTreeSet::from([
                RecallAcceptanceCohort::Overall,
                RecallAcceptanceCohort::HeldOut,
            ])
        {
            return Err(RecallEvaluationError::InvalidManifest(
                "P8.1 relevance and recall acceptance must hold overall and on held_out fixtures"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub fn sha256(json: &str) -> String {
        format!("{:x}", Sha256::digest(json.as_bytes()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecallEvaluationObservation {
    pub case_id: String,
    pub ranked_record_ids: Vec<Uuid>,
    pub prompt_tokens: usize,
    pub candidate_count: usize,
    pub latency_micros: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RecallFalseInclusionCounts {
    pub unsupported: usize,
    pub stale: usize,
    pub corrected: usize,
    pub superseded: usize,
    pub authority_scope_leaks: usize,
}

impl RecallFalseInclusionCounts {
    pub fn unsupported_or_stale_precision_failures(&self) -> usize {
        self.unsupported + self.stale
    }

    fn add_assign(&mut self, other: &Self) {
        self.unsupported += other.unsupported;
        self.stale += other.stale;
        self.corrected += other.corrected;
        self.superseded += other.superseded;
        self.authority_scope_leaks += other.authority_scope_leaks;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecallEvaluationCaseReport {
    pub case_id: String,
    pub split: RecallEvaluationSplit,
    pub ranked_record_ids: Vec<Uuid>,
    pub ndcg_at_5: f64,
    pub recall_at_5: f64,
    pub false_inclusions: RecallFalseInclusionCounts,
    pub unsupported_or_stale_precision_failures: usize,
    pub prompt_tokens: usize,
    pub candidate_count: usize,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecallEvaluationAggregate {
    pub case_count: usize,
    pub mean_ndcg_at_5: f64,
    pub mean_recall_at_5: f64,
    pub false_inclusions: RecallFalseInclusionCounts,
    pub unsupported_or_stale_precision_failures: usize,
    pub mean_prompt_tokens: f64,
    pub max_prompt_tokens: usize,
    pub mean_candidate_count: f64,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecallEvaluationSplitReport {
    pub split: RecallEvaluationSplit,
    pub metrics: RecallEvaluationAggregate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecallEvaluationReport {
    pub schema_version: u16,
    pub evaluator_version: String,
    pub manifest_sha256: String,
    pub retrieval_mode: String,
    pub measured_at: DateTime<Utc>,
    pub acceptance: RecallAcceptancePolicy,
    pub cases: Vec<RecallEvaluationCaseReport>,
    pub splits: Vec<RecallEvaluationSplitReport>,
    pub overall: RecallEvaluationAggregate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecallBaselineEnvironment {
    pub database: String,
    pub retrieval: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecallBaselineArtifact {
    pub schema_version: u16,
    pub captured_at: DateTime<Utc>,
    pub environment: RecallBaselineEnvironment,
    pub report: RecallEvaluationReport,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecallDeterministicReport {
    pub schema_version: u16,
    pub evaluator_version: String,
    pub manifest_sha256: String,
    pub retrieval_mode: String,
    pub acceptance: RecallAcceptancePolicy,
    pub cases: Vec<RecallDeterministicCaseReport>,
    pub splits: Vec<RecallDeterministicSplitReport>,
    pub overall: RecallDeterministicAggregate,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecallDeterministicCaseReport {
    pub case_id: String,
    pub split: RecallEvaluationSplit,
    pub ranked_record_ids: Vec<Uuid>,
    pub ndcg_at_5: f64,
    pub recall_at_5: f64,
    pub false_inclusions: RecallFalseInclusionCounts,
    pub unsupported_or_stale_precision_failures: usize,
    pub prompt_tokens: usize,
    pub candidate_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecallDeterministicAggregate {
    pub case_count: usize,
    pub mean_ndcg_at_5: f64,
    pub mean_recall_at_5: f64,
    pub false_inclusions: RecallFalseInclusionCounts,
    pub unsupported_or_stale_precision_failures: usize,
    pub mean_prompt_tokens: f64,
    pub max_prompt_tokens: usize,
    pub mean_candidate_count: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecallDeterministicSplitReport {
    pub split: RecallEvaluationSplit,
    pub metrics: RecallDeterministicAggregate,
}

mod deterministic;
mod error;
mod evaluator;

pub use error::RecallEvaluationError;
pub use evaluator::evaluate_recall_observations;

#[cfg(test)]
mod tests;

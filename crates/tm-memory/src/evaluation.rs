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

impl RecallEvaluationReport {
    pub fn deterministic(&self) -> RecallDeterministicReport {
        RecallDeterministicReport {
            schema_version: self.schema_version,
            evaluator_version: self.evaluator_version.clone(),
            manifest_sha256: self.manifest_sha256.clone(),
            retrieval_mode: self.retrieval_mode.clone(),
            acceptance: self.acceptance.clone(),
            cases: self
                .cases
                .iter()
                .map(|case| RecallDeterministicCaseReport {
                    case_id: case.case_id.clone(),
                    split: case.split,
                    ranked_record_ids: case.ranked_record_ids.clone(),
                    ndcg_at_5: case.ndcg_at_5,
                    recall_at_5: case.recall_at_5,
                    false_inclusions: case.false_inclusions.clone(),
                    unsupported_or_stale_precision_failures: case
                        .unsupported_or_stale_precision_failures,
                    prompt_tokens: case.prompt_tokens,
                    candidate_count: case.candidate_count,
                })
                .collect(),
            splits: self
                .splits
                .iter()
                .map(|split| RecallDeterministicSplitReport {
                    split: split.split,
                    metrics: deterministic_aggregate(&split.metrics),
                })
                .collect(),
            overall: deterministic_aggregate(&self.overall),
        }
    }
}

fn deterministic_aggregate(aggregate: &RecallEvaluationAggregate) -> RecallDeterministicAggregate {
    RecallDeterministicAggregate {
        case_count: aggregate.case_count,
        mean_ndcg_at_5: aggregate.mean_ndcg_at_5,
        mean_recall_at_5: aggregate.mean_recall_at_5,
        false_inclusions: aggregate.false_inclusions.clone(),
        unsupported_or_stale_precision_failures: aggregate.unsupported_or_stale_precision_failures,
        mean_prompt_tokens: aggregate.mean_prompt_tokens,
        max_prompt_tokens: aggregate.max_prompt_tokens,
        mean_candidate_count: aggregate.mean_candidate_count,
    }
}

pub fn evaluate_recall_observations(
    manifest_json: &str,
    retrieval_mode: impl Into<String>,
    measured_at: DateTime<Utc>,
    observations: Vec<RecallEvaluationObservation>,
) -> Result<RecallEvaluationReport, RecallEvaluationError> {
    let manifest = RecallEvaluationManifest::parse(manifest_json)?;
    let observations = observations
        .into_iter()
        .map(|observation| (observation.case_id.clone(), observation))
        .collect::<BTreeMap<_, _>>();
    if observations.len() != manifest.cases.len()
        || manifest
            .cases
            .iter()
            .any(|case| !observations.contains_key(&case.id))
    {
        return Err(RecallEvaluationError::ObservationSetMismatch);
    }
    let records = manifest
        .records
        .iter()
        .map(|record| (record.id(), record))
        .collect::<BTreeMap<_, _>>();
    let mut case_reports = Vec::with_capacity(manifest.cases.len());
    for case in &manifest.cases {
        let observation = &observations[&case.id];
        if observation.latency_micros.len() != manifest.latency_samples_per_case {
            return Err(RecallEvaluationError::InvalidLatencySampleCount {
                case_id: case.id.clone(),
                expected: manifest.latency_samples_per_case,
                actual: observation.latency_micros.len(),
            });
        }
        let top_ids = observation
            .ranked_record_ids
            .iter()
            .copied()
            .take(manifest.top_k)
            .collect::<Vec<_>>();
        if observation
            .ranked_record_ids
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != observation.ranked_record_ids.len()
            || observation.candidate_count != observation.ranked_record_ids.len()
        {
            return Err(RecallEvaluationError::InvalidObservedCandidates(
                case.id.clone(),
            ));
        }
        if top_ids.iter().any(|id| !records.contains_key(id)) {
            return Err(RecallEvaluationError::UnknownObservedRecord(
                case.id.clone(),
            ));
        }
        let relevance = case
            .relevance
            .iter()
            .map(|judgment| (judgment.record_id, judgment.grade))
            .collect::<BTreeMap<_, _>>();
        let false_inclusions = count_false_inclusions(case, &top_ids, &records);
        let latency = observation
            .latency_micros
            .iter()
            .map(|micros| *micros as f64 / 1_000.0)
            .collect::<Vec<_>>();
        case_reports.push(RecallEvaluationCaseReport {
            case_id: case.id.clone(),
            split: case.split,
            ranked_record_ids: observation.ranked_record_ids.clone(),
            ndcg_at_5: round_metric(ndcg_at_k(&top_ids, &relevance, manifest.top_k)),
            recall_at_5: round_metric(recall_at_k(&top_ids, &relevance)),
            unsupported_or_stale_precision_failures: false_inclusions
                .unsupported_or_stale_precision_failures(),
            false_inclusions,
            prompt_tokens: observation.prompt_tokens,
            candidate_count: observation.candidate_count,
            latency_p50_ms: round_metric(percentile(&latency, 0.50)),
            latency_p95_ms: round_metric(percentile(&latency, 0.95)),
        });
    }
    let splits = [RecallEvaluationSplit::Tune, RecallEvaluationSplit::HeldOut]
        .into_iter()
        .map(|split| RecallEvaluationSplitReport {
            split,
            metrics: aggregate(
                case_reports.iter().filter(|report| report.split == split),
                &observations,
            ),
        })
        .collect();
    let overall = aggregate(case_reports.iter(), &observations);
    Ok(RecallEvaluationReport {
        schema_version: RECALL_EVALUATION_SCHEMA_VERSION,
        evaluator_version: RECALL_EVALUATOR_VERSION.to_string(),
        manifest_sha256: RecallEvaluationManifest::sha256(manifest_json),
        retrieval_mode: retrieval_mode.into(),
        measured_at,
        acceptance: manifest.acceptance,
        cases: case_reports,
        splits,
        overall,
    })
}

fn count_false_inclusions(
    case: &RecallEvaluationCase,
    top_ids: &[Uuid],
    records: &BTreeMap<Uuid, &RecallFixtureRecord>,
) -> RecallFalseInclusionCounts {
    let mut counts = RecallFalseInclusionCounts::default();
    for id in top_ids {
        let record = records[id];
        if !record.is_authorized_for(&case.owner_subject, &case.memory_scope) {
            counts.authority_scope_leaks += 1;
        }
        match record.quality() {
            RecallRecordQuality::Supported => {}
            RecallRecordQuality::Unsupported => counts.unsupported += 1,
            RecallRecordQuality::Stale => counts.stale += 1,
            RecallRecordQuality::Corrected => counts.corrected += 1,
            RecallRecordQuality::Superseded => counts.superseded += 1,
        }
    }
    counts
}

fn ndcg_at_k(ranked: &[Uuid], relevance: &BTreeMap<Uuid, u8>, k: usize) -> f64 {
    let dcg = ranked
        .iter()
        .take(k)
        .enumerate()
        .map(|(index, id)| gain(*relevance.get(id).unwrap_or(&0), index))
        .sum::<f64>();
    let mut ideal = relevance.values().copied().collect::<Vec<_>>();
    ideal.sort_unstable_by(|left, right| right.cmp(left));
    let idcg = ideal
        .into_iter()
        .take(k)
        .enumerate()
        .map(|(index, grade)| gain(grade, index))
        .sum::<f64>();
    if idcg == 0.0 { 0.0 } else { dcg / idcg }
}

fn gain(grade: u8, zero_based_rank: usize) -> f64 {
    ((2_u64.pow(u32::from(grade)) - 1) as f64) / ((zero_based_rank + 2) as f64).log2()
}

fn recall_at_k(ranked: &[Uuid], relevance: &BTreeMap<Uuid, u8>) -> f64 {
    if relevance.is_empty() {
        return 1.0;
    }
    ranked
        .iter()
        .filter(|id| relevance.contains_key(id))
        .count() as f64
        / relevance.len() as f64
}

fn aggregate<'a>(
    reports: impl Iterator<Item = &'a RecallEvaluationCaseReport>,
    observations: &BTreeMap<String, RecallEvaluationObservation>,
) -> RecallEvaluationAggregate {
    let reports = reports.collect::<Vec<_>>();
    let count = reports.len();
    let mut false_inclusions = RecallFalseInclusionCounts::default();
    let mut latency = Vec::new();
    for report in &reports {
        false_inclusions.add_assign(&report.false_inclusions);
        latency.extend(
            observations[&report.case_id]
                .latency_micros
                .iter()
                .map(|micros| *micros as f64 / 1_000.0),
        );
    }
    let divisor = count.max(1) as f64;
    RecallEvaluationAggregate {
        case_count: count,
        mean_ndcg_at_5: round_metric(
            reports.iter().map(|report| report.ndcg_at_5).sum::<f64>() / divisor,
        ),
        mean_recall_at_5: round_metric(
            reports.iter().map(|report| report.recall_at_5).sum::<f64>() / divisor,
        ),
        unsupported_or_stale_precision_failures: false_inclusions
            .unsupported_or_stale_precision_failures(),
        false_inclusions,
        mean_prompt_tokens: round_metric(
            reports
                .iter()
                .map(|report| report.prompt_tokens as f64)
                .sum::<f64>()
                / divisor,
        ),
        max_prompt_tokens: reports
            .iter()
            .map(|report| report.prompt_tokens)
            .max()
            .unwrap_or(0),
        mean_candidate_count: round_metric(
            reports
                .iter()
                .map(|report| report.candidate_count as f64)
                .sum::<f64>()
                / divisor,
        ),
        latency_p50_ms: round_metric(percentile(&latency, 0.50)),
        latency_p95_ms: round_metric(percentile(&latency, 0.95)),
    }
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    let index = ((percentile * values.len() as f64).ceil() as usize)
        .saturating_sub(1)
        .min(values.len() - 1);
    values[index]
}

fn round_metric(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RecallEvaluationError {
    #[error("invalid recall evaluation manifest: {0}")]
    InvalidManifest(String),
    #[error("unsupported recall evaluation schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("unsupported recall evaluator version {0}")]
    UnsupportedEvaluatorVersion(String),
    #[error("recall observations do not exactly match the fixture cases")]
    ObservationSetMismatch,
    #[error("case {case_id} has {actual} latency samples; expected {expected}")]
    InvalidLatencySampleCount {
        case_id: String,
        expected: usize,
        actual: usize,
    },
    #[error("case {0} returned a record outside the fixture manifest")]
    UnknownObservedRecord(String),
    #[error("case {0} returned duplicate or inconsistent candidate ids")]
    InvalidObservedCandidates(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_MANIFEST: &str = r#"{
      "schemaVersion": 1,
      "evaluatorVersion": "p8.1-recall-eval-v1",
      "name": "contract-test",
      "createdAt": "2026-07-15T00:00:00Z",
      "topK": 5,
      "recallLimit": 5,
      "promptBudgetTokens": 1600,
      "latencySamplesPerCase": 1,
      "acceptance": {
        "minRelativeNdcgAt5Improvement": 0.1,
        "preserveRecallAt5": true,
        "requiredCohorts": ["overall", "held_out"],
        "zeroFalseInclusions": ["unsupported", "stale", "corrected", "superseded", "authority_scope_leak"],
        "maxPromptTokens": 1600,
        "maxFinalRecallItems": 5,
        "latencyP95CeilingMs": 100.0
      },
      "records": [{
        "kind": "recall_chunk",
        "owner_subject": "brian",
        "quality": "supported",
        "record": {
          "id": "10000000-0000-0000-0000-000000000001",
          "scope": "global",
          "text": "supported",
          "source": "test",
          "importance": 1.0,
          "created_at": "2026-07-15T00:00:00Z"
        }
      }],
      "cases": [
        {
          "id": "tune",
          "split": "tune",
          "ownerSubject": "brian",
          "memoryScope": "global",
          "query": "supported",
          "coverage": ["relevant", "irrelevant", "unsupported", "stale", "corrected", "superseded", "cross_scope"],
          "relevance": [{"recordId": "10000000-0000-0000-0000-000000000001", "grade": 3}]
        },
        {
          "id": "held",
          "split": "held_out",
          "ownerSubject": "brian",
          "memoryScope": "global",
          "query": "supported",
          "coverage": ["relevant"],
          "relevance": [{"recordId": "10000000-0000-0000-0000-000000000001", "grade": 3}]
        }
      ]
    }"#;

    #[test]
    fn evaluator_reports_relevance_budgets_candidates_and_latency() {
        let id = Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap();
        let observations = ["tune", "held"]
            .into_iter()
            .map(|case_id| RecallEvaluationObservation {
                case_id: case_id.to_string(),
                ranked_record_ids: vec![id],
                prompt_tokens: 12,
                candidate_count: 1,
                latency_micros: vec![1_500],
            })
            .collect();
        let report = evaluate_recall_observations(
            MINIMAL_MANIFEST,
            "test",
            "2026-07-15T00:00:00Z".parse().unwrap(),
            observations,
        )
        .unwrap();

        assert_eq!(report.overall.mean_ndcg_at_5, 1.0);
        assert_eq!(report.overall.mean_recall_at_5, 1.0);
        assert_eq!(report.overall.mean_prompt_tokens, 12.0);
        assert_eq!(report.overall.mean_candidate_count, 1.0);
        assert_eq!(report.overall.latency_p95_ms, 1.5);
        assert_eq!(report.splits.len(), 2);
        assert_eq!(report.deterministic().cases.len(), 2);
    }

    #[test]
    fn manifest_rejects_a_recall_limit_that_widens_the_frozen_bound() {
        let widened = MINIMAL_MANIFEST.replace("\"recallLimit\": 5", "\"recallLimit\": 6");
        assert_eq!(
            RecallEvaluationManifest::parse(&widened),
            Err(RecallEvaluationError::InvalidManifest(
                "P8.1 freezes evaluation, recall candidates, and final recall at k=5".to_string()
            ))
        );
    }
}

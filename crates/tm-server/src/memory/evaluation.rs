use std::time::Instant;

use chrono::Utc;
use tm_memory::{
    RecallEvaluationManifest, RecallEvaluationObservation, RecallEvaluationReport,
    evaluate_recall_observations,
};

use crate::{MemoryProvider, Result, ServerError, Store};

use super::MemoryContext;

pub const POSTGRES_LEXICAL_BASELINE_MODE: &str = "postgres_fts_profile_v1";
pub const IN_MEMORY_LEXICAL_BASELINE_MODE: &str = "in_memory_substring_profile_v1";
pub const POSTGRES_HYBRID_RECALL_MODE: &str = "postgres_fts_pgvector_rrf_v1";

/// Replays the frozen P8.1 fixture against the existing profile/summary/lexical path.
///
/// Callers own database isolation and fixture seeding. The evaluator is read-only: it uses the
/// same store methods and prompt budgeter as a normal turn and records every latency sample.
pub async fn evaluate_lexical_recall_baseline<S>(
    store: &S,
    manifest_json: &str,
    retrieval_mode: &str,
) -> Result<RecallEvaluationReport>
where
    S: Store,
{
    let manifest = RecallEvaluationManifest::parse(manifest_json)
        .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
    let mut observations = Vec::with_capacity(manifest.cases.len());
    for case in &manifest.cases {
        let mut latency_micros = Vec::with_capacity(manifest.latency_samples_per_case);
        let mut stable_result = None;
        for _ in 0..manifest.latency_samples_per_case {
            let started = Instant::now();
            let facts = store.profile_facts(&case.owner_subject).await?;
            let chunks = store
                .recall_chunks(&case.memory_scope, &case.query, manifest.recall_limit)
                .await?;
            let summaries = store.memory_summaries(&case.memory_scope, 3).await?;
            let context = MemoryContext::from_records_with_summaries(
                &case.owner_subject,
                &case.memory_scope,
                facts,
                summaries,
                chunks,
                manifest.prompt_budget_tokens,
            );
            latency_micros.push(started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64);
            let ranked_record_ids = context
                .profile_facts
                .iter()
                .chain(context.summaries.iter())
                .chain(context.recall_chunks.iter())
                .map(|item| item.id)
                .collect::<Vec<_>>();
            let result = (
                ranked_record_ids,
                context.budget.used_estimated_tokens,
                context.budget.included_profile_facts
                    + context.budget.included_summaries
                    + context.budget.included_recall_chunks,
            );
            if let Some(previous) = &stable_result {
                if previous != &result {
                    return Err(ServerError::Store(format!(
                        "recall fixture {} returned non-deterministic candidates",
                        case.id
                    )));
                }
            } else {
                stable_result = Some(result);
            }
        }
        let (ranked_record_ids, prompt_tokens, candidate_count) =
            stable_result.expect("positive latency sample count is validated by the manifest");
        observations.push(RecallEvaluationObservation {
            case_id: case.id.clone(),
            ranked_record_ids,
            prompt_tokens,
            candidate_count,
            latency_micros,
        });
    }
    evaluate_recall_observations(manifest_json, retrieval_mode, Utc::now(), observations)
        .map_err(|error| ServerError::Store(error.to_string()))
}

/// Replays the frozen P8 fixture through the same configured provider used by a real turn.
/// Hybrid items remain first in their fused order; a visible provider-loss fallback is evaluated
/// through the legacy profile/summary/lexical ordering returned by that provider.
pub async fn evaluate_memory_provider_recall<M>(
    provider: &M,
    manifest_json: &str,
    retrieval_mode: &str,
) -> Result<RecallEvaluationReport>
where
    M: MemoryProvider,
{
    let manifest = RecallEvaluationManifest::parse(manifest_json)
        .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
    let mut observations = Vec::with_capacity(manifest.cases.len());
    for case in &manifest.cases {
        let mut latency_micros = Vec::with_capacity(manifest.latency_samples_per_case);
        let mut stable_result = None;
        for _ in 0..manifest.latency_samples_per_case {
            let started = Instant::now();
            let context = provider
                .context_for_turn(&case.owner_subject, &case.memory_scope, &case.query)
                .await?;
            latency_micros.push(started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64);
            let ranked_record_ids = context
                .hybrid_recall
                .iter()
                .chain(context.profile_facts.iter())
                .chain(context.summaries.iter())
                .chain(context.recall_chunks.iter())
                .map(|item| item.id)
                .collect::<Vec<_>>();
            let result = (
                ranked_record_ids,
                context.budget.used_estimated_tokens,
                context.budget.included_hybrid_recall
                    + context.budget.included_profile_facts
                    + context.budget.included_summaries
                    + context.budget.included_recall_chunks,
            );
            if let Some(previous) = &stable_result {
                if previous != &result {
                    return Err(ServerError::Store(format!(
                        "recall fixture {} returned non-deterministic candidates",
                        case.id
                    )));
                }
            } else {
                stable_result = Some(result);
            }
        }
        let (ranked_record_ids, prompt_tokens, candidate_count) =
            stable_result.expect("positive latency sample count is validated by the manifest");
        observations.push(RecallEvaluationObservation {
            case_id: case.id.clone(),
            ranked_record_ids,
            prompt_tokens,
            candidate_count,
            latency_micros,
        });
    }
    evaluate_recall_observations(manifest_json, retrieval_mode, Utc::now(), observations)
        .map_err(|error| ServerError::Store(error.to_string()))
}

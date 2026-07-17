use super::*;
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

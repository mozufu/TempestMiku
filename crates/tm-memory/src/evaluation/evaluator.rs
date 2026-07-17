use super::*;
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

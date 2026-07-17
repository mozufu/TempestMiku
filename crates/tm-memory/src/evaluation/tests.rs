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

use chrono::Utc;
use serde_json::json;
use std::{sync::Arc, time::Instant};
use tm_memory::{
    EmbeddingClient, EmbeddingConfig, EmbeddingInput, EmbeddingProvider, EmbeddingRequest,
    EmbeddingResponse, EmbeddingVector, EpisodicMemoryRecord, HybridMemoryCandidate,
    MemoryEmbeddingJobClaim, MemoryEvidenceRef, MemoryRecordEvidence, MemoryRecordLinks,
    MemoryRecordResource, MemoryRecordStatus, MemorySummaryKind, NewMemoryEmbeddingGeneration,
    NewMemorySummaryRecord, RecallBaselineArtifact, RecallBaselineEnvironment,
    RecallEvaluationManifest, RecallFixtureRecord, RecallRecordQuality, SemanticMemoryRecord,
    StoredMemoryRecord,
};
use tm_modes::{AssetStatus, ModeId};
use uuid::Uuid;

use crate::store::{ProfileFactRecord, RecallChunkRecord};
use crate::{InMemoryStore, NewSession, PostgresStore, ServerError, Store};

use super::util::short_id;
use super::*;

const P8_RECALL_MANIFEST_JSON: &str =
    include_str!("../../../tests/fixtures/p8_recall_v1/manifest.json");
const P8_POSTGRES_BASELINE_JSON: &str =
    include_str!("../../../../../docs/evidence/2026-07-15-p8-1-lexical-baseline.json");

fn postgres_test_dsn() -> Option<String> {
    (std::env::var("TM_POSTGRES_TESTS").ok().as_deref() == Some("1"))
        .then(|| {
            std::env::var("TM_TEST_DATABASE_URL")
                .or_else(|_| std::env::var("TM_DATABASE_URL"))
                .ok()
        })
        .flatten()
}

struct ConstantEmbeddingClient;

#[async_trait::async_trait]
impl tm_memory::EmbeddingClient for ConstantEmbeddingClient {
    async fn embed(
        &self,
        request: tm_memory::EmbeddingRequest,
    ) -> std::result::Result<EmbeddingResponse, tm_memory::EmbeddingError> {
        let dimensions = request.config.dimensions.unwrap();
        let mut values = vec![0.0; dimensions];
        values[0] = 1.0;
        Ok(EmbeddingResponse {
            embedding_version: request.config.embedding_version()?.unwrap(),
            vectors: request
                .inputs
                .into_iter()
                .map(|input| EmbeddingVector {
                    id: input.id,
                    values: values.clone(),
                })
                .collect(),
        })
    }
}

struct UnavailableEmbeddingClient;

#[async_trait::async_trait]
impl tm_memory::EmbeddingClient for UnavailableEmbeddingClient {
    async fn embed(
        &self,
        _request: tm_memory::EmbeddingRequest,
    ) -> std::result::Result<EmbeddingResponse, tm_memory::EmbeddingError> {
        Err(tm_memory::EmbeddingError::Transport(
            "fixture provider unavailable".to_string(),
        ))
    }
}

async fn seed_p8_recall_fixture<S: Store>(store: &S, manifest: &RecallEvaluationManifest) {
    for fixture in &manifest.records {
        match fixture {
            RecallFixtureRecord::ProfileFact { record, .. } => {
                store.add_profile_fact(record.clone()).await.unwrap();
            }
            RecallFixtureRecord::RecallChunk { record, .. } => {
                store.add_recall_chunk(record.clone()).await.unwrap();
            }
        }
    }
}

async fn seed_p8_hybrid_fixture<S: Store>(store: &S, manifest: &RecallEvaluationManifest) {
    let mut records = std::collections::BTreeMap::<Uuid, StoredMemoryRecord>::new();
    for fixture in &manifest.records {
        let (quality, mut resource) = match fixture {
            RecallFixtureRecord::ProfileFact { quality, record } => (
                *quality,
                MemoryRecordResource::Semantic(
                    SemanticMemoryRecord::from_profile_fact(
                        record.subject.clone(),
                        "global",
                        record.clone(),
                        vec![MemoryRecordEvidence::resource(
                            record.provenance.clone(),
                            "P8 recall fixture",
                        )],
                    )
                    .unwrap(),
                ),
            ),
            RecallFixtureRecord::RecallChunk {
                owner_subject,
                quality,
                record,
            } => (
                *quality,
                MemoryRecordResource::Episodic(
                    EpisodicMemoryRecord::from_recall_chunk(
                        owner_subject.clone(),
                        record.clone(),
                        vec![MemoryRecordEvidence::resource(
                            record.source.clone(),
                            "P8 recall fixture",
                        )],
                    )
                    .unwrap(),
                ),
            ),
        };
        let status = match quality {
            RecallRecordQuality::Supported => MemoryRecordStatus::Active,
            RecallRecordQuality::Unsupported => MemoryRecordStatus::Unsupported,
            RecallRecordQuality::Stale => MemoryRecordStatus::Withheld,
            RecallRecordQuality::Corrected => MemoryRecordStatus::Corrected,
            RecallRecordQuality::Superseded => MemoryRecordStatus::Superseded,
        };
        match &mut resource {
            MemoryRecordResource::Episodic(record) => {
                record.status = status;
                record.effective_to = (!status.is_retrievable()).then_some(record.observed_at);
                record.links = MemoryRecordLinks::default();
            }
            MemoryRecordResource::Semantic(record) => {
                record.status = status;
                record.effective_to = (!status.is_retrievable()).then_some(record.observed_at);
                record.links = MemoryRecordLinks::default();
            }
        }
        let record = StoredMemoryRecord::new(resource).unwrap();
        records.insert(record.id(), record);
    }
    for record in records.values().cloned() {
        store.upsert_memory_record(record).await.unwrap();
    }
    for (old, new, correction) in [
        (
            "10000000-0000-0000-0000-000000000003",
            "10000000-0000-0000-0000-000000000002",
            false,
        ),
        (
            "20000000-0000-0000-0000-000000000101",
            "20000000-0000-0000-0000-000000000102",
            true,
        ),
        (
            "20000000-0000-0000-0000-000000000203",
            "20000000-0000-0000-0000-000000000204",
            false,
        ),
    ] {
        let old = Uuid::parse_str(old).unwrap();
        let new = Uuid::parse_str(new).unwrap();
        let mut old_record = records.get(&old).unwrap().clone();
        let mut new_record = records.get(&new).unwrap().clone();
        if correction {
            match &mut old_record.resource {
                MemoryRecordResource::Episodic(record) => {
                    record.links.corrected_by_record_id = Some(new)
                }
                MemoryRecordResource::Semantic(record) => {
                    record.links.corrected_by_record_id = Some(new)
                }
            }
            match &mut new_record.resource {
                MemoryRecordResource::Episodic(record) => {
                    record.links.corrects_record_id = Some(old)
                }
                MemoryRecordResource::Semantic(record) => {
                    record.links.corrects_record_id = Some(old)
                }
            }
        } else {
            match &mut old_record.resource {
                MemoryRecordResource::Episodic(record) => {
                    record.links.superseded_by_record_id = Some(new)
                }
                MemoryRecordResource::Semantic(record) => {
                    record.links.superseded_by_record_id = Some(new)
                }
            }
            match &mut new_record.resource {
                MemoryRecordResource::Episodic(record) => {
                    record.links.supersedes_record_id = Some(old)
                }
                MemoryRecordResource::Semantic(record) => {
                    record.links.supersedes_record_id = Some(old)
                }
            }
        }
        store
            .upsert_memory_record(StoredMemoryRecord::new(old_record.resource).unwrap())
            .await
            .unwrap();
        store
            .upsert_memory_record(StoredMemoryRecord::new(new_record.resource).unwrap())
            .await
            .unwrap();
    }
}

async fn assert_p8_record_contracts_compile_through_store<S: Store>(store: &S) {
    let fact_id = Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap();
    let fact = store.profile_fact("brian", fact_id).await.unwrap();
    let semantic = SemanticMemoryRecord::from_profile_fact(
        "brian",
        "global",
        fact,
        vec![MemoryRecordEvidence::resource(
            format!("memory://profile/brian/facts/{fact_id}"),
            "legacy profile fixture",
        )],
    )
    .unwrap();
    let semantic_resource = MemoryRecordResource::Semantic(semantic);
    semantic_resource.validate().unwrap();
    assert_eq!(
        serde_json::to_value(&semantic_resource).unwrap()["record"]["ownerSubject"],
        json!("brian")
    );

    let chunk_id = Uuid::parse_str("20000000-0000-0000-0000-000000000102").unwrap();
    let chunk = store.recall_chunk("global", chunk_id).await.unwrap();
    let episodic = EpisodicMemoryRecord::from_recall_chunk(
        "brian",
        chunk,
        vec![MemoryRecordEvidence::resource(
            format!("memory://scopes/global/chunks/{chunk_id}"),
            "legacy recall fixture",
        )],
    )
    .unwrap();
    let episodic_resource = MemoryRecordResource::Episodic(episodic);
    episodic_resource.validate().unwrap();
    assert_eq!(
        serde_json::to_value(&episodic_resource).unwrap()["record"]["memoryScope"],
        json!("global")
    );
}

mod hybrid_evaluation;
mod lexical_evaluation;
mod prompt_context;
mod state_capture;

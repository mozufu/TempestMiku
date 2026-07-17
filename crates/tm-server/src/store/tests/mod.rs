use chrono::{Duration, Utc};
use serde_json::json;
use tm_memory::{
    DenseRecallQuery, DenseRecallStatus, DreamReason, DreamStatus,
    EMBEDDING_PROVENANCE_SCHEMA_VERSION, EmbeddingConfig, EmbeddingNormalization,
    EmbeddingProvenance, EmbeddingProvider, EpisodicMemoryRecord, HybridRecallRequest,
    MEMORY_RECORD_SCHEMA_VERSION, MemoryEmbeddingGenerationStatus, MemoryEmbeddingJobClaim,
    MemoryEvidenceRef, MemoryEvidenceSource, MemoryRecordEvidence, MemoryRecordKind,
    MemoryRecordLinks, MemoryRecordStatus, MemorySchemaReadiness, MemorySummaryKind,
    NewDreamQueueRecord, NewMemoryEmbeddingGeneration, NewMemoryEmbeddingJob,
    NewMemorySummaryRecord, NewSkillProposalRecord, PgVectorReadiness, RecallChunkRecord,
    ReembeddingState, SkillVerification, StoredMemoryRecord,
};
use tm_modes::{AssetStatus, ModeId};
use uuid::Uuid;

use super::*;
use crate::{
    AuthDeviceStore, FakePushProvider, PostgresPushStore, PushCipher, PushMessageKind, PushService,
    ServerError,
};
use std::sync::Arc;

struct ProviderDown;

#[async_trait::async_trait]
impl tm_memory::EmbeddingClient for ProviderDown {
    async fn embed(
        &self,
        _request: tm_memory::EmbeddingRequest,
    ) -> std::result::Result<tm_memory::EmbeddingResponse, tm_memory::EmbeddingError> {
        Err(tm_memory::EmbeddingError::Transport(
            "fixture local provider is unavailable".to_string(),
        ))
    }
}

struct UnitEmbeddingClient;

#[async_trait::async_trait]
impl tm_memory::EmbeddingClient for UnitEmbeddingClient {
    async fn embed(
        &self,
        request: tm_memory::EmbeddingRequest,
    ) -> std::result::Result<tm_memory::EmbeddingResponse, tm_memory::EmbeddingError> {
        let dimensions = request.config.dimensions.unwrap();
        let mut values = vec![0.0; dimensions];
        values[0] = 1.0;
        Ok(tm_memory::EmbeddingResponse {
            embedding_version: request.config.embedding_version()?.unwrap(),
            vectors: request
                .inputs
                .into_iter()
                .map(|input| tm_memory::EmbeddingVector {
                    id: input.id,
                    values: values.clone(),
                })
                .collect(),
        })
    }
}

fn postgres_test_dsn() -> Option<String> {
    (std::env::var("TM_POSTGRES_TESTS").ok().as_deref() == Some("1"))
        .then(|| {
            std::env::var("TM_TEST_DATABASE_URL")
                .or_else(|_| std::env::var("TM_DATABASE_URL"))
                .ok()
        })
        .flatten()
}

fn assert_postgres_timestamp_eq(
    actual: Option<chrono::DateTime<Utc>>,
    expected: Option<chrono::DateTime<Utc>>,
) {
    assert_eq!(
        actual.map(|value| value.timestamp_micros()),
        expected.map(|value| value.timestamp_micros())
    );
}

fn durable_episodic_record(
    id: Uuid,
    owner_subject: &str,
    memory_scope: &str,
    text: &str,
    status: MemoryRecordStatus,
    links: MemoryRecordLinks,
) -> StoredMemoryRecord {
    let now = Utc::now();
    StoredMemoryRecord::new(tm_memory::MemoryRecordResource::Episodic(
        EpisodicMemoryRecord {
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
            id,
            owner_subject: owner_subject.to_string(),
            memory_scope: memory_scope.to_string(),
            text: text.to_string(),
            evidence: vec![MemoryRecordEvidence {
                schema_version: MEMORY_RECORD_SCHEMA_VERSION,
                label: "test fixture".to_string(),
                source: MemoryEvidenceSource::Resource {
                    uri: format!("memory://fixtures/{id}"),
                },
            }],
            confidence: 0.9,
            importance: 0.8,
            observed_at: now,
            effective_from: now,
            effective_to: None,
            status,
            links,
            created_at: now,
        },
    ))
    .unwrap()
}

fn pending_embedding_job(record: &StoredMemoryRecord) -> NewMemoryEmbeddingJob {
    let provenance = EmbeddingProvenance {
        schema_version: EMBEDDING_PROVENANCE_SCHEMA_VERSION,
        provider: EmbeddingProvider::Local,
        model_id: "fixture-local-v1".to_string(),
        dimensions: 768,
        normalization: EmbeddingNormalization::L2,
        content_hash: "a".repeat(64),
        embedding_version: EmbeddingProvenance::version_id(
            EmbeddingProvider::Local,
            "fixture-local-v1",
            768,
            EmbeddingNormalization::L2,
        ),
        created_at: Utc::now(),
        reembedding_state: ReembeddingState::Pending,
    };
    NewMemoryEmbeddingJob {
        id: Uuid::new_v4(),
        record_kind: record.kind(),
        record_id: record.id(),
        owner_subject: record.resource.owner_subject().to_string(),
        memory_scope: record.resource.memory_scope().to_string(),
        content_key: record.content_key.clone(),
        provenance,
        input_limit_bytes: tm_memory::DEFAULT_EMBEDDING_MAX_INPUT_BYTES,
        created_at: Utc::now(),
    }
}

fn embedding_claim(embedding_version: &str, owner_id: Uuid) -> MemoryEmbeddingJobClaim {
    MemoryEmbeddingJobClaim {
        owner_subject: "brian".to_string(),
        memory_scope: "global".to_string(),
        embedding_version: embedding_version.to_string(),
        owner_id,
        now: Utc::now(),
        lease_timeout: Duration::seconds(30),
        limit: 3,
    }
}

fn dense_query(
    generation: &tm_memory::MemoryEmbeddingGeneration,
    values: impl Into<Vec<f32>>,
) -> DenseRecallQuery {
    DenseRecallQuery {
        embedding_version: generation.embedding_version.clone(),
        snapshot_revision: generation.snapshot_revision,
        values: values.into(),
    }
}

mod in_memory_approvals;
mod in_memory_memory;
mod in_memory_scheduling;
mod in_memory_sessions;
mod postgres_approvals;
mod postgres_drive;
mod postgres_durable_memory;
mod postgres_end_to_end;
mod postgres_memory_embeddings;
mod postgres_platform;
mod postgres_scheduling;
mod postgres_sessions;
mod postgres_tm_lang;

#[derive(Debug, PartialEq, Eq)]
struct LogicalDriveSnapshot {
    path: String,
    uri: String,
    project: Option<String>,
    doc_kind: Option<String>,
    tags: Vec<String>,
    content_hash: String,
    source_uri: Option<String>,
    proposal_status: String,
}

impl LogicalDriveSnapshot {
    fn from_memory(entry: &tm_drive::DriveEntry, proposal: &tm_drive::OrganizerProposal) -> Self {
        let mut tags = entry.tags.clone();
        tags.sort();
        Self {
            path: entry.path.clone(),
            uri: entry.uri.clone(),
            project: entry.project.clone(),
            doc_kind: entry.doc_kind.clone(),
            tags,
            content_hash: entry.content_hash.clone(),
            source_uri: entry.source_uri.clone(),
            proposal_status: serde_json::to_value(&proposal.status)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
        }
    }
}

async fn insert_postgres_drive_snapshot(
    store: &PostgresStore,
    entry: &tm_drive::DriveEntry,
    proposal: &tm_drive::OrganizerProposal,
) {
    let size_bytes = entry.size_bytes as i64;
    let provenance = serde_json::to_value(&entry.provenance).unwrap();
    let entry_record = serde_json::to_value(entry).unwrap();
    let entry_version = entry.version as i64;
    store
        .client()
        .execute(
            "insert into drive_entries (id, path, uri, blob_uri, content_hash, mime, size_bytes, title, doc_kind, project, source_uri, provenance_json, summary, status, created_at, updated_at, version, record_json)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)",
            &[
                &entry.id,
                &entry.path,
                &entry.uri,
                &entry.blob_uri,
                &entry.content_hash,
                &entry.mime,
                &size_bytes,
                &entry.title,
                &entry.doc_kind,
                &entry.project,
                &entry.source_uri,
                &provenance,
                &entry.summary,
                &"active",
                &entry.created_at,
                &entry.updated_at,
                &entry_version,
                &entry_record,
            ],
        )
        .await
        .unwrap();
    for (idx, attribute) in entry.attributes.iter().enumerate() {
        let idx = idx as i32;
        let evidence = serde_json::to_value(&attribute.evidence).unwrap();
        store
            .client()
            .execute(
                "insert into drive_attributes (entry_id, idx, key, value, confidence, evidence_json, extractor, source_uri, session_id, event_seq, content_hash)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                &[
                    &entry.id,
                    &idx,
                    &attribute.key,
                    &attribute.value,
                    &attribute.confidence,
                    &evidence,
                    &attribute.extractor,
                    &attribute.source_uri,
                    &attribute.session_id,
                    &attribute.event_seq,
                    &attribute.content_hash,
                ],
            )
            .await
            .unwrap();
    }
    for tag in &entry.tags {
        store
            .client()
            .execute(
                "insert into drive_tags (entry_id, tag) values ($1, $2) on conflict do nothing",
                &[&entry.id, tag],
            )
            .await
            .unwrap();
    }
    let proposed_tags = serde_json::to_value(&proposal.proposed_tags).unwrap();
    let evidence = serde_json::to_value(&proposal.evidence).unwrap();
    let replay_metadata = serde_json::to_value(&proposal.replay_metadata).unwrap();
    let action = serde_json::to_value(&proposal.action)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    let policy_decision = serde_json::to_value(&proposal.policy_decision)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    let status = serde_json::to_value(&proposal.status)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    let proposal_record = serde_json::to_value(proposal).unwrap();
    let proposal_version = proposal.version as i64;
    store
        .client()
        .execute(
            "insert into drive_proposals (id, action, entry_id, source_path, proposed_path, proposed_tags, proposed_doc_kind, proposed_project, evidence_json, confidence, policy_decision, approval_id, status, source_run_id, replay_metadata, created_at, updated_at, version, entry_id_snapshot, record_json)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $3, $19)",
            &[
                &proposal.id,
                &action,
                &entry.id,
                &proposal.source_path,
                &proposal.proposed_path,
                &proposed_tags,
                &proposal.proposed_doc_kind,
                &proposal.proposed_project,
                &evidence,
                &proposal.confidence,
                &policy_decision,
                &proposal.approval_id,
                &status,
                &proposal.source_run_id,
                &replay_metadata,
                &proposal.created_at,
                &proposal.updated_at,
                &proposal_version,
                &proposal_record,
            ],
        )
        .await
        .unwrap();
}

async fn postgres_drive_snapshot(
    store: &PostgresStore,
    uri: &str,
    proposal_id: Uuid,
) -> LogicalDriveSnapshot {
    let row = store
        .client()
        .query_one(
            "select id, path, uri, project, doc_kind, content_hash, source_uri from drive_entries where uri = $1",
            &[&uri],
        )
        .await
        .unwrap();
    let entry_id: Uuid = row.get("id");
    let mut tags = store
        .client()
        .query(
            "select tag from drive_tags where entry_id = $1 order by tag asc",
            &[&entry_id],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<_, String>("tag"))
        .collect::<Vec<_>>();
    tags.sort();
    let status: String = store
        .client()
        .query_one(
            "select status from drive_proposals where id = $1 and entry_id = $2",
            &[&proposal_id, &entry_id],
        )
        .await
        .unwrap()
        .get("status");
    LogicalDriveSnapshot {
        path: row.get("path"),
        uri: row.get("uri"),
        project: row.get("project"),
        doc_kind: row.get("doc_kind"),
        tags,
        content_hash: row.get("content_hash"),
        source_uri: row.get("source_uri"),
        proposal_status: status,
    }
}

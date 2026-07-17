use tm_memory::{DurableMemoryReadiness, EmbeddingConfig, MemorySchemaReadiness};

use crate::{Result, ServerError};

use super::PostgresStore;

impl PostgresStore {
    /// Reports the P8.3 storage state without installing extensions or silently enabling dense
    /// retrieval. A missing pgvector installation is observable but does not make lexical-only
    /// durable memory unavailable while embeddings are disabled.
    pub async fn memory_readiness(
        &self,
        embeddings: &EmbeddingConfig,
    ) -> Result<DurableMemoryReadiness> {
        let required_relations = self
            .client
            .query_one(
                "select to_regclass('schema_migrations') is not null as migrations,
                        to_regclass('memory_records') is not null as records,
                        to_regclass('memory_record_evidence') is not null as evidence,
                        to_regclass('memory_record_relations') is not null as relations,
                        to_regclass('memory_embedding_provenance') is not null as provenance,
                        to_regclass('memory_embedding_jobs') is not null as jobs,
                        to_regclass('memory_embedding_generations') is not null as generations,
                        to_regclass('memory_embedding_active_versions') is not null as active_versions,
                        to_regclass('memory_scope_tombstones') is not null as tombstones,
                        to_regclass('memory_scope_authority_guards') is not null as authority_guards,
                        to_regclass('memory_scope_revisions') is not null as scope_revisions,
                        to_regclass('memory_legacy_migration_quarantine') is not null as quarantine",
                &[],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        let relations_present = required_relations.get::<_, bool>("migrations")
            && required_relations.get::<_, bool>("records")
            && required_relations.get::<_, bool>("evidence")
            && required_relations.get::<_, bool>("relations")
            && required_relations.get::<_, bool>("provenance")
            && required_relations.get::<_, bool>("jobs")
            && required_relations.get::<_, bool>("generations")
            && required_relations.get::<_, bool>("active_versions")
            && required_relations.get::<_, bool>("tombstones")
            && required_relations.get::<_, bool>("authority_guards")
            && required_relations.get::<_, bool>("scope_revisions")
            && required_relations.get::<_, bool>("quarantine");
        let schema = if !relations_present {
            MemorySchemaReadiness::Corrupt {
                reason: "required P8.3 memory relation is missing".to_string(),
            }
        } else {
            let migration = self
                .client
                .query_one(
                    "select exists(select 1 from schema_migrations where version = 18) as applied,
                            exists(
                                select 1
                                  from information_schema.columns
                                 where table_schema = current_schema()
                                   and table_name = 'memory_records'
                                   and column_name = 'content_key'
                            ) as record_key_column",
                    &[],
                )
                .await;
            match migration {
                Ok(row) if row.get::<_, bool>("applied") && row.get("record_key_column") => {
                    match self
                        .client
                        .batch_execute(
                            "select record_kind, id, schema_version, owner_subject, memory_scope,
                                    evidence_json, confidence, importance, observed_at,
                                    effective_from, status, content_key, version_key, created_at
                               from memory_records limit 0;
                             select record_kind, record_id, ordinal, evidence_json
                               from memory_record_evidence limit 0;
                             select record_kind, record_id, relation, linked_record_id, created_at
                               from memory_record_relations limit 0;
                             select record_kind, record_id, embedding_version, schema_version,
                                    provider, model_id, dimensions, normalization, content_hash,
                                    reembedding_state, created_at
                               from memory_embedding_provenance limit 0;
                             select id, record_kind, record_id, owner_subject, memory_scope,
                                    content_key, embedding_version, reembedding_key, status,
                                    input_limit_bytes, failure_code, attempts, available_at,
                                    lease_owner, lease_epoch, created_at, updated_at
                               from memory_embedding_jobs limit 0;
                             select owner_subject, memory_scope, embedding_version, provider,
                                    model_id, dimensions, normalization, vector_table,
                                    expected_records, completed_records, status, created_at, updated_at,
                                    generation_order, snapshot_revision, input_limit_bytes
                               from memory_embedding_generations limit 0;
                             select owner_subject, memory_scope, embedding_version, activated_at,
                                    generation_order, snapshot_revision
                               from memory_embedding_active_versions limit 0;
                             select owner_subject, memory_scope, reason, revoked_at
                               from memory_scope_tombstones limit 0;
                             select owner_subject, memory_scope, revoked_at
                               from memory_scope_authority_guards limit 0;
                             select owner_subject, memory_scope, revision, updated_at
                               from memory_scope_revisions limit 0;
                             select source_kind, source_id, reason, captured_at
                               from memory_legacy_migration_quarantine limit 0;",
                        )
                        .await
                    {
                        Ok(()) => MemorySchemaReadiness::Ready,
                        Err(error) => MemorySchemaReadiness::Corrupt {
                            reason: tm_memory::redact_dream_text(&error.to_string()).text,
                        },
                    }
                }
                Ok(_) => MemorySchemaReadiness::Corrupt {
                    reason: "P8 hardening migrations or idempotency key column are missing"
                        .to_string(),
                },
                Err(error) => MemorySchemaReadiness::Corrupt {
                    reason: tm_memory::redact_dream_text(&error.to_string()).text,
                },
            }
        };
        let pgvector_available = self
            .client
            .query_one(
                "select exists(select 1 from pg_extension where extname = 'vector') as available",
                &[],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?
            .get("available");
        Ok(DurableMemoryReadiness::from_embedding_config(
            schema,
            embeddings,
            pgvector_available,
        ))
    }
}

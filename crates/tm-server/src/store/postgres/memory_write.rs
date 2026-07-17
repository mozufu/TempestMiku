use tm_memory::{MemoryRecordResource, StoredMemoryRecord};
use tokio_postgres::GenericClient;

use crate::{Result, ServerError};

use super::rows::row_to_stored_memory_record;
use super::{DURABLE_MEMORY_RECORD_COLUMNS, PostgresStore, postgres_memory_error};

pub(super) async fn upsert_typed_memory_record<C>(
    client: &C,
    record: &StoredMemoryRecord,
) -> Result<()>
where
    C: GenericClient + Sync,
{
    let (
        schema_version,
        owner_subject,
        memory_scope,
        text,
        semantic_subject,
        predicate,
        object,
        evidence,
        confidence,
        importance,
        observed_at,
        effective_from,
        effective_to,
        status,
        links,
        created_at,
    ) = match &record.resource {
        MemoryRecordResource::Episodic(value) => (
            value.schema_version,
            value.owner_subject.as_str(),
            value.memory_scope.as_str(),
            Some(value.text.as_str()),
            None,
            None,
            None,
            &value.evidence,
            value.confidence,
            value.importance,
            value.observed_at,
            value.effective_from,
            value.effective_to,
            value.status,
            &value.links,
            value.created_at,
        ),
        MemoryRecordResource::Semantic(value) => (
            value.schema_version,
            value.owner_subject.as_str(),
            value.memory_scope.as_str(),
            None,
            Some(value.semantic_subject.as_str()),
            Some(value.predicate.as_str()),
            Some(value.object.as_str()),
            &value.evidence,
            value.confidence,
            value.importance,
            value.observed_at,
            value.effective_from,
            value.effective_to,
            value.status,
            &value.links,
            value.created_at,
        ),
    };
    let evidence_json =
        serde_json::to_value(evidence).map_err(|error| ServerError::Store(error.to_string()))?;
    client
        .execute(
            "insert into memory_records(
                record_kind, id, schema_version, owner_subject, memory_scope, text,
                semantic_subject, predicate, object, evidence_json, confidence, importance,
                observed_at, effective_from, effective_to, status,
                corrects_record_id, corrected_by_record_id,
                supersedes_record_id, superseded_by_record_id,
                content_key, version_key, created_at
             ) values (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23
             ) on conflict (record_kind, id) do update set
                schema_version = excluded.schema_version,
                owner_subject = excluded.owner_subject,
                memory_scope = excluded.memory_scope,
                text = excluded.text,
                semantic_subject = excluded.semantic_subject,
                predicate = excluded.predicate,
                object = excluded.object,
                evidence_json = excluded.evidence_json,
                confidence = excluded.confidence,
                importance = excluded.importance,
                observed_at = excluded.observed_at,
                effective_from = excluded.effective_from,
                effective_to = excluded.effective_to,
                status = excluded.status,
                corrects_record_id = excluded.corrects_record_id,
                corrected_by_record_id = excluded.corrected_by_record_id,
                supersedes_record_id = excluded.supersedes_record_id,
                superseded_by_record_id = excluded.superseded_by_record_id,
                content_key = excluded.content_key,
                version_key = excluded.version_key",
            &[
                &record.kind().as_str(),
                &record.id(),
                &i32::from(schema_version),
                &owner_subject,
                &memory_scope,
                &text,
                &semantic_subject,
                &predicate,
                &object,
                &evidence_json,
                &confidence,
                &importance,
                &observed_at,
                &effective_from,
                &effective_to,
                &status.as_str(),
                &links.corrects_record_id,
                &links.corrected_by_record_id,
                &links.supersedes_record_id,
                &links.superseded_by_record_id,
                &record.content_key,
                &record.version_key,
                &created_at,
            ],
        )
        .await
        .map_err(postgres_memory_error)?;
    Ok(())
}

impl PostgresStore {
    pub(super) async fn ensure_memory_scope_is_readable(
        &self,
        owner_subject: &str,
        memory_scope: &str,
    ) -> Result<()> {
        let tombstoned = self
            .client
            .query_opt(
                "select 1 from memory_scope_tombstones
                  where owner_subject = $1 and memory_scope = $2",
                &[&owner_subject, &memory_scope],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?
            .is_some();
        if tombstoned {
            return Err(ServerError::NotFound(format!(
                "memory scope {owner_subject}/{memory_scope}"
            )));
        }
        Ok(())
    }

    pub(super) async fn ensure_memory_record_links_are_scoped(
        &self,
        record: &StoredMemoryRecord,
    ) -> Result<()> {
        for target_id in [
            record.resource.links().corrects_record_id,
            record.resource.links().corrected_by_record_id,
            record.resource.links().supersedes_record_id,
            record.resource.links().superseded_by_record_id,
        ]
        .into_iter()
        .flatten()
        {
            let target = self
                .client
                .query_opt(
                    "select 1 from memory_records
                      where id = $1 and owner_subject = $2 and memory_scope = $3",
                    &[
                        &target_id,
                        &record.resource.owner_subject(),
                        &record.resource.memory_scope(),
                    ],
                )
                .await
                .map_err(|error| ServerError::Store(error.to_string()))?;
            if target.is_none() {
                return Err(ServerError::NotFound(format!(
                    "memory record {target_id} in requested authority"
                )));
            }
        }
        Ok(())
    }

    pub(super) async fn memory_record_with_content_key(
        &self,
        record: &StoredMemoryRecord,
    ) -> Result<Option<StoredMemoryRecord>> {
        let row = self
            .client
            .query_opt(
                &format!(
                    "select {DURABLE_MEMORY_RECORD_COLUMNS}
                       from memory_records
                      where record_kind = $1
                        and owner_subject = $2
                        and memory_scope = $3
                        and content_key = $4
                        and status = 'active' and effective_to is null"
                ),
                &[
                    &record.kind().as_str(),
                    &record.resource.owner_subject(),
                    &record.resource.memory_scope(),
                    &record.content_key,
                ],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        row.map(row_to_stored_memory_record).transpose()
    }
}

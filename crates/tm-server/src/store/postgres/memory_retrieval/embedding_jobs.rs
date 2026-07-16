use chrono::{DateTime, Utc};
use tm_memory::{
    MemoryEmbeddingGeneration, MemoryEmbeddingGenerationStatus, MemoryEmbeddingJobClaim,
    MemoryEmbeddingJobLease, embedding_text,
};

use crate::{Result, ServerError};

use super::super::{
    MEMORY_EMBEDDING_JOB_COLUMNS, PostgresStore, rows::row_to_memory_embedding_job,
};
use crate::store::Store;

use super::generation::{quote_identifier, store_error, validate_embedding_values, vector_literal};

impl PostgresStore {
    /// Claims a bounded re-embedding batch, reclaiming only expired leases. A restarted worker can
    /// therefore resume without creating duplicate vectors or switching the active version early.
    pub async fn claim_memory_embedding_jobs(
        &self,
        claim: &MemoryEmbeddingJobClaim,
    ) -> Result<Vec<MemoryEmbeddingJobLease>> {
        self.ensure_memory_scope_is_readable(&claim.owner_subject, &claim.memory_scope)
            .await?;
        if claim.limit == 0 || claim.limit > tm_memory::MAX_EMBEDDING_BATCH_SIZE {
            return Err(ServerError::InvalidRequest(format!(
                "embedding claim limit must be between 1 and {}",
                tm_memory::MAX_EMBEDDING_BATCH_SIZE
            )));
        }
        let stale_before = claim.now - claim.lease_timeout;
        let limit = i64::try_from(claim.limit).map_err(|_| {
            ServerError::InvalidRequest("embedding claim limit is too large".to_string())
        })?;
        let rows = self
            .client
            .query(
                &format!(
                    "with candidates as (
                        select job.id
                          from memory_embedding_jobs job
                          join memory_embedding_generations generation
                            on generation.owner_subject = job.owner_subject
                           and generation.memory_scope = job.memory_scope
                           and generation.embedding_version = job.embedding_version
                         where job.owner_subject = $1
                           and job.memory_scope = $2
                           and job.embedding_version = $3
                           and generation.status = 'staging'
                           and (
                               (job.status = 'queued' and job.available_at <= $4)
                               or (job.status = 'running' and job.locked_at <= $5)
                           )
                         order by job.available_at asc, job.created_at asc, job.id asc
                         limit $6 for update skip locked
                    ), claimed as (
                        update memory_embedding_jobs job
                           set status = 'running', locked_at = $4, lease_owner = $7,
                               lease_epoch = job.lease_epoch + 1, attempts = job.attempts + 1,
                               updated_at = $4
                          from candidates
                         where job.id = candidates.id
                        returning job.*
                    )
                    select {MEMORY_EMBEDDING_JOB_COLUMNS}
                      from claimed j
                      join memory_embedding_provenance p
                        on p.record_kind = j.record_kind
                       and p.record_id = j.record_id
                       and p.embedding_version = j.embedding_version
                     order by j.created_at asc, j.id asc"
                ),
                &[
                    &claim.owner_subject,
                    &claim.memory_scope,
                    &claim.embedding_version,
                    &claim.now,
                    &stale_before,
                    &limit,
                    &claim.owner_id,
                ],
            )
            .await
            .map_err(store_error)?;
        rows.into_iter()
            .map(|row| {
                let job = row_to_memory_embedding_job(row)?;
                Ok(MemoryEmbeddingJobLease {
                    owner_id: claim.owner_id,
                    epoch: job.lease_epoch,
                    job,
                })
            })
            .collect()
    }

    /// Stores one validated vector and atomically promotes its generation only once all queued
    /// snapshot records are complete. The active pointer is the only read path for dense recall.
    pub async fn complete_memory_embedding_job(
        &self,
        lease: &MemoryEmbeddingJobLease,
        values: &[f32],
        completed_at: DateTime<Utc>,
    ) -> Result<MemoryEmbeddingGeneration> {
        validate_embedding_values(
            values,
            lease.job.provenance.dimensions,
            lease.job.provenance.normalization,
        )?;
        let generation = self
            .memory_embedding_generation(
                &lease.job.owner_subject,
                &lease.job.memory_scope,
                &lease.job.provenance.embedding_version,
            )
            .await?;
        if generation.status != MemoryEmbeddingGenerationStatus::Staging {
            return Err(ServerError::Conflict(
                "embedding generation is no longer staging".to_string(),
            ));
        }
        let record = self
            .memory_record(
                &lease.job.owner_subject,
                &lease.job.memory_scope,
                lease.job.record_kind,
                lease.job.record_id,
            )
            .await?;
        let content = embedding_text(&record);
        if record.content_key != lease.job.content_key
            || tm_memory::embedding_content_hash(&content) != lease.job.provenance.content_hash
        {
            self.fail_memory_embedding_job(
                lease,
                "content_changed",
                "memory content changed during re-embedding",
                completed_at,
            )
            .await?;
            self.reconcile_memory_embedding_generation(&generation, completed_at)
                .await?;
            return Err(ServerError::Conflict(
                "memory content changed during re-embedding; retained lexical-only recall"
                    .to_string(),
            ));
        }
        let vector_schema = self.pgvector_extension_schema().await?.ok_or_else(|| {
            ServerError::Policy("pgvector is unavailable; retained lexical-only recall".to_string())
        })?;
        let literal = vector_literal(values)?;
        let updated = self
            .client
            .execute(
                &format!(
                    "with completed_job as (
                        update memory_embedding_jobs
                           set status = 'completed', locked_at = null, lease_owner = null,
                               failure_code = null, updated_at = $9
                         where id = $10 and status = 'running'
                           and lease_owner = $11 and lease_epoch = $12
                        returning id
                     )
                     insert into {}(
                        record_kind, record_id, owner_subject, memory_scope, content_hash,
                        content_key, embedding_version, embedding, created_at
                     ) select $1, $2, $3, $4, $5, $6, $7, $8::text::{}.vector, $9
                         from completed_job
                     on conflict (record_kind, record_id) do update set
                        owner_subject = excluded.owner_subject,
                        memory_scope = excluded.memory_scope,
                        content_hash = excluded.content_hash,
                        content_key = excluded.content_key,
                        embedding_version = excluded.embedding_version,
                        embedding = excluded.embedding,
                        created_at = excluded.created_at",
                    generation.vector_table,
                    quote_identifier(&vector_schema),
                ),
                &[
                    &lease.job.record_kind.as_str(),
                    &lease.job.record_id,
                    &lease.job.owner_subject,
                    &lease.job.memory_scope,
                    &lease.job.provenance.content_hash,
                    &lease.job.content_key,
                    &lease.job.provenance.embedding_version,
                    &literal,
                    &completed_at,
                    &lease.job.id,
                    &lease.owner_id,
                    &lease.epoch,
                ],
            )
            .await
            .map_err(store_error)?;
        if updated != 1 {
            return Err(ServerError::NotFound(format!(
                "active embedding lease {} owner {} epoch {}",
                lease.job.id, lease.owner_id, lease.epoch
            )));
        }
        self.client
            .execute(
                "update memory_embedding_provenance
                    set reembedding_state = 'ready'
                  where record_kind = $1 and record_id = $2 and embedding_version = $3",
                &[
                    &lease.job.record_kind.as_str(),
                    &lease.job.record_id,
                    &lease.job.provenance.embedding_version,
                ],
            )
            .await
            .map_err(store_error)?;
        self.try_activate_memory_embedding_generation(&generation, completed_at)
            .await
    }

    /// Releases a failed provider batch back to the queue. No active pointer changes, so callers
    /// visibly remain on lexical-only or the previous fully-covered dense generation.
    pub async fn retry_memory_embedding_job(
        &self,
        lease: &MemoryEmbeddingJobLease,
        available_at: DateTime<Utc>,
    ) -> Result<()> {
        let updated = self
            .client
            .execute(
                "update memory_embedding_jobs
                    set status = 'queued', available_at = $4, locked_at = null,
                        lease_owner = null, failure_code = null, updated_at = $4
                  where id = $1 and status = 'running' and lease_owner = $2 and lease_epoch = $3",
                &[&lease.job.id, &lease.owner_id, &lease.epoch, &available_at],
            )
            .await
            .map_err(store_error)?;
        if updated != 1 {
            return Err(ServerError::NotFound(format!(
                "active embedding lease {} owner {} epoch {}",
                lease.job.id, lease.owner_id, lease.epoch
            )));
        }
        Ok(())
    }

    pub async fn fail_memory_embedding_job(
        &self,
        lease: &MemoryEmbeddingJobLease,
        failure_code: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let updated = self
            .client
            .execute(
                "update memory_embedding_jobs
                    set status = 'failed', failure_code = $4, locked_at = null,
                        lease_owner = null, updated_at = $5
                  where id = $1 and status = 'running' and lease_owner = $2 and lease_epoch = $3",
                &[
                    &lease.job.id,
                    &lease.owner_id,
                    &lease.epoch,
                    &failure_code,
                    &now,
                ],
            )
            .await
            .map_err(store_error)?;
        if updated != 1 {
            return Err(ServerError::NotFound(format!(
                "active embedding lease {} owner {} epoch {}",
                lease.job.id, lease.owner_id, lease.epoch
            )));
        }
        self.client
            .execute(
                "update memory_embedding_provenance
                    set reembedding_state = 'failed'
                  where record_kind = $1 and record_id = $2 and embedding_version = $3",
                &[
                    &lease.job.record_kind.as_str(),
                    &lease.job.record_id,
                    &lease.job.provenance.embedding_version,
                ],
            )
            .await
            .map_err(store_error)?;
        tracing::warn!(job_id = %lease.job.id, %reason, "failed stale memory embedding job");
        Ok(())
    }
}

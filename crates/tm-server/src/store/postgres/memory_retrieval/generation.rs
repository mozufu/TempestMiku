use chrono::{DateTime, Utc};
use tm_memory::{
    EmbeddingConfig, EmbeddingNormalization, EmbeddingProvenance, EmbeddingProvider,
    MemoryEmbeddingGeneration, MemoryEmbeddingGenerationStatus, NewMemoryEmbeddingGeneration,
    NewMemoryEmbeddingJob, embedding_text,
};
use uuid::Uuid;

use crate::{Result, ServerError};

use super::super::{ACTIVE_MEMORY_SUCCESSOR_QUERY, PostgresStore};
use crate::store::Store;

impl PostgresStore {
    pub async fn active_memory_scopes(&self, owner_subject: &str) -> Result<Vec<String>> {
        let rows = self
            .client
            .query(
                "select memory_scope from (
                    select 'global'::text as memory_scope
                    union
                    select distinct record.memory_scope
                      from memory_records record
                     where record.owner_subject = $1
                       and record.status = 'active' and record.effective_to is null
                       and not exists (
                           select 1 from memory_scope_tombstones tombstone
                            where tombstone.owner_subject = record.owner_subject
                              and tombstone.memory_scope = record.memory_scope
                       )
                 ) scopes
                 order by memory_scope asc",
                &[&owner_subject],
            )
            .await
            .map_err(store_error)?;
        Ok(rows
            .into_iter()
            .map(|row| row.get("memory_scope"))
            .collect())
    }

    /// Stages a versioned embedding generation but leaves the current dense generation untouched.
    /// The active pointer only moves once the exact snapshot reaches complete vector coverage.
    pub async fn stage_memory_embedding_generation(
        &self,
        generation: NewMemoryEmbeddingGeneration,
    ) -> Result<MemoryEmbeddingGeneration> {
        generation
            .validate()
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        self.ensure_memory_scope_is_readable(&generation.owner_subject, &generation.memory_scope)
            .await?;
        let snapshot_revision = self
            .memory_scope_revision(&generation.owner_subject, &generation.memory_scope)
            .await?;
        let existing = self
            .memory_embedding_generation_optional(
                &generation.owner_subject,
                &generation.memory_scope,
                &generation.embedding_version,
            )
            .await?;
        if let Some(existing) = &existing
            && existing.snapshot_revision == snapshot_revision
            && existing.input_limit_bytes >= generation.input_limit_bytes
        {
            if existing.status == MemoryEmbeddingGenerationStatus::Failed {
                return Ok(existing.clone());
            }
            if existing.status == MemoryEmbeddingGenerationStatus::Ready
                && self
                    .generation_vector_table_exists(&existing.vector_table)
                    .await?
            {
                return Ok(existing.clone());
            }
        }
        let vector_schema = self.pgvector_extension_schema().await?.ok_or_else(|| {
            ServerError::Policy(
                "pgvector is not installed; dense memory remains lexical-only".to_string(),
            )
        })?;
        let vector_table = vector_table_name(&generation.embedding_version);
        self.ensure_generation_vector_table(&vector_schema, &vector_table, generation.dimensions)
            .await?;

        let records = self
            .active_memory_records(
                &generation.owner_subject,
                &generation.memory_scope,
                i64::MAX as usize,
            )
            .await?;
        let expected_records = i32::try_from(records.len()).map_err(|_| {
            ServerError::InvalidRequest(
                "too many memory records for an embedding generation".to_string(),
            )
        })?;
        let dimensions = i32::try_from(generation.dimensions).map_err(|_| {
            ServerError::InvalidRequest(
                "embedding dimensions exceed Postgres integer range".to_string(),
            )
        })?;
        let staged = if existing.as_ref().is_some_and(|existing| {
            existing.snapshot_revision == snapshot_revision
                && existing.status == MemoryEmbeddingGenerationStatus::Staging
                && existing.input_limit_bytes >= generation.input_limit_bytes
        }) {
            existing.expect("checked existing staging generation")
        } else if existing.is_none() {
            let row = self
                .client
                .query_one(
                    "insert into memory_embedding_generations(
                    owner_subject, memory_scope, embedding_version, schema_version, provider,
                    model_id, dimensions, normalization, vector_table, expected_records,
                    completed_records, status, created_at, updated_at, snapshot_revision,
                    input_limit_bytes
                 ) values ($1, $2, $3, 1, $4, $5, $6, $7, $8, $9, 0, 'staging', $10, $10,
                    $11, $12)
                 returning owner_subject, memory_scope, embedding_version, provider, model_id,
                           dimensions, normalization, vector_table, expected_records,
                           completed_records, status, created_at, updated_at, generation_order,
                           snapshot_revision, input_limit_bytes",
                    &[
                        &generation.owner_subject,
                        &generation.memory_scope,
                        &generation.embedding_version,
                        &generation.provider.as_str(),
                        &generation.model_id,
                        &dimensions,
                        &generation.normalization.as_str(),
                        &vector_table,
                        &expected_records,
                        &generation.created_at,
                        &snapshot_revision,
                        &i32::try_from(generation.input_limit_bytes).map_err(|_| {
                            ServerError::InvalidRequest(
                                "embedding input limit exceeds Postgres range".to_string(),
                            )
                        })?,
                    ],
                )
                .await
                .map_err(store_error)?;
            row_to_memory_embedding_generation(row)?
        } else {
            let row = self
                .client
                .query_opt(
                    "update memory_embedding_generations
                        set expected_records = $4, completed_records = 0, status = 'staging',
                            updated_at = $5, snapshot_revision = $6,
                            input_limit_bytes = greatest(input_limit_bytes, $7)
                      where owner_subject = $1 and memory_scope = $2 and embedding_version = $3
                        and snapshot_revision <= $6
                     returning owner_subject, memory_scope, embedding_version, provider, model_id,
                               dimensions, normalization, vector_table, expected_records,
                               completed_records, status, created_at, updated_at, generation_order,
                               snapshot_revision, input_limit_bytes",
                    &[
                        &generation.owner_subject,
                        &generation.memory_scope,
                        &generation.embedding_version,
                        &expected_records,
                        &generation.created_at,
                        &snapshot_revision,
                        &i32::try_from(generation.input_limit_bytes).map_err(|_| {
                            ServerError::InvalidRequest(
                                "embedding input limit exceeds Postgres range".to_string(),
                            )
                        })?,
                    ],
                )
                .await
                .map_err(store_error)?;
            match row {
                Some(row) => row_to_memory_embedding_generation(row)?,
                None => {
                    self.memory_embedding_generation(
                        &generation.owner_subject,
                        &generation.memory_scope,
                        &generation.embedding_version,
                    )
                    .await?
                }
            }
        };

        let config = EmbeddingConfig {
            provider: generation.provider,
            dimensions: Some(generation.dimensions),
            model: Some(generation.model_id.clone()),
            normalization: generation.normalization,
            max_input_bytes: generation.input_limit_bytes,
            ..EmbeddingConfig::default()
        };
        for record in records {
            if self
                .memory_record_has_current_vector(&staged, &record)
                .await?
            {
                continue;
            }
            let content = embedding_text(&record);
            let provenance =
                EmbeddingProvenance::from_config(&config, &content, generation.created_at)
                    .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
            self.enqueue_memory_embedding_job(NewMemoryEmbeddingJob {
                id: Uuid::new_v4(),
                record_kind: record.kind(),
                record_id: record.id(),
                owner_subject: generation.owner_subject.clone(),
                memory_scope: generation.memory_scope.clone(),
                content_key: record.content_key,
                provenance,
                input_limit_bytes: generation.input_limit_bytes,
                created_at: generation.created_at,
            })
            .await?;
        }
        self.try_activate_memory_embedding_generation(&staged, generation.created_at)
            .await
    }

    pub async fn memory_embedding_generation(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        embedding_version: &str,
    ) -> Result<MemoryEmbeddingGeneration> {
        self.ensure_memory_scope_is_readable(owner_subject, memory_scope)
            .await?;
        self.memory_embedding_generation_optional(owner_subject, memory_scope, embedding_version)
            .await?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "memory embedding generation {owner_subject}/{memory_scope}/{embedding_version}"
                ))
            })
    }

    pub async fn active_memory_embedding_generation(
        &self,
        owner_subject: &str,
        memory_scope: &str,
    ) -> Result<Option<MemoryEmbeddingGeneration>> {
        self.ensure_memory_scope_is_readable(owner_subject, memory_scope)
            .await?;
        let row = self
            .client
            .query_opt(
                "select generation.owner_subject, generation.memory_scope,
                        generation.embedding_version, generation.provider, generation.model_id,
                        generation.dimensions, generation.normalization, generation.vector_table,
                        generation.expected_records, generation.completed_records, generation.status,
                        generation.created_at, generation.updated_at, generation.generation_order,
                        generation.snapshot_revision, generation.input_limit_bytes
                   from memory_embedding_active_versions active
                   join memory_embedding_generations generation
                     on generation.owner_subject = active.owner_subject
                    and generation.memory_scope = active.memory_scope
                    and generation.embedding_version = active.embedding_version
                   join memory_scope_revisions revision
                     on revision.owner_subject = active.owner_subject
                    and revision.memory_scope = active.memory_scope
                  where active.owner_subject = $1 and active.memory_scope = $2
                    and active.generation_order = generation.generation_order
                    and active.snapshot_revision = generation.snapshot_revision
                    and active.snapshot_revision = revision.revision
                    and generation.status = 'ready'",
                &[&owner_subject, &memory_scope],
            )
            .await
            .map_err(store_error)?;
        row.map(row_to_memory_embedding_generation).transpose()
    }

    async fn memory_scope_revision(&self, owner_subject: &str, memory_scope: &str) -> Result<i64> {
        if let Some(row) = self
            .client
            .query_opt(
                "select revision from memory_scope_revisions
                  where owner_subject = $1 and memory_scope = $2",
                &[&owner_subject, &memory_scope],
            )
            .await
            .map_err(store_error)?
        {
            return Ok(row.get("revision"));
        }
        let row = self
            .client
            .query_one(
                "insert into memory_scope_revisions(owner_subject, memory_scope, revision)
                 values ($1, $2, 0)
                 on conflict (owner_subject, memory_scope) do update
                    set revision = memory_scope_revisions.revision
                 returning revision",
                &[&owner_subject, &memory_scope],
            )
            .await
            .map_err(store_error)?;
        Ok(row.get("revision"))
    }

    async fn generation_vector_table_exists(&self, vector_table: &str) -> Result<bool> {
        self.client
            .query_one(
                "select to_regclass($1) is not null as present",
                &[&vector_table],
            )
            .await
            .map_err(store_error)
            .map(|row| row.get("present"))
    }

    async fn memory_embedding_generation_optional(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        embedding_version: &str,
    ) -> Result<Option<MemoryEmbeddingGeneration>> {
        let row = self
            .client
            .query_opt(
                "select owner_subject, memory_scope, embedding_version, provider, model_id,
                        dimensions, normalization, vector_table, expected_records,
                        completed_records, status, created_at, updated_at, generation_order,
                        snapshot_revision, input_limit_bytes
                   from memory_embedding_generations
                  where owner_subject = $1 and memory_scope = $2 and embedding_version = $3",
                &[&owner_subject, &memory_scope, &embedding_version],
            )
            .await
            .map_err(store_error)?;
        row.map(row_to_memory_embedding_generation).transpose()
    }

    pub(super) async fn try_activate_memory_embedding_generation(
        &self,
        generation: &MemoryEmbeddingGeneration,
        now: DateTime<Utc>,
    ) -> Result<MemoryEmbeddingGeneration> {
        self.pgvector_extension_schema().await?.ok_or_else(|| {
            ServerError::Policy("pgvector is unavailable; retained lexical-only recall".to_string())
        })?;
        self.client
            .execute(
                &format!(
                    "with coverage as (
                    select count(*)::integer as completed_records
                      from {} vector
                      join memory_records record
                        on record.record_kind = vector.record_kind
                       and record.id = vector.record_id
                       and record.content_key = vector.content_key
                     where vector.owner_subject = $1 and vector.memory_scope = $2
                       and vector.embedding_version = $3
                       and record.owner_subject = $1 and record.memory_scope = $2
                       and record.status = 'active' and record.effective_to is null
                       and not exists ({ACTIVE_MEMORY_SUCCESSOR_QUERY})
                 ), eligible as (
                    update memory_embedding_generations generation
                       set completed_records = coverage.completed_records,
                           status = 'ready', updated_at = $4
                      from coverage
                     where generation.owner_subject = $1 and generation.memory_scope = $2
                       and generation.embedding_version = $3 and generation.status = 'staging'
                       and generation.snapshot_revision = $5
                       and exists (
                           select 1 from memory_scope_revisions revision
                            where revision.owner_subject = $1 and revision.memory_scope = $2
                              and revision.revision = generation.snapshot_revision
                       )
                       and generation.expected_records = coverage.completed_records
                    returning generation.owner_subject, generation.memory_scope,
                              generation.embedding_version, generation.generation_order,
                              generation.snapshot_revision
                 ), active as (
                    insert into memory_embedding_active_versions(
                        owner_subject, memory_scope, embedding_version, activated_at,
                        generation_order, snapshot_revision
                    )
                    select owner_subject, memory_scope, embedding_version, $4,
                           generation_order, snapshot_revision from eligible
                    on conflict (owner_subject, memory_scope) do update set
                        embedding_version = excluded.embedding_version,
                        activated_at = excluded.activated_at,
                        generation_order = excluded.generation_order,
                        snapshot_revision = excluded.snapshot_revision
                      where memory_embedding_active_versions.generation_order
                                < excluded.generation_order
                         or (memory_embedding_active_versions.generation_order
                                = excluded.generation_order
                             and memory_embedding_active_versions.snapshot_revision
                                <= excluded.snapshot_revision)
                    returning owner_subject, memory_scope, embedding_version
                 )
                 select 1 from active",
                    generation.vector_table,
                ),
                &[
                    &generation.owner_subject,
                    &generation.memory_scope,
                    &generation.embedding_version,
                    &now,
                    &generation.snapshot_revision,
                ],
            )
            .await
            .map_err(store_error)?;
        self.memory_embedding_generation(
            &generation.owner_subject,
            &generation.memory_scope,
            &generation.embedding_version,
        )
        .await
    }

    async fn memory_record_has_current_vector(
        &self,
        generation: &MemoryEmbeddingGeneration,
        record: &tm_memory::StoredMemoryRecord,
    ) -> Result<bool> {
        let row = self
            .client
            .query_opt(
                &format!(
                    "select 1 from {} vector
                      where vector.record_kind = $1 and vector.record_id = $2
                        and vector.owner_subject = $3 and vector.memory_scope = $4
                        and vector.embedding_version = $5 and vector.content_key = $6",
                    generation.vector_table
                ),
                &[
                    &record.kind().as_str(),
                    &record.id(),
                    &generation.owner_subject,
                    &generation.memory_scope,
                    &generation.embedding_version,
                    &record.content_key,
                ],
            )
            .await
            .map_err(store_error)?;
        Ok(row.is_some())
    }

    pub async fn reconcile_memory_embedding_generation(
        &self,
        generation: &MemoryEmbeddingGeneration,
        now: DateTime<Utc>,
    ) -> Result<MemoryEmbeddingGeneration> {
        let generation = self
            .try_activate_memory_embedding_generation(generation, now)
            .await?;
        if generation.status != MemoryEmbeddingGenerationStatus::Staging {
            return Ok(generation);
        }
        self.client
            .execute(
                "update memory_embedding_generations generation
                    set status = 'failed', updated_at = $4
                  where generation.owner_subject = $1 and generation.memory_scope = $2
                    and generation.embedding_version = $3 and generation.status = 'staging'
                    and generation.snapshot_revision = $5
                    and exists (
                        select 1 from memory_embedding_jobs job
                         where job.owner_subject = $1 and job.memory_scope = $2
                           and job.embedding_version = $3 and job.status = 'failed'
                    )
                    and not exists (
                        select 1 from memory_embedding_jobs job
                         where job.owner_subject = $1 and job.memory_scope = $2
                           and job.embedding_version = $3
                           and job.status in ('queued', 'running')
                    )",
                &[
                    &generation.owner_subject,
                    &generation.memory_scope,
                    &generation.embedding_version,
                    &now,
                    &generation.snapshot_revision,
                ],
            )
            .await
            .map_err(store_error)?;
        self.memory_embedding_generation(
            &generation.owner_subject,
            &generation.memory_scope,
            &generation.embedding_version,
        )
        .await
    }

    pub(super) async fn pgvector_extension_schema(&self) -> Result<Option<String>> {
        self.client
            .query_opt(
                "select namespace.nspname
                   from pg_extension extension
                   join pg_namespace namespace on namespace.oid = extension.extnamespace
                  where extension.extname = 'vector'",
                &[],
            )
            .await
            .map_err(store_error)
            .map(|row| row.map(|row| row.get(0)))
    }

    async fn ensure_generation_vector_table(
        &self,
        vector_schema: &str,
        vector_table: &str,
        dimensions: usize,
    ) -> Result<()> {
        if vector_table != vector_table_name_from_table(vector_table) {
            return Err(ServerError::InvalidRequest(
                "unsafe generated pgvector table name".to_string(),
            ));
        }
        let index_name = format!("{vector_table}_hnsw");
        let vector_type = quote_identifier(vector_schema);
        self.client
            .batch_execute(&format!(
                "create table if not exists {vector_table}(
                    record_kind text not null check (record_kind in ('episodic', 'semantic')),
                    record_id uuid not null,
                    owner_subject text not null,
                    memory_scope text not null,
                    content_hash text not null check (content_hash ~ '^[[:xdigit:]]{{64}}$'),
                    content_key text not null,
                    embedding_version text not null,
                    embedding {vector_type}.vector({dimensions}) not null,
                    created_at timestamptz not null,
                    primary key(record_kind, record_id)
                 );
                 create index if not exists {index_name}
                    on {vector_table} using hnsw (embedding {vector_type}.vector_cosine_ops);
                 create index if not exists {vector_table}_authority_idx
                    on {vector_table}(owner_subject, memory_scope);"
            ))
            .await
            .map_err(store_error)
    }
}

fn row_to_memory_embedding_generation(
    row: tokio_postgres::Row,
) -> Result<MemoryEmbeddingGeneration> {
    let provider: String = row.get("provider");
    let provider = EmbeddingProvider::parse(&provider)
        .ok_or_else(|| ServerError::Store(format!("unknown embedding provider {provider}")))?;
    let normalization: String = row.get("normalization");
    let normalization = EmbeddingNormalization::parse(&normalization).ok_or_else(|| {
        ServerError::Store(format!("unknown embedding normalization {normalization}"))
    })?;
    let status: String = row.get("status");
    let status = MemoryEmbeddingGenerationStatus::parse(&status).ok_or_else(|| {
        ServerError::Store(format!("unknown embedding generation status {status}"))
    })?;
    let dimensions: i32 = row.get("dimensions");
    let expected_records: i32 = row.get("expected_records");
    let completed_records: i32 = row.get("completed_records");
    let input_limit_bytes: i32 = row.get("input_limit_bytes");
    Ok(MemoryEmbeddingGeneration {
        owner_subject: row.get("owner_subject"),
        memory_scope: row.get("memory_scope"),
        embedding_version: row.get("embedding_version"),
        provider,
        model_id: row.get("model_id"),
        dimensions: usize::try_from(dimensions).map_err(|_| {
            ServerError::Store("invalid embedding generation dimensions".to_string())
        })?,
        normalization,
        generation_order: row.get("generation_order"),
        snapshot_revision: row.get("snapshot_revision"),
        input_limit_bytes: usize::try_from(input_limit_bytes).map_err(|_| {
            ServerError::Store("invalid embedding generation input limit".to_string())
        })?,
        vector_table: row.get("vector_table"),
        expected_records: usize::try_from(expected_records)
            .map_err(|_| ServerError::Store("invalid embedding expected count".to_string()))?,
        completed_records: usize::try_from(completed_records)
            .map_err(|_| ServerError::Store("invalid embedding completed count".to_string()))?,
        status,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub(super) fn validate_embedding_values(
    values: &[f32],
    dimensions: usize,
    normalization: EmbeddingNormalization,
) -> Result<()> {
    if values.len() != dimensions {
        return Err(ServerError::InvalidRequest(format!(
            "embedding vector has {} dimensions, expected {dimensions}",
            values.len()
        )));
    }
    if values.iter().any(|value| !value.is_finite()) {
        return Err(ServerError::InvalidRequest(
            "embedding vector contains a non-finite value".to_string(),
        ));
    }
    if normalization == EmbeddingNormalization::L2 {
        let magnitude = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        if (magnitude - 1.0).abs() > 0.001 {
            return Err(ServerError::InvalidRequest(
                "embedding vector is not L2 normalized".to_string(),
            ));
        }
    }
    Ok(())
}

pub(super) fn vector_literal(values: &[f32]) -> Result<String> {
    if values.iter().any(|value| !value.is_finite()) {
        return Err(ServerError::InvalidRequest(
            "embedding vector contains a non-finite value".to_string(),
        ));
    }
    Ok(format!(
        "[{}]",
        values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",")
    ))
}

fn vector_table_name(embedding_version: &str) -> String {
    let digest = tm_memory::embedding_content_hash(embedding_version);
    format!("tm_memory_vec_{}", &digest[..32])
}

pub(super) fn vector_table_name_from_table(value: &str) -> String {
    if value.len() == "tm_memory_vec_".len() + 32
        && value.starts_with("tm_memory_vec_")
        && value["tm_memory_vec_".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        value.to_string()
    } else {
        String::new()
    }
}

pub(super) fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub(super) fn is_missing_pgvector_relation(error: &tokio_postgres::Error) -> bool {
    error.code().is_some_and(|code| {
        *code == tokio_postgres::error::SqlState::UNDEFINED_TABLE
            || *code == tokio_postgres::error::SqlState::UNDEFINED_OBJECT
    })
}

pub(super) fn store_error(error: tokio_postgres::Error) -> ServerError {
    super::super::postgres_memory_error(error)
}

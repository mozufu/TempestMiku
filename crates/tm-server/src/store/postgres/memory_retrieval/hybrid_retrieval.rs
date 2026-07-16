use tm_memory::{
    DenseRecallQuery, DenseRecallStatus, HybridRecallRequest, HybridRecallResult,
    RankedMemoryCandidate, fuse_hybrid_candidates,
};

use crate::{Result, ServerError};

use super::super::{
    ACTIVE_MEMORY_SUCCESSOR_QUERY, PostgresStore, rows::row_to_stored_memory_record,
};

use super::generation::{
    is_missing_pgvector_relation, quote_identifier, store_error, validate_embedding_values,
    vector_literal,
};

const QUALIFIED_DURABLE_MEMORY_RECORD_COLUMNS: &str = "record.record_kind, record.id, record.schema_version, record.owner_subject, record.memory_scope, record.text, record.semantic_subject, record.predicate, record.object, record.evidence_json, record.confidence, record.importance, record.observed_at, record.effective_from, record.effective_to, record.status, record.corrects_record_id, record.corrected_by_record_id, record.supersedes_record_id, record.superseded_by_record_id, record.content_key, record.version_key, record.created_at";

impl PostgresStore {
    /// Runs the durable FTS candidate query. This is intentionally separate from the frozen P8.1
    /// lexical control-group query and is not wired into turn prompting until P8.4.
    pub async fn memory_lexical_candidates(
        &self,
        request: &HybridRecallRequest,
        query: &str,
    ) -> Result<Vec<RankedMemoryCandidate>> {
        request
            .validate()
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        self.ensure_memory_scope_is_readable(&request.owner_subject, &request.memory_scope)
            .await?;
        let limit = i64::try_from(request.candidate_limit).map_err(|_| {
            ServerError::InvalidRequest("hybrid candidate limit is too large".to_string())
        })?;
        let escaped_query = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped_query}%");
        let rows = self
            .client
            .query(
                &format!(
                    "select {QUALIFIED_DURABLE_MEMORY_RECORD_COLUMNS},
                            ts_rank_cd(
                                to_tsvector('simple', coalesce(record.text, '') || ' ' ||
                                    coalesce(record.semantic_subject, '') || ' ' ||
                                    coalesce(record.predicate, '') || ' ' ||
                                    coalesce(record.object, '')),
                                plainto_tsquery('simple', $3)
                            ) as recall_score
                       from memory_records record
                      where record.owner_subject = $1 and record.memory_scope = $2
                        and record.status = 'active' and record.effective_to is null
                        and (
                            to_tsvector('simple', coalesce(record.text, '') || ' ' ||
                                coalesce(record.semantic_subject, '') || ' ' ||
                                coalesce(record.predicate, '') || ' ' ||
                                coalesce(record.object, '')) @@ plainto_tsquery('simple', $3)
                            or concat_ws(' ', record.text, record.semantic_subject,
                                record.predicate, record.object) ilike $4 escape '\\'
                        )
                        and not exists ({ACTIVE_MEMORY_SUCCESSOR_QUERY})
                      order by recall_score desc, record.importance desc, record.observed_at desc,
                               record.record_kind asc, record.id asc
                      limit $5"
                ),
                &[
                    &request.owner_subject,
                    &request.memory_scope,
                    &query,
                    &pattern,
                    &limit,
                ],
            )
            .await
            .map_err(store_error)?;
        rows.into_iter()
            .enumerate()
            .map(|(index, row)| {
                Ok(RankedMemoryCandidate {
                    record: row_to_stored_memory_record(row.clone())?,
                    rank: u32::try_from(index + 1).expect("bounded hybrid rank fits u32"),
                    score: row.get("recall_score"),
                    embedding_version: None,
                })
            })
            .collect()
    }

    async fn memory_dense_candidates_with_status(
        &self,
        request: &HybridRecallRequest,
        dense_query: &DenseRecallQuery,
    ) -> Result<(Vec<RankedMemoryCandidate>, DenseRecallStatus)> {
        request
            .validate()
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        let Some(generation) = self
            .active_memory_embedding_generation(&request.owner_subject, &request.memory_scope)
            .await?
        else {
            return Ok((Vec::new(), DenseRecallStatus::GenerationChanged));
        };
        if generation.embedding_version != dense_query.embedding_version
            || generation.snapshot_revision != dense_query.snapshot_revision
        {
            return Ok((Vec::new(), DenseRecallStatus::GenerationChanged));
        }
        if self.pgvector_extension_schema().await?.is_none() {
            return Ok((Vec::new(), DenseRecallStatus::Unavailable));
        }
        validate_embedding_values(
            &dense_query.values,
            generation.dimensions,
            generation.normalization,
        )?;
        let vector_schema = self
            .pgvector_extension_schema()
            .await?
            .expect("checked pgvector extension presence");
        let literal = vector_literal(&dense_query.values)?;
        let limit = i64::try_from(request.candidate_limit).map_err(|_| {
            ServerError::InvalidRequest("hybrid candidate limit is too large".to_string())
        })?;
        let rows = match self
            .client
            .query(
                &format!(
                    "with authorized as materialized (
                        select record.*, vector.embedding
                          from {} vector
                          join memory_records record
                            on record.record_kind = vector.record_kind
                           and record.id = vector.record_id
                         where vector.owner_subject = $1 and vector.memory_scope = $2
                           and vector.embedding_version = $4
                           and vector.content_key = record.content_key
                           and record.owner_subject = $1 and record.memory_scope = $2
                           and record.status = 'active' and record.effective_to is null
                           and not exists ({ACTIVE_MEMORY_SUCCESSOR_QUERY})
                           and exists (
                               select 1 from memory_embedding_active_versions active
                                where active.owner_subject = $1 and active.memory_scope = $2
                                  and active.embedding_version = $4
                                  and active.snapshot_revision = $5
                           )
                    )
                    select {QUALIFIED_DURABLE_MEMORY_RECORD_COLUMNS},
                           (1 - (record.embedding OPERATOR({}.<=>) $3::text::{}.vector))::real
                               as recall_score
                      from authorized record
                     order by record.embedding OPERATOR({}.<=>) $3::text::{}.vector asc,
                              record.importance desc, record.observed_at desc,
                              record.record_kind asc, record.id asc
                     limit $6",
                    generation.vector_table,
                    quote_identifier(&vector_schema),
                    quote_identifier(&vector_schema),
                    quote_identifier(&vector_schema),
                    quote_identifier(&vector_schema),
                ),
                &[
                    &request.owner_subject,
                    &request.memory_scope,
                    &literal,
                    &generation.embedding_version,
                    &generation.snapshot_revision,
                    &limit,
                ],
            )
            .await
        {
            Ok(rows) => rows,
            Err(error) if is_missing_pgvector_relation(&error) => {
                return Ok((Vec::new(), DenseRecallStatus::Unavailable));
            }
            Err(error) => return Err(store_error(error)),
        };
        let candidates = rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                Ok(RankedMemoryCandidate {
                    record: row_to_stored_memory_record(row.clone())?,
                    rank: u32::try_from(index + 1).expect("bounded hybrid rank fits u32"),
                    score: row.get("recall_score"),
                    embedding_version: Some(generation.embedding_version.clone()),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((candidates, DenseRecallStatus::Applied))
    }

    /// Returns candidates from the one fully-covered active embedding generation. Missing or
    /// stale dense infrastructure returns `None` so direct callers can fuse lexical-only results.
    pub async fn memory_dense_candidates(
        &self,
        request: &HybridRecallRequest,
        dense_query: &DenseRecallQuery,
    ) -> Result<Option<Vec<RankedMemoryCandidate>>> {
        let (candidates, status) = self
            .memory_dense_candidates_with_status(request, dense_query)
            .await?;
        Ok((status == DenseRecallStatus::Applied).then_some(candidates))
    }

    pub async fn memory_hybrid_candidates(
        &self,
        request: &HybridRecallRequest,
        query: &str,
        dense_query: Option<&DenseRecallQuery>,
    ) -> Result<HybridRecallResult> {
        let lexical = self.memory_lexical_candidates(request, query).await?;
        let (dense, dense_status) = match dense_query {
            Some(query) => match self
                .memory_dense_candidates_with_status(request, query)
                .await
            {
                Ok(result) => result,
                Err(ServerError::InvalidRequest(_)) => {
                    (Vec::new(), DenseRecallStatus::GenerationChanged)
                }
                Err(error) => return Err(error),
            },
            None => (Vec::new(), DenseRecallStatus::NotRequested),
        };
        let candidates = fuse_hybrid_candidates(request, lexical, dense)
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        Ok(HybridRecallResult {
            candidates,
            dense_status,
        })
    }
}

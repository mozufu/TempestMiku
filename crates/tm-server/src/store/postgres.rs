mod auth_devices;
mod memory_readiness;
mod memory_retrieval;
mod memory_write;
mod rows;
mod schema;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rows::{
    row_to_approval_effect, row_to_approval_request, row_to_cron_job, row_to_cron_run,
    row_to_dream_record, row_to_evolution_audit, row_to_evolution_review_proposal,
    row_to_memory_embedding_job, row_to_memory_summary, row_to_message_record, row_to_project_item,
    row_to_session_record, row_to_session_turn, row_to_skill_proposal, row_to_stored_memory_record,
};
use serde_json::{Value, json};
use tm_host::{EvolutionAuditRecord, EvolutionAuditStatus, EvolutionPolicyReason};
use tm_memory::{
    DreamLease, DreamQueueRecord, DreamStatus, MemoryEmbeddingJobRecord, MemoryRecordKind,
    MemoryRecordResource, MemoryScopeTombstone, MemorySummaryRecord, NewDreamQueueRecord,
    NewMemoryEmbeddingJob, NewMemorySummaryRecord, NewSkillProposalRecord, SkillProposalRecord,
    SkillProposalStatus, StoredMemoryRecord,
};
use tm_modes::{ReviewProposalStatus, ReviewProposalTarget};
use tokio_postgres::{Config as PostgresConfig, NoTls, error::SqlState};
use uuid::Uuid;

use crate::{Result, ServerError};

use memory_write::upsert_typed_memory_record;

use super::models::{
    redact_persisted_json, redact_persisted_text, sanitize_cron_job_persistence,
    sanitize_cron_run_persistence, sanitize_durable_memory_record,
    sanitize_evolution_review_proposal_persistence, sanitize_memory_summary_persistence,
    sanitize_new_memory_embedding_job, sanitize_project_item_persistence,
    sanitize_skill_proposal_persistence, turn_content_hash, validate_persistence_identifier,
    validate_profile_fact_persistence, validate_recall_chunk_persistence,
};

use super::{
    ApprovalEffectLease, ApprovalEffectRecord, ApprovalRequestRecord, CronJobRecord, CronLease,
    CronRunRecord, EndSessionDreamResult, EvolutionAuditEntry, EvolutionReviewProposalRecord,
    MessageRecord, ModeState, NewApprovalRequest, NewApprovalResolution, NewCronJobRecord,
    NewCronRunRecord, NewEvolutionReviewProposal, NewProjectItem, NewSession, ProfileFactRecord,
    ProjectItemKind, ProjectItemRecord, RecallChunkRecord, SessionEvent, SessionRecord,
    SessionSummaryRecord, SessionTurnRecord, Store, StoreRuntimeMetrics,
};

fn evolution_audit_entry(
    payload: &Value,
    approval_id: Uuid,
    effect_id: Uuid,
    status: EvolutionAuditStatus,
    occurred_at: DateTime<Utc>,
    error_code: Option<EvolutionPolicyReason>,
    idempotency_key: String,
) -> Result<Option<EvolutionAuditEntry>> {
    Ok(crate::evolution::evolution_audit_record(
        payload,
        approval_id,
        effect_id,
        status,
        occurred_at,
        error_code,
    )?
    .map(|record| EvolutionAuditEntry {
        idempotency_key,
        record,
    }))
}

fn evolution_audit_params(
    entry: Option<EvolutionAuditEntry>,
) -> Result<(Option<Value>, Option<String>)> {
    entry
        .map(|entry| {
            serde_json::to_value(entry.record)
                .map(|record| (Some(record), Some(entry.idempotency_key)))
                .map_err(|error| ServerError::Store(error.to_string()))
        })
        .unwrap_or_else(|| Ok((None, None)))
}

pub struct PostgresStore {
    client: tokio_postgres::Client,
    memory_mutation_client: tokio::sync::Mutex<tokio_postgres::Client>,
}

pub(super) const DURABLE_MEMORY_RECORD_COLUMNS: &str = "record_kind, id, schema_version, owner_subject, memory_scope, text, semantic_subject, predicate, object, evidence_json, confidence, importance, observed_at, effective_from, effective_to, status, corrects_record_id, corrected_by_record_id, supersedes_record_id, superseded_by_record_id, content_key, version_key, created_at";

pub(super) const MEMORY_EMBEDDING_JOB_COLUMNS: &str = "j.id, j.record_kind, j.record_id, j.owner_subject, j.memory_scope, j.content_key, j.reembedding_key, j.status, j.input_limit_bytes, j.failure_code, j.attempts, j.available_at, j.locked_at, j.lease_owner, j.lease_epoch, j.cancelled_at, j.created_at, j.updated_at, p.schema_version, p.provider, p.model_id, p.dimensions, p.normalization, p.content_hash, p.embedding_version, p.created_at as provenance_created_at, p.reembedding_state";

pub(super) const ACTIVE_MEMORY_SUCCESSOR_QUERY: &str = "
with recursive successors(id) as (
    select successor.id from memory_records successor
     where successor.owner_subject = record.owner_subject
       and successor.memory_scope = record.memory_scope
       and (successor.corrects_record_id = record.id
            or successor.supersedes_record_id = record.id
            or successor.id = record.corrected_by_record_id
            or successor.id = record.superseded_by_record_id)
    union
    select successor.id from memory_records successor
     join successors prior on true
     join memory_records prior_record on prior_record.id = prior.id
      and prior_record.owner_subject = record.owner_subject
      and prior_record.memory_scope = record.memory_scope
     where successor.owner_subject = record.owner_subject
       and successor.memory_scope = record.memory_scope
       and (successor.corrects_record_id = prior.id
            or successor.supersedes_record_id = prior.id
            or successor.id = prior_record.corrected_by_record_id
            or successor.id = prior_record.superseded_by_record_id)
)
select 1 from memory_records successor
 join successors on successors.id = successor.id
 where successor.status = 'active' and successor.effective_to is null
";

pub(super) fn postgres_memory_error(error: tokio_postgres::Error) -> ServerError {
    if error.code().is_some_and(|code| code.code() == "TM001") {
        ServerError::NotFound("revoked memory scope".to_string())
    } else {
        let message = error
            .as_db_error()
            .map(|database| format!("{} [{}]", database.message(), database.code().code()))
            .unwrap_or_else(|| error.to_string());
        ServerError::Store(tm_memory::redact_dream_text(&message).text)
    }
}

impl PostgresStore {
    const SCHEMA_LOCK_ID: i64 = 0x544d_5343_4845_4d41;

    pub async fn connect(dsn: &str) -> Result<Self> {
        let config = dsn
            .parse::<PostgresConfig>()
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Self::connect_config(config).await
    }

    #[cfg(test)]
    pub(crate) async fn connect_in_schema(dsn: &str, schema: &str) -> Result<Self> {
        if schema.is_empty()
            || !schema
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(ServerError::InvalidRequest(
                "test schema must be a safe Postgres identifier".to_string(),
            ));
        }
        let mut config = dsn
            .parse::<PostgresConfig>()
            .map_err(|err| ServerError::Store(err.to_string()))?;
        config.options(format!("-c search_path={schema}"));
        Self::connect_config(config).await
    }

    async fn connect_config(config: PostgresConfig) -> Result<Self> {
        let (client, connection) = config
            .clone()
            .connect(NoTls)
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                let err = tm_memory::redact_dream_text(&err.to_string()).text;
                tracing::error!(%err, "postgres connection failed");
            }
        });
        let (memory_mutation_client, mutation_connection) = config
            .connect(NoTls)
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        tokio::spawn(async move {
            if let Err(err) = mutation_connection.await {
                let err = tm_memory::redact_dream_text(&err.to_string()).text;
                tracing::error!(%err, "postgres memory mutation connection failed");
            }
        });
        let mut store = Self {
            client,
            memory_mutation_client: tokio::sync::Mutex::new(memory_mutation_client),
        };
        store.ensure_schema().await?;
        Ok(store)
    }

    pub fn client(&self) -> &tokio_postgres::Client {
        &self.client
    }
}

#[async_trait]
impl Store for PostgresStore {
    async fn create_session(&self, new: NewSession) -> Result<SessionRecord> {
        let owner_subject = self
            .client
            .query_opt(
                "select owner_subject from server_authority where singleton = true",
                &[],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .map(|row| row.get("owner_subject"))
            .unwrap_or_else(|| "owner".to_string());
        let session = SessionRecord {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            status: "open".to_string(),
            mode: new.mode.clone(),
            mode_state: ModeState::new(new.mode, Utc::now()),
            persona_status: new.persona_status,
            owner_subject,
            memory_scope: "global".to_string(),
        };
        let mode = serde_json::to_value(session.mode.clone())
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let mode_state = serde_json::to_value(&session.mode_state)
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let persona_status = serde_json::to_value(&session.persona_status)
            .map_err(|err| ServerError::Store(err.to_string()))?;
        self.client
            .execute(
                "insert into sessions (id, created_at, updated_at, status, mode, mode_state_json, persona_status, owner_subject, memory_scope) values ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                &[&session.id, &session.created_at, &session.updated_at, &session.status, &mode, &mode_state, &persona_status, &session.owner_subject, &session.memory_scope],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(session)
    }

    async fn configure_owner_subject(&self, owner_subject: &str) -> Result<usize> {
        let owner_subject = owner_subject.trim();
        if owner_subject.is_empty() {
            return Err(ServerError::InvalidRequest(
                "owner subject must not be empty".to_string(),
            ));
        }
        validate_persistence_identifier("owner subject", owner_subject)?;
        let row = self
            .client
            .query_one(
                "with authority_lock as (
                    select pg_advisory_xact_lock(hashtext('tm-server-authority'))
                 ), current_authority as (
                    select server_authority.owner_subject
                      from server_authority, authority_lock
                     where singleton = true
                     for update
                 ), changed as (
                    select count(*)::bigint as count
                      from sessions, current_authority
                     where sessions.owner_subject <> $1
                 ), authority as (
                    update server_authority
                       set owner_subject = $1, updated_at = now()
                     where singleton = true
                       and owner_subject in ('owner', $1)
                       and exists (select 1 from current_authority)
                    returning owner_subject
                 )
                 select exists (select 1 from authority) as configured,
                        coalesce((select count from changed), 0)::bigint as updated,
                        (select owner_subject from current_authority) as prior_owner",
                &[&owner_subject],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        if !row.get::<_, bool>("configured") {
            return Err(ServerError::Conflict(
                "server is already configured for a different owner subject".to_string(),
            ));
        }
        let prior_owner: String = row.get("prior_owner");
        if prior_owner != owner_subject {
            self.client
                .execute(
                    "update memory_records
                        set owner_subject = $1
                      where owner_subject = $2
                        and content_key like 'legacy-memory-content-v1:%'",
                    &[&owner_subject, &prior_owner],
                )
                .await
                .map_err(|error| ServerError::Store(error.to_string()))?;
            self.client
                .execute(
                    "update memory_embedding_jobs
                        set owner_subject = $1, updated_at = now()
                      where owner_subject = $2",
                    &[&owner_subject, &prior_owner],
                )
                .await
                .map_err(|error| ServerError::Store(error.to_string()))?;
            self.client
                .execute(
                    "insert into memory_scope_tombstones(
                        owner_subject, memory_scope, link_alias, reason, revoked_at
                     )
                     select $1, memory_scope, link_alias, reason, revoked_at
                       from memory_scope_tombstones
                      where owner_subject = $2
                     on conflict (owner_subject, memory_scope) do update set
                        link_alias = coalesce(memory_scope_tombstones.link_alias, excluded.link_alias),
                        reason = excluded.reason,
                        revoked_at = least(memory_scope_tombstones.revoked_at, excluded.revoked_at)",
                    &[&owner_subject, &prior_owner],
                )
                .await
                .map_err(|error| ServerError::Store(error.to_string()))?;
            self.client
                .execute(
                    "delete from memory_scope_tombstones where owner_subject = $1",
                    &[&prior_owner],
                )
                .await
                .map_err(|error| ServerError::Store(error.to_string()))?;
        }
        Ok(row.get::<_, i64>("updated").max(0) as usize)
    }

    async fn set_session_memory_scope(
        &self,
        session_id: Uuid,
        memory_scope: &str,
    ) -> Result<SessionRecord> {
        let memory_scope = memory_scope.trim();
        if memory_scope.is_empty() {
            return Err(ServerError::InvalidRequest(
                "memory scope must not be empty".to_string(),
            ));
        }
        validate_persistence_identifier("memory scope", memory_scope)?;
        let row = self
            .client
            .query_opt(
                "update sessions
                    set memory_scope = $2, updated_at = now()
                  where id = $1 and status <> 'ended'
                  returning id, created_at, updated_at, status, mode, mode_state_json,
                            persona_status, owner_subject, memory_scope",
                &[&session_id, &memory_scope],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        match row {
            Some(row) => row_to_session_record(&row),
            None => {
                let session = self.get_session(session_id).await?;
                Err(ServerError::Conflict(format!(
                    "session {} has {}",
                    session.id, session.status
                )))
            }
        }
    }

    async fn end_session(&self, session_id: Uuid) -> Result<SessionRecord> {
        let session_key = session_id.to_string();
        let row = self
            .client
            .query_opt(
                "with advisory_lock as (
                    select pg_advisory_xact_lock(hashtext($1))
                 ), locked_session as (
                    select sessions.id
                      from sessions, advisory_lock
                     where sessions.id = $2
                     for update
                 ), active_turn as (
                    select exists (
                        select 1
                          from session_turns, locked_session
                         where session_turns.session_id = $2
                           and session_turns.status in ('queued', 'running')
                    ) as present
                 ), ended as (
                    update sessions
                       set status = 'ended', updated_at = now()
                     where id = $2
                       and not (select present from active_turn)
                    returning id
                 )
                 select active_turn.present as has_active_turn,
                        (select id from ended) as ended_id
                   from locked_session, active_turn",
                &[&session_key, &session_id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        if row.get::<_, bool>("has_active_turn") {
            return Err(ServerError::Conflict(format!(
                "session {session_id} has queued or running turns"
            )));
        }
        let ended_id: Option<Uuid> = row.get("ended_id");
        if ended_id != Some(session_id) {
            return Err(ServerError::Store(format!(
                "session {session_id} did not reach ended state"
            )));
        }
        self.get_session(session_id).await
    }

    async fn end_session_and_enqueue_dream(
        &self,
        session_id: Uuid,
        subject: String,
        scope: String,
    ) -> Result<EndSessionDreamResult> {
        validate_persistence_identifier("dream subject", &subject)?;
        validate_persistence_identifier("dream scope", &scope)?;
        let new = NewDreamQueueRecord::session_end(session_id, subject, scope, None);
        let dream_id = Uuid::new_v4();
        let queued = DreamStatus::Queued.as_str();
        let reason = new.reason.as_str();
        let session_key = session_id.to_string();
        let end_payload = serde_json::json!({
            "status": "ended",
            "reason": "user_requested",
        });
        let row = self
            .client
            .query_opt(
                "with advisory_lock as (
                    select pg_advisory_xact_lock(hashtext($1))
                 ), locked_session as (
                    select sessions.status, sessions.owner_subject, sessions.memory_scope
                      from sessions, advisory_lock
                     where sessions.id = $2
                     for update
                 ), active_turn as (
                    select exists (
                        select 1
                          from session_turns, locked_session
                         where session_turns.session_id = $2
                           and session_turns.status in ('queued', 'running')
                    ) as present
                 ), base_seq as (
                    select coalesce(max(session_events.seq), 0)::bigint as seq
                      from session_events, locked_session
                     where session_events.session_id = $2
                 ), prior_end as (
                    select seq
                      from session_events
                     where session_id = $2 and event_type = 'session_end'
                     order by seq desc
                     limit 1
                 ), lifecycle as (
                    select locked_session.status,
                           locked_session.owner_subject,
                           locked_session.memory_scope,
                           active_turn.present as has_active_turn,
                           base_seq.seq,
                           (locked_session.status <> 'ended'
                             or not exists (select 1 from prior_end)) as needs_end,
                           case
                             when locked_session.status <> 'ended'
                               or not exists (select 1 from prior_end)
                             then base_seq.seq + 2
                             else (select seq from prior_end)
                           end as source_event_seq
                      from locked_session, active_turn, base_seq
                 ), ended as (
                    update sessions
                       set status = 'ended', updated_at = now()
                     where id = $2
                       and exists (select 1 from locked_session)
                       and not (select has_active_turn from lifecycle)
                     returning id
                 ), dream as (
                    insert into dream_queue
                        (id, session_id, subject, scope, reason, status, dedupe_key,
                         source_event_seq, attempts, enqueued_at, available_at, locked_at,
                         lease_owner, lease_epoch, heartbeat_at, completed_at, error_at, last_error)
                    select $4, $2, lifecycle.owner_subject, lifecycle.memory_scope, $5, $6, $7,
                           lifecycle.source_event_seq, 0, now(), $8, null, null, 0,
                           null, null, null, null
                      from lifecycle
                     where not lifecycle.has_active_turn
                    on conflict (dedupe_key) do update
                       set subject = excluded.subject,
                           scope = excluded.scope,
                           source_event_seq = coalesce(
                           dream_queue.source_event_seq,
                           excluded.source_event_seq
                       )
                    returning id, (xmax = 0) as inserted
                 ), dream_event as (
                    insert into session_events
                        (session_id, seq, event_type, payload_json, actor_id, artifact_uri, history_uri, created_at)
                    select $2, lifecycle.seq + 1,
                           'dream_queued',
                           jsonb_build_object(
                               'dreamId', dream.id,
                               'sessionId', $2,
                               'reason', $5,
                               'status', $6,
                               'subject', lifecycle.owner_subject,
                               'scope', lifecycle.memory_scope,
                               'dedupeKey', $7,
                               'sourceEventSeq', lifecycle.source_event_seq,
                               'availableAt', $8
                           ),
                           null, null, null, now()
                      from lifecycle, dream
                     where dream.inserted
                    returning seq
                 ), end_event as (
                    insert into session_events
                        (session_id, seq, event_type, payload_json, actor_id, artifact_uri, history_uri, created_at)
                    select $2, lifecycle.source_event_seq, 'session_end',
                           $3 || jsonb_build_object(
                               'subject', lifecycle.owner_subject,
                               'scope', lifecycle.memory_scope
                           ),
                           null, null, null, now()
                      from lifecycle
                     where lifecycle.needs_end and not lifecycle.has_active_turn
                    returning seq
                 )
                 select dream.id as dream_id,
                        lifecycle.status <> 'ended' as newly_ended,
                        lifecycle.has_active_turn,
                        (select seq from end_event) as end_event_seq,
                        (select seq from dream_event) as dream_event_seq
                   from lifecycle
                   left join dream on true",
                &[
                    &session_key,
                    &session_id,
                    &end_payload,
                    &dream_id,
                    &reason,
                    &queued,
                    &new.dedupe_key,
                    &new.available_at,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        if row.get::<_, bool>("has_active_turn") {
            return Err(ServerError::Conflict(format!(
                "session {session_id} has queued or running turns"
            )));
        }
        let dream_id: Uuid = row
            .get::<_, Option<Uuid>>("dream_id")
            .ok_or_else(|| ServerError::Store("session end produced no dream row".to_string()))?;
        let newly_ended: bool = row.get("newly_ended");
        let end_event_seq: Option<i64> = row.get("end_event_seq");
        let dream_event_seq: Option<i64> = row.get("dream_event_seq");
        let session = self.get_session(session_id).await?;
        let dream = self.dream(dream_id).await?;
        let min_seq = [end_event_seq, dream_event_seq].into_iter().flatten().min();
        let events = match min_seq {
            Some(seq) => self
                .events_after(session_id, Some(seq - 1))
                .await?
                .into_iter()
                .filter(|event| {
                    Some(event.seq) == end_event_seq || Some(event.seq) == dream_event_seq
                })
                .collect(),
            None => Vec::new(),
        };
        Ok(EndSessionDreamResult {
            session,
            dream,
            events,
            newly_ended,
        })
    }

    async fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummaryRecord>> {
        let rows = self.client
            .query(
                "select s.id,
                        s.created_at,
                        s.updated_at,
                        s.status,
                        s.mode,
                        s.mode_state_json,
                        s.persona_status,
                        s.owner_subject,
                        s.memory_scope,
                        coalesce(m.message_count, 0)::bigint as message_count,
                        coalesce(p.summary, m.assistant_summary) as summary,
                        m.title,
                        m.preview,
                        e.last_event_id
                   from sessions s
                   left join lateral (
                        select
                          (select count(*)::bigint from messages where session_id = s.id) as message_count,
                          (select content from messages where session_id = s.id and role = 'user' order by seq asc limit 1) as title,
                          (select content from messages where session_id = s.id and role = 'assistant' order by seq desc limit 1) as assistant_summary,
                          (select content from messages where session_id = s.id order by seq desc limit 1) as preview
                   ) m on true
                   left join lateral (
                        select text as summary
                          from project_items
                         where source_session_id = s.id
                           and kind = 'summary'
                         order by created_at desc
                         limit 1
                   ) p on true
                   left join lateral (
                        select max(seq) as last_event_id from session_events where session_id = s.id
                   ) e on true
                  order by s.updated_at desc
                  limit $1",
                &[&(limit as i64)],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        rows.into_iter()
            .map(|row| {
                Ok(SessionSummaryRecord {
                    session: row_to_session_record(&row)?,
                    summary: row.get("summary"),
                    title: row.get("title"),
                    preview: row.get("preview"),
                    message_count: row.get("message_count"),
                    last_event_id: row.get("last_event_id"),
                })
            })
            .collect()
    }

    async fn get_session(&self, session_id: Uuid) -> Result<SessionRecord> {
        let row = self.client
            .query_opt("select id, created_at, updated_at, status, mode, mode_state_json, persona_status, owner_subject, memory_scope from sessions where id = $1", &[&session_id])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        row_to_session_record(&row)
    }

    async fn session_messages(&self, session_id: Uuid) -> Result<Vec<MessageRecord>> {
        self.get_session(session_id).await?;
        let rows = self.client
            .query("select session_id, seq, role, content, turn_id, created_at from messages where session_id = $1 order by seq asc", &[&session_id])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(rows.into_iter().map(row_to_message_record).collect())
    }

    async fn set_mode_state(
        &self,
        session_id: Uuid,
        mode_state: ModeState,
    ) -> Result<SessionRecord> {
        self.get_session(session_id).await?;
        let mode = serde_json::to_value(mode_state.mode.clone())
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let mode_state_json =
            serde_json::to_value(&mode_state).map_err(|err| ServerError::Store(err.to_string()))?;
        self.client
            .execute(
                "update sessions set mode = $2, mode_state_json = $3, updated_at = now() where id = $1",
                &[&session_id, &mode, &mode_state_json],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        self.get_session(session_id).await
    }

    async fn append_message(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
    ) -> Result<MessageRecord> {
        self.append_message_for_turn(session_id, role, content, None)
            .await
    }

    async fn append_message_for_turn(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
        turn_id: Option<Uuid>,
    ) -> Result<MessageRecord> {
        validate_persistence_identifier("message role", role)?;
        let content = redact_persisted_text(content);
        self.get_session(session_id).await?;
        if let Some(turn_id) = turn_id {
            let turn = self.turn(turn_id).await?;
            if turn.session_id != session_id {
                return Err(ServerError::NotFound(format!("turn {turn_id}")));
            }
        }
        let session_key = session_id.to_string();
        let row = self
            .client
            .query_one(
                "with lock as (select pg_advisory_xact_lock(hashtext($1))),
                      next_seq as (select coalesce(max(seq) + 1, 1) as seq from messages, lock where session_id = $2),
                      inserted as (
                        insert into messages (session_id, seq, role, content, turn_id, created_at)
                        select $2, next_seq.seq, $3, $4, $5, now() from next_seq
                        returning seq, created_at
                      ),
                      touch as (
                        update sessions set updated_at = (select created_at from inserted) where id = $2
                      )
                 select seq, created_at from inserted",
                &[&session_key, &session_id, &role, &content, &turn_id],
            )
            .await
            .map_err(|err| {
                if err.code() == Some(&SqlState::UNIQUE_VIOLATION) {
                    ServerError::Conflict(format!(
                        "turn-linked {role} message already exists"
                    ))
                } else {
                    ServerError::Store(err.to_string())
                }
            })?;
        Ok(MessageRecord {
            session_id,
            seq: row.get("seq"),
            role: role.to_string(),
            content,
            turn_id,
            created_at: row.get("created_at"),
        })
    }

    async fn append_event(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
    ) -> Result<SessionEvent> {
        self.append_event_for_turn(session_id, event_type, payload_json, None)
            .await
    }

    async fn append_event_for_turn(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
        turn_id: Option<Uuid>,
    ) -> Result<SessionEvent> {
        validate_persistence_identifier("event type", event_type)?;
        let payload_json = redact_persisted_json(payload_json);
        self.get_session(session_id).await?;
        if let Some(turn_id) = turn_id {
            let turn = self.turn(turn_id).await?;
            if turn.session_id != session_id {
                return Err(ServerError::NotFound(format!("turn {turn_id}")));
            }
        }
        let session_key = session_id.to_string();
        let output_refs = SessionEvent::output_refs(event_type, &payload_json);
        let row = self
            .client
            .query_one(
                "with lock as (select pg_advisory_xact_lock(hashtext($1))),
                      next_seq as (select coalesce(max(seq) + 1, 1) as seq from session_events, lock where session_id = $2),
                      inserted as (
                        insert into session_events (session_id, seq, event_type, payload_json, actor_id, artifact_uri, history_uri, turn_id, created_at)
                        select $2, next_seq.seq, $3, $4, $5, $6, $7, $8, now() from next_seq
                        returning seq, created_at
                      ),
                      touch as (
                        update sessions set updated_at = (select created_at from inserted) where id = $2
                      )
                 select seq, created_at from inserted",
                &[
                    &session_key,
                    &session_id,
                    &event_type,
                    &payload_json,
                    &output_refs.actor_id,
                    &output_refs.artifact_uri,
                    &output_refs.history_uri,
                    &turn_id,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let mut event = SessionEvent::new(
            session_id,
            row.get("seq"),
            event_type,
            payload_json,
            row.get("created_at"),
        );
        event.turn_id = turn_id;
        Ok(event)
    }

    async fn append_event_for_turn_once(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
        turn_id: Uuid,
    ) -> Result<(SessionEvent, bool)> {
        validate_persistence_identifier("event type", event_type)?;
        let payload_json = redact_persisted_json(payload_json);
        self.get_session(session_id).await?;
        let turn = self.turn(turn_id).await?;
        if turn.session_id != session_id {
            return Err(ServerError::NotFound(format!("turn {turn_id}")));
        }
        let session_key = session_id.to_string();
        let output_refs = SessionEvent::output_refs(event_type, &payload_json);
        let mut collision_retries = 0usize;
        let row = loop {
            let result = self
                .client
                .query_one(
                    "with lock as (
                    select pg_advisory_xact_lock(hashtext($1))
                 ), existing as (
                    select event.session_id, event.seq, event.event_type, event.payload_json,
                           event.actor_id, event.artifact_uri, event.history_uri, event.turn_id,
                           event.created_at, false as inserted
                      from session_events event, lock
                     where event.session_id = $2 and event.turn_id = $8
                       and event.event_type = $3
                     order by event.seq asc limit 1
                 ), next_seq as (
                    select coalesce(max(event.seq) + 1, 1) as seq
                      from session_events event, lock
                     where event.session_id = $2
                    having not exists (select 1 from existing)
                 ), inserted as (
                    insert into session_events (
                        session_id, seq, event_type, payload_json, actor_id, artifact_uri,
                        history_uri, turn_id, created_at
                    )
                    select $2, next_seq.seq, $3, $4, $5, $6, $7, $8, now()
                      from next_seq
                    returning session_id, seq, event_type, payload_json, actor_id, artifact_uri,
                              history_uri, turn_id, created_at, true as inserted
                 ), touch as (
                    update sessions
                       set updated_at = (select created_at from inserted)
                     where id = $2 and exists (select 1 from inserted)
                 )
                 select session_id, seq, event_type, payload_json, actor_id, artifact_uri,
                        history_uri, turn_id, created_at, inserted
                   from existing
                 union all
                 select session_id, seq, event_type, payload_json, actor_id, artifact_uri,
                        history_uri, turn_id, created_at, inserted
                   from inserted
                  order by seq asc limit 1",
                    &[
                        &session_key,
                        &session_id,
                        &event_type,
                        &payload_json,
                        &output_refs.actor_id,
                        &output_refs.artifact_uri,
                        &output_refs.history_uri,
                        &turn_id,
                    ],
                )
                .await;
            match result {
                Ok(row) => break row,
                // A competing session event may have committed after this statement acquired its
                // MVCC snapshot. The primary-key collision is the fence; retrying gets a fresh
                // snapshot and either observes the existing typed event or allocates the next seq.
                Err(error)
                    if error.code() == Some(&SqlState::UNIQUE_VIOLATION)
                        && collision_retries < 8 =>
                {
                    collision_retries += 1;
                    continue;
                }
                Err(error) if error.code() == Some(&SqlState::UNIQUE_VIOLATION) => {
                    return Err(ServerError::Conflict(
                        "could not allocate an exact-once turn event after concurrent writes"
                            .to_string(),
                    ));
                }
                Err(error) => return Err(ServerError::Store(error.to_string())),
            }
        };
        Ok((
            SessionEvent {
                session_id: row.get("session_id"),
                seq: row.get("seq"),
                event_type: row.get("event_type"),
                payload_json: row.get("payload_json"),
                actor_id: row.get("actor_id"),
                artifact_uri: row.get("artifact_uri"),
                history_uri: row.get("history_uri"),
                turn_id: row.get("turn_id"),
                created_at: row.get("created_at"),
            },
            row.get("inserted"),
        ))
    }

    async fn events_after(
        &self,
        session_id: Uuid,
        last_event_id: Option<i64>,
    ) -> Result<Vec<SessionEvent>> {
        self.get_session(session_id).await?;
        let min_seq = last_event_id.unwrap_or(0);
        let rows = self.client
            .query("select session_id, seq, event_type, payload_json, actor_id, artifact_uri, history_uri, turn_id, created_at from session_events where session_id = $1 and seq > $2 order by seq asc", &[&session_id, &min_seq])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| SessionEvent {
                session_id: row.get("session_id"),
                seq: row.get("seq"),
                event_type: row.get("event_type"),
                payload_json: row.get("payload_json"),
                actor_id: row.get("actor_id"),
                artifact_uri: row.get("artifact_uri"),
                history_uri: row.get("history_uri"),
                turn_id: row.get("turn_id"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    async fn event_for_turn(
        &self,
        session_id: Uuid,
        turn_id: Uuid,
        event_type: &str,
    ) -> Result<Option<SessionEvent>> {
        self.get_session(session_id).await?;
        let row = self
            .client
            .query_opt(
                "select session_id, seq, event_type, payload_json, actor_id, artifact_uri,
                        history_uri, turn_id, created_at
                   from session_events
                  where session_id = $1 and turn_id = $2 and event_type = $3
                  order by seq asc limit 1",
                &[&session_id, &turn_id, &event_type],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        Ok(row.map(|row| SessionEvent {
            session_id: row.get("session_id"),
            seq: row.get("seq"),
            event_type: row.get("event_type"),
            payload_json: row.get("payload_json"),
            actor_id: row.get("actor_id"),
            artifact_uri: row.get("artifact_uri"),
            history_uri: row.get("history_uri"),
            turn_id: row.get("turn_id"),
            created_at: row.get("created_at"),
        }))
    }

    async fn events_by_type(
        &self,
        session_id: Uuid,
        event_type: &str,
        limit: usize,
    ) -> Result<Vec<SessionEvent>> {
        self.get_session(session_id).await?;
        let limit = i64::try_from(limit)
            .map_err(|_| ServerError::InvalidRequest("event limit is too large".to_string()))?;
        let rows = self
            .client
            .query(
                "select session_id, seq, event_type, payload_json, actor_id, artifact_uri,
                        history_uri, turn_id, created_at
                   from (
                        select session_id, seq, event_type, payload_json, actor_id, artifact_uri,
                               history_uri, turn_id, created_at
                          from session_events
                         where session_id = $1 and event_type = $2
                         order by seq desc limit $3
                   ) bounded
                  order by seq asc",
                &[&session_id, &event_type, &limit],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| SessionEvent {
                session_id: row.get("session_id"),
                seq: row.get("seq"),
                event_type: row.get("event_type"),
                payload_json: row.get("payload_json"),
                actor_id: row.get("actor_id"),
                artifact_uri: row.get("artifact_uri"),
                history_uri: row.get("history_uri"),
                turn_id: row.get("turn_id"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    async fn memory_recall_events(
        &self,
        session_id: Uuid,
        owner_subject: &str,
        memory_scope: &str,
        limit: usize,
    ) -> Result<Vec<SessionEvent>> {
        self.get_session(session_id).await?;
        let limit = i64::try_from(limit)
            .map_err(|_| ServerError::InvalidRequest("event limit is too large".to_string()))?;
        let rows = self
            .client
            .query(
                "select session_id, seq, event_type, payload_json, actor_id, artifact_uri,
                        history_uri, turn_id, created_at
                   from (
                        select session_id, seq, event_type, payload_json, actor_id, artifact_uri,
                               history_uri, turn_id, created_at
                          from session_events
                         where session_id = $1
                           and event_type = 'memory_recall'
                           and payload_json #>> '{context,subject}' = $2
                           and payload_json #>> '{context,scope}' = $3
                         order by seq desc limit $4
                   ) bounded
                  order by seq asc",
                &[&session_id, &owner_subject, &memory_scope, &limit],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| SessionEvent {
                session_id: row.get("session_id"),
                seq: row.get("seq"),
                event_type: row.get("event_type"),
                payload_json: row.get("payload_json"),
                actor_id: row.get("actor_id"),
                artifact_uri: row.get("artifact_uri"),
                history_uri: row.get("history_uri"),
                turn_id: row.get("turn_id"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    async fn enqueue_turn(
        &self,
        session_id: Uuid,
        client_message_id: &str,
        content: &str,
    ) -> Result<SessionTurnRecord> {
        let client_message_id = client_message_id.trim();
        if client_message_id.is_empty() || client_message_id.chars().count() > 128 {
            return Err(ServerError::InvalidRequest(
                "client message id must contain 1 to 128 characters".to_string(),
            ));
        }
        validate_persistence_identifier("client message id", client_message_id)?;
        if content.trim().is_empty() {
            return Err(ServerError::InvalidRequest(
                "turn content must not be empty".to_string(),
            ));
        }
        let turn_id = Uuid::new_v4();
        let content_hash = turn_content_hash(content);
        let persisted_content = redact_persisted_text(content);
        let now = Utc::now();
        let session_key = session_id.to_string();
        let row = self
            .client
            .query_opt(
                "with advisory_lock as (
                    select pg_advisory_xact_lock(hashtext($1))
                 ), locked_session as (
                    select sessions.status
                      from sessions, advisory_lock
                     where sessions.id = $2
                     for update
                 ), inserted_turn as (
                    insert into session_turns
                        (id, session_id, client_message_id, content, content_hash, status,
                         created_at, updated_at, started_at, completed_at, worker_id, error)
                    select $3, $2, $4, $5, $6, 'queued', $7, $7, null, null, null, null
                     from locked_session
                     where locked_session.status <> 'ended'
                    on conflict (session_id, client_message_id) do update
                       set client_message_id = excluded.client_message_id
                    returning id, session_id, client_message_id, content, content_hash, status,
                              created_at, updated_at, started_at, completed_at, worker_id, error,
                              (xmax = 0) as inserted
                 ), selected as (
                    select * from inserted_turn
                 ), next_seq as (
                    select coalesce(max(messages.seq) + 1, 1)::bigint as seq
                      from messages, locked_session
                     where messages.session_id = $2
                 ), user_message as (
                    insert into messages(session_id, seq, role, content, turn_id, created_at)
                    select $2, next_seq.seq, 'user', selected.content, selected.id, $7
                      from selected, next_seq
                     where selected.inserted
                    returning seq
                 ), touch as (
                    update sessions
                       set updated_at = $7
                     where id = $2 and exists (select 1 from user_message)
                 )
                 select locked_session.status as session_status,
                        selected.id, selected.session_id, selected.client_message_id,
                        selected.content, selected.content_hash, selected.status,
                        selected.created_at, selected.updated_at, selected.started_at,
                        selected.completed_at, selected.worker_id, selected.error,
                        selected.inserted
                   from locked_session
                   left join selected on true",
                &[
                    &session_key,
                    &session_id,
                    &turn_id,
                    &client_message_id,
                    &persisted_content,
                    &content_hash,
                    &now,
                ],
            )
            .await
            .map_err(|err| {
                if err.code() == Some(&SqlState::UNIQUE_VIOLATION) {
                    ServerError::Conflict(format!(
                        "client message id {client_message_id} was already used"
                    ))
                } else {
                    ServerError::Store(err.to_string())
                }
            })?
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        let session_status: String = row.get("session_status");
        if session_status == "ended" {
            return Err(ServerError::Conflict(format!(
                "session {session_id} has ended"
            )));
        }
        let selected_id: Option<Uuid> = row.get("id");
        if selected_id.is_none() {
            return Err(ServerError::Store(
                "turn enqueue produced no durable row".to_string(),
            ));
        }
        let existing_hash: String = row.get("content_hash");
        if existing_hash != content_hash {
            return Err(ServerError::Conflict(format!(
                "client message id {client_message_id} was already used with different content"
            )));
        }
        Ok(row_to_session_turn(row))
    }

    async fn turn(&self, turn_id: Uuid) -> Result<SessionTurnRecord> {
        let row = self
            .client
            .query_opt(
                "select id, session_id, client_message_id, content, content_hash, status,
                        created_at, updated_at, started_at, completed_at, worker_id, error
                   from session_turns
                  where id = $1",
                &[&turn_id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("turn {turn_id}")))?;
        Ok(row_to_session_turn(row))
    }

    async fn claim_next_turn(
        &self,
        worker_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<SessionTurnRecord>> {
        let row = match self
            .client
            .query_opt(
                "with candidate_session as (
                    select sessions.id, first_queued.created_at
                      from sessions
                      join lateral (
                          select created_at
                            from session_turns
                           where session_id = sessions.id and status = 'queued'
                           order by created_at asc, id asc
                           limit 1
                      ) first_queued on true
                     where sessions.status <> 'ended'
                       and not exists (
                           select 1 from session_turns running
                            where running.session_id = sessions.id and running.status = 'running'
                       )
                     order by first_queued.created_at asc, sessions.id asc
                     limit 1
                     for update of sessions skip locked
                 ), candidate_turn as (
                    select turns.id
                      from session_turns turns
                      join candidate_session on candidate_session.id = turns.session_id
                     where turns.status = 'queued'
                     order by turns.created_at asc, turns.id asc
                     limit 1
                     for update of turns skip locked
                 )
                 update session_turns
                    set status = 'running', updated_at = $2, started_at = $2,
                        completed_at = null, worker_id = $1, error = null
                  where id = (select id from candidate_turn) and status = 'queued'
                  returning id, session_id, client_message_id, content, content_hash, status,
                            created_at, updated_at, started_at, completed_at, worker_id, error",
                &[&worker_id, &now],
            )
            .await
        {
            Ok(row) => row,
            Err(err) if err.code() == Some(&SqlState::UNIQUE_VIOLATION) => return Ok(None),
            Err(err) => return Err(ServerError::Store(err.to_string())),
        };
        Ok(row.map(row_to_session_turn))
    }

    async fn fail_stale_running_turns(
        &self,
        stale_before: DateTime<Utc>,
        failed_at: DateTime<Utc>,
        error: &str,
    ) -> Result<usize> {
        let error = redact_persisted_text(error);
        let updated = self
            .client
            .execute(
                "update session_turns
                    set status = 'failed', updated_at = $2, completed_at = $2, error = $3
                  where status = 'running' and updated_at <= $1",
                &[&stale_before, &failed_at, &error],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(updated as usize)
    }

    async fn heartbeat_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<SessionTurnRecord> {
        let row = self
            .client
            .query_opt(
                "update session_turns
                    set updated_at = greatest(updated_at, $3)
                  where id = $1 and status = 'running' and worker_id = $2
                  returning id, session_id, client_message_id, content, content_hash, status,
                            created_at, updated_at, started_at, completed_at, worker_id, error",
                &[&turn_id, &worker_id, &now],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        if let Some(row) = row {
            return Ok(row_to_session_turn(row));
        }
        let turn = self.turn(turn_id).await?;
        Err(ServerError::Conflict(format!(
            "turn {} is not owned by worker {} (status {})",
            turn.id, worker_id, turn.status
        )))
    }

    async fn complete_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        assistant_content: &str,
        completed_at: DateTime<Utc>,
    ) -> Result<SessionTurnRecord> {
        let assistant_content = redact_persisted_text(assistant_content);
        let row = self
            .client
            .query_opt(
                "with turn_ref as (
                    select session_id from session_turns where id = $1
                 ), advisory_lock as (
                    select pg_advisory_xact_lock(hashtext(turn_ref.session_id::text))
                      from turn_ref
                 ), locked_turn as (
                    select turns.id, turns.session_id
                      from session_turns turns, advisory_lock
                     where turns.id = $1 and turns.status = 'running' and turns.worker_id = $2
                     for update
                 ), next_seq as (
                    select coalesce(max(messages.seq) + 1, 1)::bigint as seq
                      from messages, locked_turn
                     where messages.session_id = locked_turn.session_id
                 ), assistant_message as (
                    insert into messages(session_id, seq, role, content, turn_id, created_at)
                    select locked_turn.session_id, next_seq.seq, 'assistant', $3,
                           locked_turn.id, $4
                      from locked_turn, next_seq
                    returning session_id
                 ), updated as (
                    update session_turns
                       set status = 'completed', updated_at = $4, completed_at = $4, error = null
                     where id = $1 and status = 'running' and worker_id = $2
                       and exists (select 1 from assistant_message)
                    returning id, session_id, client_message_id, content, content_hash, status,
                              created_at, updated_at, started_at, completed_at, worker_id, error
                 ), touch as (
                    update sessions
                       set updated_at = $4
                     where id = (select session_id from assistant_message)
                 )
                 select * from updated",
                &[&turn_id, &worker_id, &assistant_content, &completed_at],
            )
            .await
            .map_err(|err| {
                if err.code() == Some(&SqlState::UNIQUE_VIOLATION) {
                    ServerError::Conflict(format!(
                        "turn {turn_id} already has an assistant message"
                    ))
                } else {
                    ServerError::Store(err.to_string())
                }
            })?;
        if let Some(row) = row {
            return Ok(row_to_session_turn(row));
        }
        let turn = self.turn(turn_id).await?;
        if turn.status == "completed" {
            return Ok(turn);
        }
        Err(ServerError::Conflict(format!(
            "turn {turn_id} is not owned by worker {worker_id}"
        )))
    }

    async fn fail_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        error: &str,
        failed_at: DateTime<Utc>,
    ) -> Result<SessionTurnRecord> {
        let error = redact_persisted_text(error);
        let row = self
            .client
            .query_opt(
                "update session_turns
                    set status = 'failed', updated_at = $4, completed_at = $4, error = $3
                  where id = $1 and status = 'running' and worker_id = $2
                  returning id, session_id, client_message_id, content, content_hash, status,
                            created_at, updated_at, started_at, completed_at, worker_id, error",
                &[&turn_id, &worker_id, &error, &failed_at],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        if let Some(row) = row {
            return Ok(row_to_session_turn(row));
        }
        let turn = self.turn(turn_id).await?;
        if turn.status == "failed" {
            return Ok(turn);
        }
        Err(ServerError::Conflict(format!(
            "turn {turn_id} is not owned by worker {worker_id}"
        )))
    }

    async fn create_approval_request(
        &self,
        mut request: NewApprovalRequest,
    ) -> Result<ApprovalRequestRecord> {
        validate_persistence_identifier("approval origin", &request.origin)?;
        validate_persistence_identifier("approval effect type", &request.effect_type)?;
        request.action = redact_persisted_text(&request.action);
        tm_memory::redact_json_value(&mut request.scope_json);
        tm_memory::redact_json_value(&mut request.options_json);
        tm_memory::redact_json_value(&mut request.effect_payload_json);
        if request.origin.trim().is_empty() || request.action.trim().is_empty() {
            return Err(ServerError::InvalidRequest(
                "approval origin and action must not be empty".to_string(),
            ));
        }
        if !request.options_json.is_array()
            || request.expires_at <= request.created_at
            || request.effect_type.trim().is_empty()
        {
            return Err(ServerError::InvalidRequest(
                "approval options or expiry are invalid".to_string(),
            ));
        }
        self.get_session(request.session_id).await?;
        if let Some(turn_id) = request.turn_id {
            let turn = self.turn(turn_id).await?;
            if turn.session_id != request.session_id {
                return Err(ServerError::NotFound(format!("turn {turn_id}")));
            }
        }
        let (audit_json, audit_key) = evolution_audit_params(evolution_audit_entry(
            &request.effect_payload_json,
            request.id,
            request.id,
            EvolutionAuditStatus::AwaitingApproval,
            request.created_at,
            None,
            format!("approval:{}:attempt", request.id),
        )?)?;
        let row = self
            .client
            .query_one(
                "with request as (
                    insert into approval_requests
                        (id, session_id, turn_id, requester_id, origin, action, scope_json,
                         options_json, status, resumable, created_at, expires_at, heartbeat_at,
                         resolved_at, selected_option_id, resolution_json, request_event_seq,
                         resolution_event_seq, resolution_version)
                    values ($1, $2, $3, $4, $5, $6, $7, $8, 'pending', $9, $10, $11, $10,
                            null, null, null, null, null, 0)
                    on conflict (id) do update set id = excluded.id
                    returning id, session_id, turn_id, requester_id, origin, action, scope_json,
                              options_json, status, resumable, created_at, expires_at, heartbeat_at,
                              resolved_at, selected_option_id, resolution_json, request_event_seq,
                              resolution_event_seq, resolution_version
                 ), effect as (
                    insert into approval_effects
                        (id, approval_id, session_id, effect_type, payload_json, status, attempts,
                         available_at, locked_at, lease_owner, lease_epoch, applied_at, error_at,
                         last_error, created_at, updated_at)
                    select request.id, request.id, request.session_id, $12, $13, 'blocked', 0,
                           request.expires_at, null, null, 0, null, null, null,
                           request.created_at, request.created_at
                      from request
                    on conflict (approval_id) do update
                       set id = approval_effects.id
                    returning id
                 ), audit as (
                    insert into evolution_audits(
                        id, idempotency_key, session_id, dream_id, actor_id, proposal_id,
                        target_class, target_id, content_digest, configured_tier, decision_json,
                        approval_id, effect_id, status, error_code, record_json, created_at
                    )
                    select ($14::jsonb ->> 'id')::uuid, $15,
                           ($14::jsonb #>> '{origin,sessionId}')::uuid,
                           nullif($14::jsonb #>> '{origin,dreamId}', '')::uuid,
                           $14::jsonb #>> '{origin,actorId}',
                           ($14::jsonb ->> 'proposalId')::uuid,
                           $14::jsonb #>> '{target,class}', $14::jsonb #>> '{target,id}',
                           $14::jsonb ->> 'contentDigest', $14::jsonb ->> 'configuredTier',
                           $14::jsonb -> 'decision',
                           nullif($14::jsonb ->> 'approvalId', '')::uuid,
                           nullif($14::jsonb ->> 'effectId', '')::uuid,
                           $14::jsonb ->> 'status', $14::jsonb ->> 'errorCode', $14,
                           ($14::jsonb ->> 'createdAt')::timestamptz
                      from effect where $14::jsonb is not null
                    on conflict (idempotency_key) do nothing
                 )
                 select * from request",
                &[
                    &request.id,
                    &request.session_id,
                    &request.turn_id,
                    &request.requester_id,
                    &request.origin,
                    &request.action,
                    &request.scope_json,
                    &request.options_json,
                    &request.resumable,
                    &request.created_at,
                    &request.expires_at,
                    &request.effect_type,
                    &request.effect_payload_json,
                    &audit_json,
                    &audit_key,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let record = row_to_approval_request(row);
        let effect = self
            .client
            .query_one(
                "select effect_type, payload_json from approval_effects where approval_id = $1",
                &[&request.id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let effect_type: String = effect.get("effect_type");
        let effect_payload_json: Value = effect.get("payload_json");
        if record.session_id != request.session_id
            || record.turn_id != request.turn_id
            || record.requester_id != request.requester_id
            || record.origin != request.origin
            || record.action != request.action
            || record.scope_json != request.scope_json
            || record.options_json != request.options_json
            || effect_type != request.effect_type
            || effect_payload_json != request.effect_payload_json
            || record.resumable != request.resumable
            || record.created_at.timestamp_micros() != request.created_at.timestamp_micros()
            || record.expires_at.timestamp_micros() != request.expires_at.timestamp_micros()
        {
            return Err(ServerError::Conflict(format!(
                "approval {} already exists with different content",
                request.id
            )));
        }
        Ok(record)
    }

    async fn approval_request(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
    ) -> Result<ApprovalRequestRecord> {
        let row = self
            .client
            .query_opt(
                "select id, session_id, turn_id, requester_id, origin, action, scope_json,
                        options_json, status, resumable, created_at, expires_at, heartbeat_at,
                        resolved_at, selected_option_id, resolution_json, request_event_seq,
                        resolution_event_seq, resolution_version
                   from approval_requests
                  where id = $1 and session_id = $2",
                &[&approval_id, &session_id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("approval {approval_id}")))?;
        Ok(row_to_approval_request(row))
    }

    async fn heartbeat_approval_request(
        &self,
        approval_id: Uuid,
        requester_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<ApprovalRequestRecord> {
        let row = self
            .client
            .query_opt(
                "update approval_requests
                    set heartbeat_at = $3
                  where id = $1 and requester_id = $2 and status = 'pending'
                  returning id, session_id, turn_id, requester_id, origin, action, scope_json,
                            options_json, status, resumable, created_at, expires_at, heartbeat_at,
                            resolved_at, selected_option_id, resolution_json, request_event_seq,
                            resolution_event_seq, resolution_version",
                &[&approval_id, &requester_id, &now],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        if let Some(row) = row {
            return Ok(row_to_approval_request(row));
        }
        let row = self
            .client
            .query_opt(
                "select id, session_id, turn_id, requester_id, origin, action, scope_json,
                        options_json, status, resumable, created_at, expires_at, heartbeat_at,
                        resolved_at, selected_option_id, resolution_json, request_event_seq,
                        resolution_event_seq, resolution_version
                   from approval_requests where id = $1",
                &[&approval_id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("approval {approval_id}")))?;
        let record = row_to_approval_request(row);
        if record.status == "pending" && record.requester_id != requester_id {
            return Err(ServerError::Conflict(format!(
                "approval {approval_id} belongs to a different requester"
            )));
        }
        Ok(record)
    }

    async fn resolve_approval_request(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        mut resolution: NewApprovalResolution,
    ) -> Result<ApprovalRequestRecord> {
        tm_memory::redact_json_value(&mut resolution.resolution_json);
        if !matches!(
            resolution.status.as_str(),
            "approved" | "denied" | "timed_out" | "cancelled"
        ) {
            return Err(ServerError::InvalidRequest(format!(
                "invalid terminal approval status {}",
                resolution.status
            )));
        }
        let effect = self
            .client
            .query_opt(
                "select id, payload_json from approval_effects where approval_id = $1",
                &[&approval_id],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        let audit = if let Some(effect) = effect {
            evolution_audit_entry(
                &effect.get::<_, Value>("payload_json"),
                approval_id,
                effect.get("id"),
                crate::evolution::audit_status_from_approval(&resolution.status)?,
                resolution.resolved_at,
                None,
                format!("approval:{approval_id}:resolution:{}", resolution.status),
            )?
        } else {
            None
        };
        let (audit_json, audit_key) = evolution_audit_params(audit)?;
        let row = self
            .client
            .query_opt(
                "with resolved as (
                    update approval_requests
                       set status = $3, resolved_at = $6, selected_option_id = $4,
                           resolution_json = $5, resolution_version = resolution_version + 1
                     where id = $1 and session_id = $2 and status = 'pending'
                    returning id, session_id, turn_id, requester_id, origin, action, scope_json,
                              options_json, status, resumable, created_at, expires_at, heartbeat_at,
                              resolved_at, selected_option_id, resolution_json, request_event_seq,
                              resolution_event_seq, resolution_version
                 ), effect as (
                    insert into approval_effects
                        (id, approval_id, session_id, effect_type, payload_json, status, attempts,
                         available_at, locked_at, lease_owner, lease_epoch, applied_at, error_at,
                         last_error, created_at, updated_at)
                    select resolved.id, resolved.id, resolved.session_id, 'approval_resolution',
                           jsonb_build_object('resolution', $5::jsonb), 'pending', 0, $6,
                           null, null, 0, null, null, null, $6, $6
                      from resolved
                    on conflict (approval_id) do update
                       set payload_json = approval_effects.payload_json
                           || jsonb_build_object('resolution', $5::jsonb),
                           status = 'pending', available_at = $6, locked_at = null,
                           lease_owner = null, updated_at = $6, error_at = null,
                           last_error = null
                    returning id
                 ), audit as (
                    insert into evolution_audits(
                        id, idempotency_key, session_id, dream_id, actor_id, proposal_id,
                        target_class, target_id, content_digest, configured_tier, decision_json,
                        approval_id, effect_id, status, error_code, record_json, created_at
                    )
                    select ($7::jsonb ->> 'id')::uuid, $8,
                           ($7::jsonb #>> '{origin,sessionId}')::uuid,
                           nullif($7::jsonb #>> '{origin,dreamId}', '')::uuid,
                           $7::jsonb #>> '{origin,actorId}',
                           ($7::jsonb ->> 'proposalId')::uuid,
                           $7::jsonb #>> '{target,class}', $7::jsonb #>> '{target,id}',
                           $7::jsonb ->> 'contentDigest', $7::jsonb ->> 'configuredTier',
                           $7::jsonb -> 'decision',
                           nullif($7::jsonb ->> 'approvalId', '')::uuid,
                           nullif($7::jsonb ->> 'effectId', '')::uuid,
                           $7::jsonb ->> 'status', $7::jsonb ->> 'errorCode', $7,
                           ($7::jsonb ->> 'createdAt')::timestamptz
                      from resolved, effect where $7::jsonb is not null
                    on conflict (idempotency_key) do nothing
                 )
                 select * from resolved",
                &[
                    &approval_id,
                    &session_id,
                    &resolution.status,
                    &resolution.selected_option_id,
                    &resolution.resolution_json,
                    &resolution.resolved_at,
                    &audit_json,
                    &audit_key,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        if let Some(row) = row {
            return Ok(row_to_approval_request(row));
        }
        self.approval_request(session_id, approval_id).await?;
        Err(ServerError::Conflict(format!(
            "approval {approval_id} is already resolved"
        )))
    }

    async fn resolve_approval_request_with_event(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        mut resolution: NewApprovalResolution,
    ) -> Result<(ApprovalRequestRecord, SessionEvent)> {
        tm_memory::redact_json_value(&mut resolution.resolution_json);
        if !matches!(
            resolution.status.as_str(),
            "approved" | "denied" | "timed_out" | "cancelled"
        ) {
            return Err(ServerError::InvalidRequest(format!(
                "invalid terminal approval status {}",
                resolution.status
            )));
        }
        let approval_id_text = approval_id.to_string();
        if resolution
            .resolution_json
            .get("approvalId")
            .and_then(Value::as_str)
            != Some(approval_id_text.as_str())
        {
            return Err(ServerError::InvalidRequest(
                "approval resolution event payload has a different approvalId".to_string(),
            ));
        }
        let effect = self
            .client
            .query_opt(
                "select id, payload_json from approval_effects where approval_id = $1",
                &[&approval_id],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        let audit = if let Some(effect) = effect {
            evolution_audit_entry(
                &effect.get::<_, Value>("payload_json"),
                approval_id,
                effect.get("id"),
                crate::evolution::audit_status_from_approval(&resolution.status)?,
                resolution.resolved_at,
                None,
                format!("approval:{approval_id}:resolution:{}", resolution.status),
            )?
        } else {
            None
        };
        let (audit_json, audit_key) = evolution_audit_params(audit)?;
        let session_key = session_id.to_string();
        let row = self
            .client
            .query_opt(
                "with event_lock as (
                    select pg_advisory_xact_lock(hashtext($7))
                 ), next_seq as (
                    select coalesce(max(session_events.seq) + 1, 1)::bigint as seq
                      from event_lock
                      left join session_events on session_events.session_id = $2
                 ), resolved as (
                    update approval_requests
                       set status = $3, resolved_at = $6, selected_option_id = $4,
                           resolution_json = $5, resolution_event_seq = next_seq.seq,
                           resolution_version = resolution_version + 1
                      from next_seq
                     where id = $1 and session_id = $2 and status = 'pending'
                    returning id, session_id, turn_id, requester_id, origin, action, scope_json,
                              options_json, status, resumable, created_at, expires_at, heartbeat_at,
                              resolved_at, selected_option_id, resolution_json, request_event_seq,
                              resolution_event_seq, resolution_version
                 ), effect as (
                    insert into approval_effects
                        (id, approval_id, session_id, effect_type, payload_json, status, attempts,
                         available_at, locked_at, lease_owner, lease_epoch, applied_at, error_at,
                         last_error, created_at, updated_at)
                    select resolved.id, resolved.id, resolved.session_id, 'approval_resolution',
                           jsonb_build_object('resolution', $5::jsonb), 'pending', 0, $6,
                           null, null, 0, null, null, null, $6, $6
                      from resolved
                    on conflict (approval_id) do update
                       set payload_json = approval_effects.payload_json
                           || jsonb_build_object('resolution', $5::jsonb),
                           status = 'pending', available_at = $6, locked_at = null,
                           lease_owner = null, updated_at = $6, error_at = null,
                           last_error = null
                    returning id
                 ), audit as (
                    insert into evolution_audits(
                        id, idempotency_key, session_id, dream_id, actor_id, proposal_id,
                        target_class, target_id, content_digest, configured_tier, decision_json,
                        approval_id, effect_id, status, error_code, record_json, created_at
                    )
                    select ($8::jsonb ->> 'id')::uuid, $9,
                           ($8::jsonb #>> '{origin,sessionId}')::uuid,
                           nullif($8::jsonb #>> '{origin,dreamId}', '')::uuid,
                           $8::jsonb #>> '{origin,actorId}',
                           ($8::jsonb ->> 'proposalId')::uuid,
                           $8::jsonb #>> '{target,class}', $8::jsonb #>> '{target,id}',
                           $8::jsonb ->> 'contentDigest', $8::jsonb ->> 'configuredTier',
                           $8::jsonb -> 'decision',
                           nullif($8::jsonb ->> 'approvalId', '')::uuid,
                           nullif($8::jsonb ->> 'effectId', '')::uuid,
                           $8::jsonb ->> 'status', $8::jsonb ->> 'errorCode', $8,
                           ($8::jsonb ->> 'createdAt')::timestamptz
                      from resolved, effect where $8::jsonb is not null
                    on conflict (idempotency_key) do nothing
                 ), inserted as (
                    insert into session_events
                        (session_id, seq, event_type, payload_json, actor_id, artifact_uri,
                         history_uri, turn_id, created_at)
                    select resolved.session_id, resolved.resolution_event_seq,
                           'approval_resolved', $5, null, null, null, resolved.turn_id, $6
                      from resolved
                    returning seq, created_at
                 ), touch as (
                    update sessions
                       set updated_at = inserted.created_at
                      from inserted
                     where sessions.id = $2
                 )
                 select resolved.*, inserted.created_at as event_created_at
                   from resolved, inserted",
                &[
                    &approval_id,
                    &session_id,
                    &resolution.status,
                    &resolution.selected_option_id,
                    &resolution.resolution_json,
                    &resolution.resolved_at,
                    &session_key,
                    &audit_json,
                    &audit_key,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let Some(row) = row else {
            self.approval_request(session_id, approval_id).await?;
            return Err(ServerError::Conflict(format!(
                "approval {approval_id} is already resolved"
            )));
        };
        let event_created_at: DateTime<Utc> = row.get("event_created_at");
        let record = row_to_approval_request(row);
        let mut event = SessionEvent::new(
            session_id,
            record
                .resolution_event_seq
                .expect("atomic resolution links its event"),
            "approval_resolved",
            resolution.resolution_json,
            event_created_at,
        );
        event.turn_id = record.turn_id;
        Ok((record, event))
    }

    async fn link_approval_event(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        event_type: &str,
        event_seq: i64,
    ) -> Result<ApprovalRequestRecord> {
        let column = match event_type {
            "approval" => "request_event_seq",
            "approval_resolved" => "resolution_event_seq",
            _ => {
                return Err(ServerError::InvalidRequest(format!(
                    "unsupported approval event type {event_type}"
                )));
            }
        };
        let approval_id_text = approval_id.to_string();
        let sql = format!(
            "update approval_requests
                set {column} = coalesce({column}, $4)
              where id = $1 and session_id = $2
                and ({column} is null or {column} = $4)
                and exists (
                    select 1 from session_events
                     where session_id = $2 and seq = $4 and event_type = $3
                       and payload_json ->> 'approvalId' = $5
                )
              returning id, session_id, turn_id, requester_id, origin, action, scope_json,
                        options_json, status, resumable, created_at, expires_at, heartbeat_at,
                        resolved_at, selected_option_id, resolution_json, request_event_seq,
                        resolution_event_seq, resolution_version"
        );
        let row = self
            .client
            .query_opt(
                &sql,
                &[
                    &approval_id,
                    &session_id,
                    &event_type,
                    &event_seq,
                    &approval_id_text,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| {
                ServerError::Conflict(format!(
                    "approval {approval_id} event linkage failed for {event_type}"
                ))
            })?;
        Ok(row_to_approval_request(row))
    }

    async fn cancel_stale_non_resumable_approvals(
        &self,
        stale_before: DateTime<Utc>,
        cancelled_at: DateTime<Utc>,
    ) -> Result<Vec<SessionEvent>> {
        let rows = self
            .client
            .query(
                "select id, session_id, origin
                   from approval_requests
                  where status = 'pending' and resumable = false and heartbeat_at <= $1
                  order by heartbeat_at asc, id asc",
                &[&stale_before],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let approval_id: Uuid = row.get("id");
            let session_id: Uuid = row.get("session_id");
            let origin: String = row.get("origin");
            match self
                .resolve_approval_request_with_event(
                    session_id,
                    approval_id,
                    NewApprovalResolution {
                        status: "cancelled".to_string(),
                        selected_option_id: None,
                        resolution_json: json!({
                            "approvalId": approval_id,
                            "backend": origin,
                            "status": "cancelled",
                            "outcome": "cancelled",
                            "reason": "stale_non_resumable",
                        }),
                        resolved_at: cancelled_at,
                    },
                )
                .await
            {
                Ok((_, event)) => events.push(event),
                Err(ServerError::Conflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(events)
    }

    async fn expire_pending_approvals(&self, now: DateTime<Utc>) -> Result<Vec<SessionEvent>> {
        let rows = self
            .client
            .query(
                "select id, session_id, origin
                   from approval_requests
                  where status = 'pending' and expires_at <= $1
                  order by expires_at asc, id asc",
                &[&now],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let approval_id: Uuid = row.get("id");
            let session_id: Uuid = row.get("session_id");
            let origin: String = row.get("origin");
            match self
                .resolve_approval_request_with_event(
                    session_id,
                    approval_id,
                    NewApprovalResolution {
                        status: "timed_out".to_string(),
                        selected_option_id: None,
                        resolution_json: json!({
                            "approvalId": approval_id,
                            "backend": origin,
                            "status": "timed_out",
                            "outcome": "cancelled",
                            "reason": "expired",
                        }),
                        resolved_at: now,
                    },
                )
                .await
            {
                Ok((_, event)) => events.push(event),
                Err(ServerError::Conflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(events)
    }

    async fn claim_approval_effect(
        &self,
        approval_id: Uuid,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<Option<ApprovalEffectLease>> {
        let stale_before = now - lease_timeout;
        let row = self
            .client
            .query_opt(
                "update approval_effects
                    set status = 'claimed', attempts = attempts + 1, locked_at = $3,
                        lease_owner = $2, lease_epoch = lease_epoch + 1, updated_at = $3,
                        error_at = null, last_error = null
                  where approval_id = $1
                    and ((status = 'pending' and available_at <= $3)
                      or (status = 'claimed' and locked_at <= $4))
                  returning id, approval_id, session_id, effect_type, payload_json, status,
                            attempts, available_at, locked_at, lease_owner, lease_epoch,
                            applied_at, error_at, last_error, created_at, updated_at",
                &[&approval_id, &owner_id, &now, &stale_before],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(row.map(|row| {
            let effect = row_to_approval_effect(row);
            ApprovalEffectLease {
                epoch: effect.lease_epoch,
                effect,
                owner_id,
            }
        }))
    }

    async fn claim_next_approval_effect(
        &self,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<Option<ApprovalEffectLease>> {
        let stale_before = now - lease_timeout;
        let row = self
            .client
            .query_opt(
                "update approval_effects
                    set status = 'claimed', attempts = attempts + 1, locked_at = $2,
                        lease_owner = $1, lease_epoch = lease_epoch + 1, updated_at = $2,
                        error_at = null, last_error = null
                  where id = (
                    select id
                      from approval_effects
                     where (status = 'pending' and available_at <= $2)
                        or (status = 'claimed' and locked_at <= $3)
                     order by case when status = 'claimed' then locked_at else available_at end asc,
                              created_at asc, id asc
                       for update skip locked
                     limit 1
                  )
                  returning id, approval_id, session_id, effect_type, payload_json, status,
                            attempts, available_at, locked_at, lease_owner, lease_epoch,
                            applied_at, error_at, last_error, created_at, updated_at",
                &[&owner_id, &now, &stale_before],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(row.map(|row| {
            let effect = row_to_approval_effect(row);
            ApprovalEffectLease {
                epoch: effect.lease_epoch,
                effect,
                owner_id,
            }
        }))
    }

    async fn heartbeat_approval_effect(
        &self,
        lease: &ApprovalEffectLease,
        now: DateTime<Utc>,
    ) -> Result<ApprovalEffectRecord> {
        let row = self
            .client
            .query_opt(
                "update approval_effects
                    set locked_at = $2, updated_at = $2
                  where id = $1 and status = 'claimed'
                    and lease_owner = $3 and lease_epoch = $4
                  returning id, approval_id, session_id, effect_type, payload_json, status,
                            attempts, available_at, locked_at, lease_owner, lease_epoch,
                            applied_at, error_at, last_error, created_at, updated_at",
                &[&lease.effect.id, &now, &lease.owner_id, &lease.epoch],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "active approval effect lease {} owner {} epoch {}",
                    lease.effect.id, lease.owner_id, lease.epoch
                ))
            })?;
        Ok(row_to_approval_effect(row))
    }

    async fn complete_approval_effect(
        &self,
        lease: &ApprovalEffectLease,
        applied_at: DateTime<Utc>,
    ) -> Result<ApprovalEffectRecord> {
        let row = self
            .client
            .query_opt(
                "update approval_effects
                    set status = 'applied', locked_at = null, lease_owner = null,
                        applied_at = $2, updated_at = $2, error_at = null, last_error = null
                  where id = $1
                    and effect_type in ('approval_continuation', 'approval_resolution')
                    and status = 'claimed' and lease_owner = $3 and lease_epoch = $4
                  returning id, approval_id, session_id, effect_type, payload_json, status,
                            attempts, available_at, locked_at, lease_owner, lease_epoch,
                            applied_at, error_at, last_error, created_at, updated_at",
                &[&lease.effect.id, &applied_at, &lease.owner_id, &lease.epoch],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "active approval effect lease {} owner {} epoch {}",
                    lease.effect.id, lease.owner_id, lease.epoch
                ))
            })?;
        Ok(row_to_approval_effect(row))
    }

    async fn complete_approval_effect_with_event(
        &self,
        lease: &ApprovalEffectLease,
        proposal_payload_json: Value,
        turn_id: Option<Uuid>,
        applied_at: DateTime<Utc>,
    ) -> Result<(ApprovalEffectRecord, SessionEvent)> {
        let proposal_payload_json = redact_persisted_json(proposal_payload_json);
        let (audit_json, audit_key) = evolution_audit_params(evolution_audit_entry(
            &lease.effect.payload_json,
            lease.effect.approval_id,
            lease.effect.id,
            EvolutionAuditStatus::Applied,
            applied_at,
            None,
            format!("effect:{}:applied", lease.effect.id),
        )?)?;
        let row = self
            .client
            .query_opt(
                "with leased as (
                    select id, session_id
                      from approval_effects
                     where id = $1
                       and effect_type in ('memory_write', 'skill_write', 'skill_rollback', 'mode_addendum_rollback', 'evolution_review')
                       and status = 'claimed'
                       and lease_owner = $2
                       and lease_epoch = $3
                     for update
                 ), session_lock as (
                    select pg_advisory_xact_lock(hashtext(session_id::text))
                      from leased
                 ), eligible as (
                    select leased.id, leased.session_id
                      from leased, session_lock
                     where $5::uuid is null
                        or exists (
                            select 1
                              from session_turns
                             where session_turns.id = $5
                               and session_turns.session_id = leased.session_id
                        )
                 ), next_event as (
                    select eligible.id, eligible.session_id,
                           coalesce(max(session_events.seq) + 1, 1)::bigint as seq
                      from eligible
                      left join session_events
                        on session_events.session_id = eligible.session_id
                     group by eligible.id, eligible.session_id
                 ), inserted_event as (
                    insert into session_events
                        (session_id, seq, event_type, payload_json, turn_id, created_at)
                    select session_id, seq, 'write_proposal', $6, $5, $4
                      from next_event
                    returning session_id, seq, created_at
                 ), updated_effect as (
                    update approval_effects effect
                       set status = 'applied', locked_at = null, lease_owner = null,
                           applied_at = $4, updated_at = $4, error_at = null,
                           last_error = null
                      from inserted_event
                     where effect.id = $1
                       and effect.session_id = inserted_event.session_id
                       and effect.status = 'claimed'
                       and effect.lease_owner = $2
                       and effect.lease_epoch = $3
                    returning effect.id, effect.approval_id, effect.session_id,
                              effect.effect_type, effect.payload_json, effect.status,
                              effect.attempts, effect.available_at, effect.locked_at,
                              effect.lease_owner, effect.lease_epoch, effect.applied_at,
                              effect.error_at, effect.last_error, effect.created_at,
                              effect.updated_at
                 ), audit as (
                    insert into evolution_audits(
                        id, idempotency_key, session_id, dream_id, actor_id, proposal_id,
                        target_class, target_id, content_digest, configured_tier, decision_json,
                        approval_id, effect_id, status, error_code, record_json, created_at
                    )
                    select ($7::jsonb ->> 'id')::uuid, $8,
                           ($7::jsonb #>> '{origin,sessionId}')::uuid,
                           nullif($7::jsonb #>> '{origin,dreamId}', '')::uuid,
                           $7::jsonb #>> '{origin,actorId}',
                           ($7::jsonb ->> 'proposalId')::uuid,
                           $7::jsonb #>> '{target,class}', $7::jsonb #>> '{target,id}',
                           $7::jsonb ->> 'contentDigest', $7::jsonb ->> 'configuredTier',
                           $7::jsonb -> 'decision',
                           nullif($7::jsonb ->> 'approvalId', '')::uuid,
                           nullif($7::jsonb ->> 'effectId', '')::uuid,
                           $7::jsonb ->> 'status', $7::jsonb ->> 'errorCode', $7,
                           ($7::jsonb ->> 'createdAt')::timestamptz
                      from updated_effect where $7::jsonb is not null
                    on conflict (idempotency_key) do nothing
                 ), touch as (
                    update sessions
                       set updated_at = $4
                     where id = (select session_id from inserted_event)
                 )
                 select updated_effect.*, inserted_event.seq as event_seq,
                        inserted_event.created_at as event_created_at
                   from updated_effect
                   join inserted_event
                     on inserted_event.session_id = updated_effect.session_id",
                &[
                    &lease.effect.id,
                    &lease.owner_id,
                    &lease.epoch,
                    &applied_at,
                    &turn_id,
                    &proposal_payload_json,
                    &audit_json,
                    &audit_key,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "active durable proposal effect lease {} owner {} epoch {}",
                    lease.effect.id, lease.owner_id, lease.epoch
                ))
            })?;
        let event_seq = row.get("event_seq");
        let event_created_at = row.get("event_created_at");
        let effect = row_to_approval_effect(row);
        let mut event = SessionEvent::new(
            effect.session_id,
            event_seq,
            "write_proposal",
            proposal_payload_json,
            event_created_at,
        );
        event.turn_id = turn_id;
        Ok((effect, event))
    }

    async fn fail_approval_effect(
        &self,
        lease: &ApprovalEffectLease,
        error: &str,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<ApprovalEffectRecord> {
        let error = redact_persisted_text(error);
        let failed_at = Utc::now();
        let (audit_json, audit_key) = evolution_audit_params(evolution_audit_entry(
            &lease.effect.payload_json,
            lease.effect.approval_id,
            lease.effect.id,
            EvolutionAuditStatus::Failed,
            failed_at,
            crate::evolution::evolution_policy_error_code(&error),
            format!("effect:{}:failure:{}", lease.effect.id, lease.epoch),
        )?)?;
        let row = self
            .client
            .query_opt(
                "with updated_effect as (
                    update approval_effects
                       set status = case when attempts >= $4 then 'failed' else 'pending' end,
                           available_at = $3, locked_at = null, lease_owner = null,
                           updated_at = $7, error_at = $7, last_error = $2
                     where id = $1 and status = 'claimed'
                       and lease_owner = $5 and lease_epoch = $6
                    returning id, approval_id, session_id, effect_type, payload_json, status,
                              attempts, available_at, locked_at, lease_owner, lease_epoch,
                              applied_at, error_at, last_error, created_at, updated_at
                 ), audit as (
                    insert into evolution_audits(
                        id, idempotency_key, session_id, dream_id, actor_id, proposal_id,
                        target_class, target_id, content_digest, configured_tier, decision_json,
                        approval_id, effect_id, status, error_code, record_json, created_at
                    )
                    select ($8::jsonb ->> 'id')::uuid, $9,
                           ($8::jsonb #>> '{origin,sessionId}')::uuid,
                           nullif($8::jsonb #>> '{origin,dreamId}', '')::uuid,
                           $8::jsonb #>> '{origin,actorId}',
                           ($8::jsonb ->> 'proposalId')::uuid,
                           $8::jsonb #>> '{target,class}', $8::jsonb #>> '{target,id}',
                           $8::jsonb ->> 'contentDigest', $8::jsonb ->> 'configuredTier',
                           $8::jsonb -> 'decision',
                           nullif($8::jsonb ->> 'approvalId', '')::uuid,
                           nullif($8::jsonb ->> 'effectId', '')::uuid,
                           $8::jsonb ->> 'status', $8::jsonb ->> 'errorCode', $8,
                           ($8::jsonb ->> 'createdAt')::timestamptz
                      from updated_effect where $8::jsonb is not null
                    on conflict (idempotency_key) do nothing
                 )
                 select * from updated_effect",
                &[
                    &lease.effect.id,
                    &error,
                    &next_available_at,
                    &max_attempts,
                    &lease.owner_id,
                    &lease.epoch,
                    &failed_at,
                    &audit_json,
                    &audit_key,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "active approval effect lease {} owner {} epoch {}",
                    lease.effect.id, lease.owner_id, lease.epoch
                ))
            })?;
        Ok(row_to_approval_effect(row))
    }

    async fn append_evolution_audit(
        &self,
        entry: EvolutionAuditEntry,
    ) -> Result<EvolutionAuditRecord> {
        let idempotency_key = entry.idempotency_key;
        let record_json = serde_json::to_value(entry.record)
            .map_err(|error| ServerError::Store(error.to_string()))?;
        let row = self
            .client
            .query_one(
                "insert into evolution_audits(
                    id, idempotency_key, session_id, dream_id, actor_id, proposal_id,
                    target_class, target_id, content_digest, configured_tier, decision_json,
                    approval_id, effect_id, status, error_code, record_json, created_at
                 )
                 values (
                    ($1::jsonb ->> 'id')::uuid, $2,
                    ($1::jsonb #>> '{origin,sessionId}')::uuid,
                    nullif($1::jsonb #>> '{origin,dreamId}', '')::uuid,
                    $1::jsonb #>> '{origin,actorId}',
                    ($1::jsonb ->> 'proposalId')::uuid,
                    $1::jsonb #>> '{target,class}', $1::jsonb #>> '{target,id}',
                    $1::jsonb ->> 'contentDigest', $1::jsonb ->> 'configuredTier',
                    $1::jsonb -> 'decision',
                    nullif($1::jsonb ->> 'approvalId', '')::uuid,
                    nullif($1::jsonb ->> 'effectId', '')::uuid,
                    $1::jsonb ->> 'status', $1::jsonb ->> 'errorCode', $1,
                    ($1::jsonb ->> 'createdAt')::timestamptz
                 )
                 on conflict (idempotency_key) do update
                    set idempotency_key = excluded.idempotency_key
                 returning record_json",
                &[&record_json, &idempotency_key],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        row_to_evolution_audit(row)
    }

    async fn evolution_audits(&self, session_id: Uuid) -> Result<Vec<EvolutionAuditRecord>> {
        self.get_session(session_id).await?;
        self.client
            .query(
                "select record_json from evolution_audits
                  where session_id = $1 order by audit_seq",
                &[&session_id],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?
            .into_iter()
            .map(row_to_evolution_audit)
            .collect()
    }

    async fn evolution_memory_proposal(
        &self,
        proposal_id: Uuid,
    ) -> Result<crate::MemoryWriteProposal> {
        let proposal_id = proposal_id.to_string();
        let row = self
            .client
            .query_opt(
                "select payload_json -> 'proposal' as proposal
                   from approval_effects
                  where payload_json #>> '{proposal,proposalId}' = $1
                  order by created_at desc limit 1",
                &[&proposal_id],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("evolution proposal {proposal_id}")))?;
        serde_json::from_value(row.get("proposal"))
            .map_err(|error| ServerError::Store(error.to_string()))
    }

    async fn create_evolution_review_proposal(
        &self,
        proposal: NewEvolutionReviewProposal,
    ) -> Result<EvolutionReviewProposalRecord> {
        let proposal = sanitize_evolution_review_proposal_persistence(proposal)?;
        self.get_session(proposal.session_id).await?;
        let now = Utc::now();
        let record = EvolutionReviewProposalRecord {
            id: proposal.id,
            session_id: proposal.session_id,
            target: proposal.target,
            base_version: proposal.base_version,
            base_digest: proposal.base_digest,
            base_active_digest: proposal.base_active_digest,
            changes: proposal.changes,
            content_digest: proposal.content_digest,
            status: ReviewProposalStatus::Pending,
            apply_contract: proposal.apply_contract,
            created_at: now,
            updated_at: now,
        };
        let target_class = match &record.target {
            ReviewProposalTarget::Persona { .. } => "persona_proposal",
            ReviewProposalTarget::Mode { .. } => "mode_proposal",
        };
        let target_id = record.target.id();
        let base_version = i64::try_from(record.base_version).map_err(|_| {
            ServerError::InvalidRequest("review proposal base version is too large".to_string())
        })?;
        let record_json = serde_json::to_value(&record)?;
        let row = self
            .client
            .query_one(
                "insert into evolution_review_proposals(
                id, session_id, target_class, target_id, status, base_version, base_digest,
                content_digest, record_json, created_at, updated_at
             ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)
             on conflict (id) do update set id = excluded.id
             returning record_json",
                &[
                    &record.id,
                    &record.session_id,
                    &target_class,
                    &target_id,
                    &record.status.as_str(),
                    &base_version,
                    &record.base_digest,
                    &record.content_digest,
                    &record_json,
                    &now,
                ],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        let stored = row_to_evolution_review_proposal(row)?;
        if stored != record {
            return Err(ServerError::Conflict(format!(
                "evolution review proposal {} already exists with different content",
                record.id
            )));
        }
        Ok(stored)
    }

    async fn evolution_review_proposal(&self, id: Uuid) -> Result<EvolutionReviewProposalRecord> {
        self.client
            .query_opt(
                "select record_json from evolution_review_proposals where id = $1",
                &[&id],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?
            .map(row_to_evolution_review_proposal)
            .transpose()?
            .ok_or_else(|| ServerError::NotFound(format!("evolution review proposal {id}")))
    }

    async fn evolution_review_proposals_for_session(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<EvolutionReviewProposalRecord>> {
        self.get_session(session_id).await?;
        self.client
            .query(
                "select record_json from evolution_review_proposals
                  where session_id = $1 order by created_at, id",
                &[&session_id],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?
            .into_iter()
            .map(row_to_evolution_review_proposal)
            .collect()
    }

    async fn update_evolution_review_proposal_status(
        &self,
        id: Uuid,
        status: ReviewProposalStatus,
    ) -> Result<EvolutionReviewProposalRecord> {
        let mut record = self.evolution_review_proposal(id).await?;
        record.status = status;
        record.updated_at = Utc::now();
        let record_json = serde_json::to_value(&record)?;
        let row = self
            .client
            .query_opt(
                "update evolution_review_proposals
                set status = $2, record_json = $3, updated_at = $4
              where id = $1 returning record_json",
                &[&id, &status.as_str(), &record_json, &record.updated_at],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("evolution review proposal {id}")))?;
        row_to_evolution_review_proposal(row)
    }

    async fn add_profile_fact(&self, fact: ProfileFactRecord) -> Result<()> {
        validate_profile_fact_persistence(&fact)?;
        self.client
            .execute(
                "insert into profile_facts (id, subject, predicate, object, confidence, importance, provenance, valid_from, valid_to) values ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                &[&fact.id, &fact.subject, &fact.predicate, &fact.object, &fact.confidence, &fact.importance, &fact.provenance, &fact.valid_from, &fact.valid_to],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(())
    }

    async fn add_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<()> {
        validate_recall_chunk_persistence(&chunk)?;
        self.client
            .execute(
                "insert into recall_chunks (id, scope, text, source, importance, created_at) values ($1, $2, $3, $4, $5, $6)",
                &[&chunk.id, &chunk.scope, &chunk.text, &chunk.source, &chunk.importance, &chunk.created_at],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(())
    }

    async fn upsert_profile_fact(&self, fact: ProfileFactRecord) -> Result<ProfileFactRecord> {
        validate_profile_fact_persistence(&fact)?;
        let row = self
            .client
            .query_one(
                "with superseded as (
                   update profile_facts
                      set valid_to = $8
                    where subject = $2
                      and predicate = $3
                      and object <> $4
                      and id <> $1
                      and valid_to is null
                 )
                 insert into profile_facts (id, subject, predicate, object, confidence, importance, provenance, valid_from, valid_to)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 on conflict (id) do update
                   set subject = excluded.subject,
                       predicate = excluded.predicate,
                       object = excluded.object,
                       confidence = excluded.confidence,
                       importance = excluded.importance,
                       provenance = excluded.provenance,
                       valid_from = excluded.valid_from,
                       valid_to = coalesce(profile_facts.valid_to, excluded.valid_to)
                 returning id, subject, predicate, object, confidence, importance, provenance, valid_from, valid_to",
                &[&fact.id, &fact.subject, &fact.predicate, &fact.object, &fact.confidence, &fact.importance, &fact.provenance, &fact.valid_from, &fact.valid_to],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(ProfileFactRecord {
            id: row.get("id"),
            subject: row.get("subject"),
            predicate: row.get("predicate"),
            object: row.get("object"),
            confidence: row.get("confidence"),
            importance: row.get("importance"),
            provenance: row.get("provenance"),
            valid_from: row.get("valid_from"),
            valid_to: row.get("valid_to"),
        })
    }

    async fn upsert_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<RecallChunkRecord> {
        validate_recall_chunk_persistence(&chunk)?;
        let row = self
            .client
            .query_one(
                "insert into recall_chunks (id, scope, text, source, importance, created_at)
                 values ($1, $2, $3, $4, $5, $6)
                 on conflict (id) do update
                   set scope = excluded.scope,
                       text = excluded.text,
                       source = excluded.source,
                       importance = excluded.importance,
                       created_at = excluded.created_at
                 returning id, scope, text, source, importance, created_at",
                &[
                    &chunk.id,
                    &chunk.scope,
                    &chunk.text,
                    &chunk.source,
                    &chunk.importance,
                    &chunk.created_at,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(RecallChunkRecord {
            id: row.get("id"),
            scope: row.get("scope"),
            text: row.get("text"),
            source: row.get("source"),
            importance: row.get("importance"),
            created_at: row.get("created_at"),
        })
    }

    async fn apply_approved_memory_proposal(
        &self,
        proposal: &crate::MemoryWriteProposal,
    ) -> Result<crate::MemoryRecordRef> {
        let mut client = self.memory_mutation_client.lock().await;
        let transaction = client.transaction().await.map_err(postgres_memory_error)?;

        match proposal.memory_kind {
            crate::MemoryWriteKind::ProfileFact => {
                let fact = crate::memory::profile_fact_record(proposal)?;
                validate_profile_fact_persistence(&fact)?;
                transaction
                    .query_one(
                        "select pg_advisory_xact_lock(hashtextextended($1, 0))",
                        &[&format!("{}\n{}", fact.subject, fact.predicate)],
                    )
                    .await
                    .map_err(postgres_memory_error)?;

                let rows = transaction
                    .query(
                        &format!(
                            "select {DURABLE_MEMORY_RECORD_COLUMNS}
                               from memory_records
                              where record_kind = 'semantic'
                                and owner_subject = $1 and memory_scope = 'global'
                                and semantic_subject = $1
                                and predicate = $2 and object <> $3
                                and status = 'active' and effective_to is null
                              order by effective_from desc, id desc
                              for update"
                        ),
                        &[&fact.subject, &fact.predicate, &fact.object],
                    )
                    .await
                    .map_err(postgres_memory_error)?;
                let mut predecessors = rows
                    .into_iter()
                    .map(row_to_stored_memory_record)
                    .collect::<Result<Vec<_>>>()?;
                let existing_forward = transaction
                    .query_opt(
                        "select supersedes_record_id
                           from memory_records
                          where record_kind = 'semantic' and id = $1
                            and owner_subject = $2 and memory_scope = 'global'
                          for update",
                        &[&proposal.record_id, &proposal.subject],
                    )
                    .await
                    .map_err(postgres_memory_error)?
                    .and_then(|row| row.get("supersedes_record_id"));
                let forward = predecessors
                    .first()
                    .map(StoredMemoryRecord::id)
                    .or(existing_forward);

                transaction
                    .query_one(
                        "with superseded as (
                           update profile_facts
                              set valid_to = $8
                            where subject = $2 and predicate = $3 and object <> $4
                              and id <> $1 and valid_to is null
                         )
                         insert into profile_facts(
                            id, subject, predicate, object, confidence, importance, provenance,
                            valid_from, valid_to
                         ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                         on conflict (id) do update set
                            subject = excluded.subject,
                            predicate = excluded.predicate,
                            object = excluded.object,
                            confidence = excluded.confidence,
                            importance = excluded.importance,
                            provenance = excluded.provenance,
                            valid_from = excluded.valid_from,
                            valid_to = coalesce(profile_facts.valid_to, excluded.valid_to)
                         returning id",
                        &[
                            &fact.id,
                            &fact.subject,
                            &fact.predicate,
                            &fact.object,
                            &fact.confidence,
                            &fact.importance,
                            &fact.provenance,
                            &fact.valid_from,
                            &fact.valid_to,
                        ],
                    )
                    .await
                    .map_err(postgres_memory_error)?;

                let typed = crate::memory::typed_memory_record(proposal, forward)?;
                upsert_typed_memory_record(&transaction, &typed).await?;
                for previous in &mut predecessors {
                    let MemoryRecordResource::Semantic(value) = &mut previous.resource else {
                        return Err(ServerError::Store(
                            "profile predecessor is not semantic".to_string(),
                        ));
                    };
                    value.status = tm_memory::MemoryRecordStatus::Superseded;
                    value.effective_to = Some(proposal.created_at);
                    value.links.superseded_by_record_id = Some(proposal.record_id);
                    *previous = StoredMemoryRecord::new(previous.resource.clone())
                        .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
                    upsert_typed_memory_record(&transaction, previous).await?;
                }
            }
            crate::MemoryWriteKind::RecallChunk => {
                let chunk = crate::memory::recall_chunk_record(proposal)?;
                validate_recall_chunk_persistence(&chunk)?;
                transaction
                    .query_one(
                        "insert into recall_chunks(id, scope, text, source, importance, created_at)
                         values ($1, $2, $3, $4, $5, $6)
                         on conflict (id) do update set
                            scope = excluded.scope,
                            text = excluded.text,
                            source = excluded.source,
                            importance = excluded.importance,
                            created_at = excluded.created_at
                         returning id",
                        &[
                            &chunk.id,
                            &chunk.scope,
                            &chunk.text,
                            &chunk.source,
                            &chunk.importance,
                            &chunk.created_at,
                        ],
                    )
                    .await
                    .map_err(postgres_memory_error)?;
                let typed = crate::memory::typed_memory_record(proposal, None)?;
                upsert_typed_memory_record(&transaction, &typed).await?;
            }
        }

        transaction.commit().await.map_err(postgres_memory_error)?;
        Ok(proposal.record_ref())
    }

    async fn profile_facts(&self, subject: &str) -> Result<Vec<ProfileFactRecord>> {
        let rows = self.client
            .query("select id, subject, predicate, object, confidence, importance, provenance, valid_from, valid_to from profile_facts where subject = $1 and valid_to is null order by importance desc, valid_from desc", &[&subject])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| ProfileFactRecord {
                id: row.get("id"),
                subject: row.get("subject"),
                predicate: row.get("predicate"),
                object: row.get("object"),
                confidence: row.get("confidence"),
                importance: row.get("importance"),
                provenance: row.get("provenance"),
                valid_from: row.get("valid_from"),
                valid_to: row.get("valid_to"),
            })
            .collect())
    }

    async fn profile_fact(&self, subject: &str, id: Uuid) -> Result<ProfileFactRecord> {
        let row = self.client
            .query_opt("select id, subject, predicate, object, confidence, importance, provenance, valid_from, valid_to from profile_facts where subject = $1 and id = $2", &[&subject, &id])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("profile fact {subject}/{id}")))?;
        Ok(ProfileFactRecord {
            id: row.get("id"),
            subject: row.get("subject"),
            predicate: row.get("predicate"),
            object: row.get("object"),
            confidence: row.get("confidence"),
            importance: row.get("importance"),
            provenance: row.get("provenance"),
            valid_from: row.get("valid_from"),
            valid_to: row.get("valid_to"),
        })
    }

    async fn recall_chunks(
        &self,
        scope: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecallChunkRecord>> {
        let pattern = format!("%{query}%");
        let rows = self
            .client
            .query(
                "select id, scope, text, source, importance, created_at
                   from recall_chunks
                  where scope = $1
                    and (
                      text ilike $2
                      or to_tsvector('simple', text) @@ plainto_tsquery('simple', $3)
                    )
                  order by ts_rank_cd(to_tsvector('simple', text), plainto_tsquery('simple', $3)) desc,
                           importance desc,
                           created_at desc
                  limit $4",
                &[&scope, &pattern, &query, &(limit as i64)],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| RecallChunkRecord {
                id: row.get("id"),
                scope: row.get("scope"),
                text: row.get("text"),
                source: row.get("source"),
                importance: row.get("importance"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    async fn recall_chunk(&self, scope: &str, id: Uuid) -> Result<RecallChunkRecord> {
        let row = self
            .client
            .query_opt(
                "select id, scope, text, source, importance, created_at from recall_chunks where scope = $1 and id = $2",
                &[&scope, &id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("recall chunk {scope}/{id}")))?;
        Ok(RecallChunkRecord {
            id: row.get("id"),
            scope: row.get("scope"),
            text: row.get("text"),
            source: row.get("source"),
            importance: row.get("importance"),
            created_at: row.get("created_at"),
        })
    }

    async fn upsert_memory_record(&self, record: StoredMemoryRecord) -> Result<StoredMemoryRecord> {
        let record = sanitize_durable_memory_record(record)?;
        self.ensure_memory_scope_is_readable(
            record.resource.owner_subject(),
            record.resource.memory_scope(),
        )
        .await?;
        self.ensure_memory_record_links_are_scoped(&record).await?;
        if let Some(existing) = self.memory_record_with_content_key(&record).await?
            && existing.id() != record.id()
        {
            return Ok(existing);
        }

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
        let evidence_json = serde_json::to_value(evidence)
            .map_err(|error| ServerError::Store(error.to_string()))?;
        let schema_version = i32::from(schema_version);
        let kind = record.kind().as_str();
        let inserted = self
            .client
            .query_one(
                &format!(
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
                     )
                     on conflict (record_kind, id) do update set
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
                        version_key = excluded.version_key
                     returning {DURABLE_MEMORY_RECORD_COLUMNS}"
                ),
                &[
                    &kind,
                    &record.id(),
                    &schema_version,
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
            .await;
        match inserted {
            Ok(row) => row_to_stored_memory_record(row),
            Err(error) if error.code() == Some(&SqlState::UNIQUE_VIOLATION) => self
                .memory_record_with_content_key(&record)
                .await?
                .ok_or_else(|| ServerError::Store(error.to_string())),
            Err(error) => Err(postgres_memory_error(error)),
        }
    }

    async fn memory_record(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        kind: MemoryRecordKind,
        id: Uuid,
    ) -> Result<StoredMemoryRecord> {
        self.ensure_memory_scope_is_readable(owner_subject, memory_scope)
            .await?;
        let row = self
            .client
            .query_opt(
                &format!(
                    "select {DURABLE_MEMORY_RECORD_COLUMNS}
                       from memory_records
                      where record_kind = $1 and id = $2
                        and owner_subject = $3 and memory_scope = $4"
                ),
                &[&kind.as_str(), &id, &owner_subject, &memory_scope],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "memory record {owner_subject}/{memory_scope}/{}/{}",
                    kind.as_str(),
                    id
                ))
            })?;
        row_to_stored_memory_record(row)
    }

    async fn active_memory_records(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        limit: usize,
    ) -> Result<Vec<StoredMemoryRecord>> {
        self.ensure_memory_scope_is_readable(owner_subject, memory_scope)
            .await?;
        let rows = self
            .client
            .query(
                &format!(
                    "select {DURABLE_MEMORY_RECORD_COLUMNS}
                       from memory_records record
                      where record.owner_subject = $1
                        and record.memory_scope = $2
                        and record.status = 'active'
                        and record.effective_to is null
                        and not exists ({ACTIVE_MEMORY_SUCCESSOR_QUERY})
                      order by record.importance desc, record.observed_at desc,
                               record.record_kind asc, record.id asc
                      limit $3"
                ),
                &[&owner_subject, &memory_scope, &(limit as i64)],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        rows.into_iter().map(row_to_stored_memory_record).collect()
    }

    async fn active_memory_embedding_generation(
        &self,
        owner_subject: &str,
        memory_scope: &str,
    ) -> Result<Option<tm_memory::MemoryEmbeddingGeneration>> {
        PostgresStore::active_memory_embedding_generation(self, owner_subject, memory_scope).await
    }

    async fn memory_hybrid_candidates(
        &self,
        request: &tm_memory::HybridRecallRequest,
        query: &str,
        dense_query: Option<&tm_memory::DenseRecallQuery>,
    ) -> Result<tm_memory::HybridRecallResult> {
        PostgresStore::memory_hybrid_candidates(self, request, query, dense_query).await
    }

    async fn enqueue_memory_embedding_job(
        &self,
        job: NewMemoryEmbeddingJob,
    ) -> Result<MemoryEmbeddingJobRecord> {
        let job = sanitize_new_memory_embedding_job(job)?;
        let record = self
            .memory_record(
                &job.owner_subject,
                &job.memory_scope,
                job.record_kind,
                job.record_id,
            )
            .await?;
        if record.content_key != job.content_key {
            return Err(ServerError::NotFound(format!(
                "memory record {}/{} at requested content version",
                job.record_kind.as_str(),
                job.record_id
            )));
        }
        let dimensions = i32::try_from(job.provenance.dimensions).map_err(|_| {
            ServerError::InvalidRequest(
                "embedding dimensions exceed Postgres integer range".to_string(),
            )
        })?;
        let input_limit_bytes = i32::try_from(job.input_limit_bytes).map_err(|_| {
            ServerError::InvalidRequest("embedding input limit exceeds Postgres range".to_string())
        })?;
        let record_kind = job.record_kind.as_str();
        let embedding_version = &job.provenance.embedding_version;
        let mut client = self.memory_mutation_client.lock().await;
        let transaction = client.transaction().await.map_err(postgres_memory_error)?;
        transaction
            .execute(
                "insert into memory_embedding_provenance(
                    record_kind, record_id, embedding_version, schema_version, provider,
                    model_id, dimensions, normalization, content_hash, reembedding_state, created_at
                 ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                 on conflict (record_kind, record_id, embedding_version) do update set
                    schema_version = excluded.schema_version,
                    provider = excluded.provider,
                    model_id = excluded.model_id,
                    dimensions = excluded.dimensions,
                    normalization = excluded.normalization,
                    content_hash = excluded.content_hash,
                    reembedding_state = excluded.reembedding_state,
                    created_at = excluded.created_at",
                &[
                    &record_kind,
                    &job.record_id,
                    &embedding_version,
                    &i32::from(job.provenance.schema_version),
                    &job.provenance.provider.as_str(),
                    &job.provenance.model_id,
                    &dimensions,
                    &job.provenance.normalization.as_str(),
                    &job.provenance.content_hash,
                    &job.provenance.reembedding_state.as_str(),
                    &job.provenance.created_at,
                ],
            )
            .await
            .map_err(postgres_memory_error)?;
        let reembedding_key = job.reembedding_key();
        transaction
            .query_one(
                "insert into memory_embedding_jobs(
                    id, record_kind, record_id, owner_subject, memory_scope, content_key,
                    embedding_version, reembedding_key, status, attempts, available_at, locked_at,
                    cancelled_at, created_at, updated_at, input_limit_bytes, failure_code
                 ) values ($1, $2, $3, $4, $5, $6, $7, $8, 'queued', 0, $9, null, null, $9, $9, $10, null)
                 on conflict (reembedding_key) do update
                    set status = case
                            when memory_embedding_jobs.status = 'completed' then 'queued'
                            when memory_embedding_jobs.status = 'failed'
                              and (memory_embedding_jobs.failure_code is distinct from 'input_too_large'
                                   or excluded.input_limit_bytes > memory_embedding_jobs.input_limit_bytes)
                              then 'queued'
                            else memory_embedding_jobs.status
                        end,
                        input_limit_bytes = greatest(memory_embedding_jobs.input_limit_bytes,
                            excluded.input_limit_bytes),
                        available_at = case
                            when memory_embedding_jobs.status = 'completed'
                              or (memory_embedding_jobs.status = 'failed'
                                  and (memory_embedding_jobs.failure_code is distinct from 'input_too_large'
                                       or excluded.input_limit_bytes > memory_embedding_jobs.input_limit_bytes))
                            then excluded.available_at else memory_embedding_jobs.available_at end,
                        failure_code = case
                            when memory_embedding_jobs.status = 'completed'
                              or (memory_embedding_jobs.status = 'failed'
                                  and (memory_embedding_jobs.failure_code is distinct from 'input_too_large'
                                       or excluded.input_limit_bytes > memory_embedding_jobs.input_limit_bytes))
                            then null else memory_embedding_jobs.failure_code end,
                        updated_at = case
                            when memory_embedding_jobs.status = 'completed'
                              or (memory_embedding_jobs.status = 'failed'
                                  and (memory_embedding_jobs.failure_code is distinct from 'input_too_large'
                                       or excluded.input_limit_bytes > memory_embedding_jobs.input_limit_bytes))
                            then excluded.updated_at else memory_embedding_jobs.updated_at end
                 returning id",
                &[
                    &job.id,
                    &record_kind,
                    &job.record_id,
                    &job.owner_subject,
                    &job.memory_scope,
                    &job.content_key,
                    &embedding_version,
                    &reembedding_key,
                    &job.created_at,
                    &input_limit_bytes,
                ],
            )
            .await
            .map_err(postgres_memory_error)?;
        let row = transaction
            .query_one(
                &format!(
                    "select {MEMORY_EMBEDDING_JOB_COLUMNS}
                       from memory_embedding_jobs j
                       join memory_embedding_provenance p
                         on p.record_kind = j.record_kind
                        and p.record_id = j.record_id
                        and p.embedding_version = j.embedding_version
                      where j.reembedding_key = $1"
                ),
                &[&reembedding_key],
            )
            .await
            .map_err(postgres_memory_error)?;
        let record = row_to_memory_embedding_job(row)?;
        transaction.commit().await.map_err(postgres_memory_error)?;
        Ok(record)
    }

    async fn memory_embedding_jobs(
        &self,
        owner_subject: &str,
        memory_scope: &str,
    ) -> Result<Vec<MemoryEmbeddingJobRecord>> {
        self.ensure_memory_scope_is_readable(owner_subject, memory_scope)
            .await?;
        let rows = self
            .client
            .query(
                &format!(
                    "select {MEMORY_EMBEDDING_JOB_COLUMNS}
                       from memory_embedding_jobs j
                       join memory_embedding_provenance p
                         on p.record_kind = j.record_kind
                        and p.record_id = j.record_id
                        and p.embedding_version = j.embedding_version
                      where j.owner_subject = $1 and j.memory_scope = $2
                      order by j.created_at asc, j.id asc"
                ),
                &[&owner_subject, &memory_scope],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        rows.into_iter().map(row_to_memory_embedding_job).collect()
    }

    async fn revoke_memory_scope(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        reason: &str,
    ) -> Result<MemoryScopeTombstone> {
        validate_persistence_identifier("memory tombstone owner subject", owner_subject)?;
        validate_persistence_identifier("memory tombstone scope", memory_scope)?;
        if memory_scope
            .strip_prefix("project:")
            .is_none_or(|slug| slug.trim().is_empty())
        {
            return Err(ServerError::InvalidRequest(
                "memory tombstones only apply to project scopes".to_string(),
            ));
        }
        let reason = redact_persisted_text(reason);
        if reason.trim().is_empty() {
            return Err(ServerError::InvalidRequest(
                "memory tombstone reason must not be empty".to_string(),
            ));
        }
        let now = Utc::now();
        let row = self
            .client
            .query_one(
                "insert into memory_scope_tombstones(
                    owner_subject, memory_scope, link_alias, reason, revoked_at
                 ) values ($1, $2, null, $3, $4)
                 on conflict (owner_subject, memory_scope) do update set
                    reason = excluded.reason,
                    revoked_at = least(memory_scope_tombstones.revoked_at, excluded.revoked_at)
                 returning owner_subject, memory_scope, link_alias, reason, revoked_at",
                &[&owner_subject, &memory_scope, &reason, &now],
            )
            .await
            .map_err(postgres_memory_error)?;
        Ok(MemoryScopeTombstone {
            owner_subject: row.get("owner_subject"),
            memory_scope: row.get("memory_scope"),
            link_alias: row.get("link_alias"),
            reason: row.get("reason"),
            revoked_at: row.get("revoked_at"),
        })
    }

    async fn memory_scope_tombstone(
        &self,
        owner_subject: &str,
        memory_scope: &str,
    ) -> Result<Option<MemoryScopeTombstone>> {
        let row = self
            .client
            .query_opt(
                "select owner_subject, memory_scope, link_alias, reason, revoked_at
                   from memory_scope_tombstones
                  where owner_subject = $1 and memory_scope = $2",
                &[&owner_subject, &memory_scope],
            )
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        Ok(row.map(|row| MemoryScopeTombstone {
            owner_subject: row.get("owner_subject"),
            memory_scope: row.get("memory_scope"),
            link_alias: row.get("link_alias"),
            reason: row.get("reason"),
            revoked_at: row.get("revoked_at"),
        }))
    }

    async fn upsert_project_item(&self, item: NewProjectItem) -> Result<ProjectItemRecord> {
        let item = sanitize_project_item_persistence(item)?;
        self.get_session(item.source_session_id).await?;
        let kind = item.kind.as_str();
        let row = self
            .client
            .query_one(
                "insert into project_items (id, project_id, kind, text, target_uri, source_session_id, source_event_seq, source_uri, dedupe_key, provenance_json, created_at)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now())
                 on conflict (project_id, kind, dedupe_key) do update
                   set text = excluded.text,
                       target_uri = excluded.target_uri,
                       source_event_seq = excluded.source_event_seq,
                       source_uri = excluded.source_uri,
                       provenance_json = excluded.provenance_json
                 returning id, project_id, kind, text, target_uri, source_session_id, source_event_seq, source_uri, dedupe_key, provenance_json, created_at",
                &[
                    &Uuid::new_v4(),
                    &item.project_id,
                    &kind,
                    &item.text,
                    &item.target_uri,
                    &item.source_session_id,
                    &item.source_event_seq,
                    &item.source_uri,
                    &item.dedupe_key,
                    &item.provenance_json,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        row_to_project_item(row)
    }

    async fn project_items(
        &self,
        project_id: &str,
        kind: Option<ProjectItemKind>,
    ) -> Result<Vec<ProjectItemRecord>> {
        let rows = if let Some(kind) = kind {
            let kind = kind.as_str();
            self.client
                .query(
                    "select id, project_id, kind, text, target_uri, source_session_id, source_event_seq, source_uri, dedupe_key, provenance_json, created_at from project_items where project_id = $1 and kind = $2 order by created_at asc",
                    &[&project_id, &kind],
                )
                .await
        } else {
            self.client
                .query(
                    "select id, project_id, kind, text, target_uri, source_session_id, source_event_seq, source_uri, dedupe_key, provenance_json, created_at from project_items where project_id = $1 order by created_at asc",
                    &[&project_id],
                )
                .await
        }
        .map_err(|err| ServerError::Store(err.to_string()))?;
        rows.into_iter().map(row_to_project_item).collect()
    }

    async fn enqueue_dream(&self, new: NewDreamQueueRecord) -> Result<DreamQueueRecord> {
        validate_persistence_identifier("dream subject", &new.subject)?;
        validate_persistence_identifier("dream scope", &new.scope)?;
        validate_persistence_identifier("dream dedupe key", &new.dedupe_key)?;
        self.get_session(new.session_id).await?;
        let reason = new.reason.as_str();
        let status = DreamStatus::Queued.as_str();
        let row = self
            .client
            .query_one(
                "insert into dream_queue (id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, lease_owner, lease_epoch, heartbeat_at, completed_at, error_at, last_error)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, 0, now(), $9, null, null, 0, null, null, null, null)
                 on conflict (dedupe_key) do update
                   set source_event_seq = coalesce(dream_queue.source_event_seq, excluded.source_event_seq)
                 returning id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, lease_owner, lease_epoch, heartbeat_at, completed_at, error_at, last_error",
                &[
                    &Uuid::new_v4(),
                    &new.session_id,
                    &new.subject,
                    &new.scope,
                    &reason,
                    &status,
                    &new.dedupe_key,
                    &new.source_event_seq,
                    &new.available_at,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        row_to_dream_record(row)
    }

    async fn dream_queue_for_session(&self, session_id: Uuid) -> Result<Vec<DreamQueueRecord>> {
        self.get_session(session_id).await?;
        let rows = self
            .client
            .query(
                "select id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, lease_owner, lease_epoch, heartbeat_at, completed_at, error_at, last_error
                   from dream_queue
                  where session_id = $1
                  order by enqueued_at asc",
                &[&session_id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        rows.into_iter().map(row_to_dream_record).collect()
    }

    async fn dream_queue(&self, scope: &str, limit: usize) -> Result<Vec<DreamQueueRecord>> {
        let rows = self
            .client
            .query(
                "select id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, lease_owner, lease_epoch, heartbeat_at, completed_at, error_at, last_error
                   from dream_queue
                  where scope = $1
                  order by enqueued_at desc
                  limit $2",
                &[&scope, &(limit as i64)],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        rows.into_iter().map(row_to_dream_record).collect()
    }

    async fn dream(&self, dream_id: Uuid) -> Result<DreamQueueRecord> {
        let row = self
            .client
            .query_opt(
                "select id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, lease_owner, lease_epoch, heartbeat_at, completed_at, error_at, last_error
                   from dream_queue
                  where id = $1",
                &[&dream_id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("dream {dream_id}")))?;
        row_to_dream_record(row)
    }

    async fn claim_ready_dream(
        &self,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        owner_id: Uuid,
    ) -> Result<Option<DreamLease>> {
        self.claim_ready_dream_bounded(now, lease_timeout, owner_id, 3)
            .await
    }

    async fn claim_ready_dream_bounded(
        &self,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        owner_id: Uuid,
        max_attempts: i32,
    ) -> Result<Option<DreamLease>> {
        if max_attempts <= 0 {
            return Err(ServerError::InvalidRequest(
                "dream max_attempts must be positive".to_string(),
            ));
        }
        let stale_before = now - lease_timeout;
        let queued = DreamStatus::Queued.as_str();
        let running = DreamStatus::Running.as_str();
        let failed = DreamStatus::Failed.as_str();
        let exhausted_error = "dream lease expired after the final permitted attempt";
        let row = self
            .client
            .query_opt(
                "with exhausted as (
                    update dream_queue
                       set status = $7, locked_at = null, lease_owner = null,
                           completed_at = null, error_at = $1, last_error = $8
                     where attempts >= $6
                       and ((status = $3 and available_at <= $1)
                         or (status = $4 and locked_at is not null and locked_at <= $2))
                     returning id
                 ), ready as (
                    select id
                      from dream_queue
                     where attempts < $6
                       and ((status = $3 and available_at <= $1)
                         or (status = $4 and locked_at is not null and locked_at <= $2))
                       and (select count(*) from exhausted) >= 0
                     order by available_at asc, enqueued_at asc
                     limit 1
                     for update skip locked
                 )
                 update dream_queue
                    set status = $4,
                        attempts = attempts + 1,
                        locked_at = $1,
                        lease_owner = $5,
                        lease_epoch = lease_epoch + 1,
                        heartbeat_at = $1,
                        completed_at = null,
                        error_at = null,
                        last_error = null
                  where id = (select id from ready)
                  returning id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, lease_owner, lease_epoch, heartbeat_at, completed_at, error_at, last_error",
                &[
                    &now,
                    &stale_before,
                    &queued,
                    &running,
                    &owner_id,
                    &max_attempts,
                    &failed,
                    &exhausted_error,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        row.map(|row| {
            let dream = row_to_dream_record(row)?;
            Ok(DreamLease::new(dream.clone(), owner_id, dream.lease_epoch))
        })
        .transpose()
    }

    async fn heartbeat_dream(&self, lease: &DreamLease, now: DateTime<Utc>) -> Result<DreamLease> {
        let running = DreamStatus::Running.as_str();
        let row = self
            .client
            .query_opt(
                "update dream_queue
                    set locked_at = $2, heartbeat_at = $2
                  where id = $1 and status = $3 and lease_owner = $4 and lease_epoch = $5
                  returning id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, lease_owner, lease_epoch, heartbeat_at, completed_at, error_at, last_error",
                &[&lease.dream.id, &now, &running, &lease.owner_id, &lease.epoch],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "active dream lease {} owner {} epoch {}",
                    lease.dream.id, lease.owner_id, lease.epoch
                ))
            })?;
        let dream = row_to_dream_record(row)?;
        Ok(DreamLease::new(dream, lease.owner_id, lease.epoch))
    }

    async fn complete_dream(
        &self,
        lease: &DreamLease,
        now: DateTime<Utc>,
    ) -> Result<DreamQueueRecord> {
        let running = DreamStatus::Running.as_str();
        let completed = DreamStatus::Completed.as_str();
        let row = self
            .client
            .query_opt(
                "update dream_queue
                    set status = $3,
                        available_at = $2,
                        locked_at = null,
                        lease_owner = null,
                        completed_at = $2,
                        error_at = null,
                        last_error = null
                  where id = $1 and status = $4 and lease_owner = $5 and lease_epoch = $6
                  returning id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, lease_owner, lease_epoch, heartbeat_at, completed_at, error_at, last_error",
                &[
                    &lease.dream.id,
                    &now,
                    &completed,
                    &running,
                    &lease.owner_id,
                    &lease.epoch,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "active dream lease {} owner {} epoch {}",
                    lease.dream.id, lease.owner_id, lease.epoch
                ))
            })?;
        row_to_dream_record(row)
    }

    async fn fail_dream(
        &self,
        lease: &DreamLease,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<DreamQueueRecord> {
        let error = redact_persisted_text(&error);
        let running = DreamStatus::Running.as_str();
        let queued = DreamStatus::Queued.as_str();
        let failed = DreamStatus::Failed.as_str();
        let row = self
            .client
            .query_opt(
                "update dream_queue
                    set status = case when attempts >= $4 then $5 else $6 end,
                        available_at = $3,
                        locked_at = null,
                        lease_owner = null,
                        completed_at = null,
                        error_at = now(),
                        last_error = $2
                  where id = $1 and status = $7 and lease_owner = $8 and lease_epoch = $9
                  returning id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, lease_owner, lease_epoch, heartbeat_at, completed_at, error_at, last_error",
                &[
                    &lease.dream.id,
                    &error,
                    &next_available_at,
                    &max_attempts,
                    &failed,
                    &queued,
                    &running,
                    &lease.owner_id,
                    &lease.epoch,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "active dream lease {} owner {} epoch {}",
                    lease.dream.id, lease.owner_id, lease.epoch
                ))
            })?;
        row_to_dream_record(row)
    }

    async fn upsert_memory_summary(
        &self,
        summary: NewMemorySummaryRecord,
    ) -> Result<MemorySummaryRecord> {
        let summary = sanitize_memory_summary_persistence(summary)?;
        if let Some(session_id) = summary.source_session_id {
            self.get_session(session_id).await?;
        }
        let kind = summary.kind.as_str();
        let evidence_json = serde_json::to_value(&summary.evidence)
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let row = self
            .client
            .query_one(
                "insert into memory_summaries (id, kind, subject, scope, title, body, evidence_json, source_dream_id, source_session_id, dedupe_key, created_at, updated_at)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now(), now())
                 on conflict (dedupe_key) do update
                   set kind = excluded.kind,
                       subject = excluded.subject,
                       scope = excluded.scope,
                       title = excluded.title,
                       body = excluded.body,
                       evidence_json = excluded.evidence_json,
                       source_dream_id = excluded.source_dream_id,
                       source_session_id = excluded.source_session_id,
                       updated_at = now()
                 returning id, kind, subject, scope, title, body, evidence_json, source_dream_id, source_session_id, dedupe_key, created_at, updated_at",
                &[
                    &Uuid::new_v4(),
                    &kind,
                    &summary.subject,
                    &summary.scope,
                    &summary.title,
                    &summary.body,
                    &evidence_json,
                    &summary.source_dream_id,
                    &summary.source_session_id,
                    &summary.dedupe_key,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        row_to_memory_summary(row)
    }

    async fn memory_summary(&self, id: Uuid) -> Result<MemorySummaryRecord> {
        let row = self
            .client
            .query_opt(
                "select id, kind, subject, scope, title, body, evidence_json, source_dream_id, source_session_id, dedupe_key, created_at, updated_at
                   from memory_summaries
                  where id = $1",
                &[&id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("memory summary {id}")))?;
        row_to_memory_summary(row)
    }

    async fn memory_summaries(
        &self,
        scope: &str,
        limit: usize,
    ) -> Result<Vec<MemorySummaryRecord>> {
        let rows = self
            .client
            .query(
                "select id, kind, subject, scope, title, body, evidence_json, source_dream_id, source_session_id, dedupe_key, created_at, updated_at
                   from memory_summaries
                  where scope = $1
                  order by updated_at desc
                  limit $2",
                &[&scope, &(limit as i64)],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        rows.into_iter().map(row_to_memory_summary).collect()
    }

    async fn upsert_skill_proposal(
        &self,
        proposal: NewSkillProposalRecord,
    ) -> Result<SkillProposalRecord> {
        let proposal = sanitize_skill_proposal_persistence(proposal)?;
        self.get_session(proposal.source_session_id).await?;
        let evidence_json = serde_json::to_value(&proposal.evidence)
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let verification_json = serde_json::to_value(&proposal.verification)
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let status = SkillProposalStatus::Pending.as_str();
        let row = self
            .client
            .query_one(
                "insert into skill_proposals (id, name, description, body, trigger, use_criteria, evidence_json, self_critique, verification_json, status, dedupe_key, source_dream_id, source_session_id, created_at, updated_at)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, now(), now())
                 on conflict (dedupe_key) do update
                   set name = excluded.name,
                       description = excluded.description,
                       body = excluded.body,
                       trigger = excluded.trigger,
                       use_criteria = excluded.use_criteria,
                       evidence_json = excluded.evidence_json,
                       self_critique = excluded.self_critique,
                       verification_json = excluded.verification_json,
                       source_dream_id = excluded.source_dream_id,
                       source_session_id = excluded.source_session_id,
                       updated_at = now()
                 returning id, name, description, body, trigger, use_criteria, evidence_json, self_critique, verification_json, status, dedupe_key, source_dream_id, source_session_id, created_at, updated_at",
                &[
                    &Uuid::new_v4(),
                    &proposal.name,
                    &proposal.description,
                    &proposal.body,
                    &proposal.trigger,
                    &proposal.use_criteria,
                    &evidence_json,
                    &proposal.self_critique,
                    &verification_json,
                    &status,
                    &proposal.dedupe_key,
                    &proposal.source_dream_id,
                    &proposal.source_session_id,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        row_to_skill_proposal(row)
    }

    async fn update_skill_proposal_status(
        &self,
        id: Uuid,
        status: SkillProposalStatus,
    ) -> Result<SkillProposalRecord> {
        let status = status.as_str();
        let row = self
            .client
            .query_opt(
                "update skill_proposals
                    set status = $2,
                        updated_at = now()
                  where id = $1
                  returning id, name, description, body, trigger, use_criteria, evidence_json, self_critique, verification_json, status, dedupe_key, source_dream_id, source_session_id, created_at, updated_at",
                &[&id, &status],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("skill proposal {id}")))?;
        row_to_skill_proposal(row)
    }

    async fn skill_proposal(&self, id: Uuid) -> Result<SkillProposalRecord> {
        let row = self
            .client
            .query_opt(
                "select id, name, description, body, trigger, use_criteria, evidence_json, self_critique, verification_json, status, dedupe_key, source_dream_id, source_session_id, created_at, updated_at
                   from skill_proposals
                  where id = $1",
                &[&id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("skill proposal {id}")))?;
        row_to_skill_proposal(row)
    }

    async fn skill_proposals_for_session(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<SkillProposalRecord>> {
        self.get_session(session_id).await?;
        let rows = self
            .client
            .query(
                "select id, name, description, body, trigger, use_criteria, evidence_json, self_critique, verification_json, status, dedupe_key, source_dream_id, source_session_id, created_at, updated_at
                   from skill_proposals
                  where source_session_id = $1
                  order by updated_at desc",
                &[&session_id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        rows.into_iter().map(row_to_skill_proposal).collect()
    }

    async fn upsert_cron_job(&self, job: NewCronJobRecord) -> Result<CronJobRecord> {
        let job = sanitize_cron_job_persistence(job)?;
        let row = self
            .client
            .query_one(
                "insert into cron_jobs (id, name, schedule, enabled, cron_mode, max_turns, script_timeout_seconds, next_run_at, updated_at)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, now())
                 on conflict (id) do update
                   set name = excluded.name,
                       schedule = excluded.schedule,
                       enabled = excluded.enabled,
                       cron_mode = excluded.cron_mode,
                       max_turns = excluded.max_turns,
                       script_timeout_seconds = excluded.script_timeout_seconds,
                       next_run_at = coalesce(cron_jobs.next_run_at, excluded.next_run_at),
                       updated_at = now()
                 returning id, name, schedule, enabled, cron_mode, max_turns, script_timeout_seconds, next_run_at, updated_at",
                &[
                    &job.id,
                    &job.name,
                    &job.schedule,
                    &job.enabled,
                    &job.cron_mode,
                    &job.max_turns,
                    &job.script_timeout_seconds,
                    &job.next_run_at,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(row_to_cron_job(row))
    }

    async fn cron_job(&self, id: &str) -> Result<CronJobRecord> {
        let row = self
            .client
            .query_opt(
                "select id, name, schedule, enabled, cron_mode, max_turns, script_timeout_seconds, next_run_at, updated_at
                   from cron_jobs
                  where id = $1",
                &[&id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("cron job {id}")))?;
        Ok(row_to_cron_job(row))
    }

    async fn cron_jobs(&self) -> Result<Vec<CronJobRecord>> {
        let rows = self
            .client
            .query(
                "select id, name, schedule, enabled, cron_mode, max_turns, script_timeout_seconds, next_run_at, updated_at
                   from cron_jobs
                  order by id asc",
                &[],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(rows.into_iter().map(row_to_cron_job).collect())
    }

    async fn materialize_cron_run(
        &self,
        run: NewCronRunRecord,
        expected_next_run_at: DateTime<Utc>,
        next_run_at: DateTime<Utc>,
    ) -> Result<Option<CronRunRecord>> {
        let run = sanitize_cron_run_persistence(run)?;
        if run.scheduled_for != expected_next_run_at || next_run_at <= expected_next_run_at {
            return Err(ServerError::InvalidRequest(
                "cron materialization must match and advance the job cursor".to_string(),
            ));
        }
        if let Some(session_id) = run.session_id {
            self.get_session(session_id).await?;
        }
        let now = Utc::now();
        let row = self
            .client
            .query_opt(
                "with advanced as (
                    update cron_jobs
                       set next_run_at = $3, updated_at = $4
                     where id = $1 and enabled and next_run_at = $2
                     returning id
                 ), inserted as (
                    insert into cron_runs
                        (id, job_id, scheduled_for, status, session_id, started_at, completed_at,
                         attempts, available_at, locked_at, lease_owner, lease_epoch, last_error,
                         result_json)
                    select $5, advanced.id, $2, 'queued', $6, $4, null,
                           0, $4, null, null, 0, null, $7
                      from advanced
                    on conflict (job_id, scheduled_for) do nothing
                    returning id, job_id, scheduled_for, status, session_id, started_at,
                              completed_at, attempts, available_at, locked_at, lease_owner,
                              lease_epoch, last_error, result_json
                 ), selected as (
                    select * from inserted
                    union all
                    select runs.id, runs.job_id, runs.scheduled_for, runs.status,
                           runs.session_id, runs.started_at, runs.completed_at, runs.attempts,
                           runs.available_at, runs.locked_at, runs.lease_owner,
                           runs.lease_epoch, runs.last_error, runs.result_json
                      from cron_runs runs
                      join advanced on advanced.id = runs.job_id
                     where runs.scheduled_for = $2
                       and not exists (select 1 from inserted)
                 )
                 select * from selected limit 1",
                &[
                    &run.job_id,
                    &expected_next_run_at,
                    &next_run_at,
                    &now,
                    &Uuid::new_v4(),
                    &run.session_id,
                    &run.result_json,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(row.map(row_to_cron_run))
    }

    async fn claim_ready_cron_run(
        &self,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        max_attempts: i32,
    ) -> Result<Option<CronLease>> {
        if max_attempts <= 0 {
            return Err(ServerError::InvalidRequest(
                "cron max_attempts must be positive".to_string(),
            ));
        }
        let stale_before = now - lease_timeout;
        let exhausted_error = "cron lease expired after the final permitted attempt";
        let row = self
            .client
            .query_opt(
                "with exhausted as (
                    update cron_runs
                       set status = 'failed', completed_at = $2, locked_at = null,
                           lease_owner = null, last_error = $5
                     where attempts >= $4
                       and ((status = 'queued' and available_at <= $2)
                         or (status = 'running' and locked_at <= $3))
                     returning id
                 ), candidate as (
                    select runs.id
                      from cron_runs runs
                      join cron_jobs jobs on jobs.id = runs.job_id
                     where jobs.enabled
                       and runs.attempts < $4
                       and ((runs.status = 'queued' and runs.available_at <= $2)
                         or (runs.status = 'running' and runs.locked_at <= $3))
                       and (select count(*) from exhausted) >= 0
                     order by runs.available_at asc, runs.scheduled_for asc, runs.id asc
                     limit 1
                     for update of runs skip locked
                 )
                 update cron_runs
                    set status = 'running', attempts = attempts + 1, available_at = $2,
                        locked_at = $2, lease_owner = $1, lease_epoch = lease_epoch + 1,
                        last_error = null, completed_at = null
                  where id = (select id from candidate)
                  returning id, job_id, scheduled_for, status, session_id, started_at,
                            completed_at, attempts, available_at, locked_at, lease_owner,
                            lease_epoch, last_error, result_json",
                &[
                    &owner_id,
                    &now,
                    &stale_before,
                    &max_attempts,
                    &exhausted_error,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(row.map(|row| {
            let run = row_to_cron_run(row);
            let epoch = run.lease_epoch;
            CronLease {
                run,
                owner_id,
                epoch,
            }
        }))
    }

    async fn claim_cron_run(
        &self,
        run: NewCronRunRecord,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<(CronLease, bool)> {
        let run = sanitize_cron_run_persistence(run)?;
        self.cron_job(&run.job_id).await?;
        if let Some(session_id) = run.session_id {
            self.get_session(session_id).await?;
        }
        let stale_before = now - lease_timeout;
        let exhausted_error = "cron lease expired after the final permitted attempt";
        let row = self
            .client
            .query_opt(
                "with exhausted as (
                    update cron_runs
                       set status = 'failed', completed_at = $5, locked_at = null,
                           lease_owner = null, last_error = $9
                     where job_id = $2 and scheduled_for = $3 and attempts >= 3
                       and ((status = 'queued' and available_at <= $5)
                         or (status = 'running' and locked_at <= $8))
                     returning id
                 ), claimed as (
                 insert into cron_runs
                    (id, job_id, scheduled_for, status, session_id, started_at, completed_at,
                     attempts, available_at, locked_at, lease_owner, lease_epoch, last_error,
                     result_json)
                 values ($1, $2, $3, 'running', $4, $5, null, 1, $5, $5, $6, 1, null, $7)
                 on conflict (job_id, scheduled_for) do update
                   set status = 'running',
                       session_id = coalesce(excluded.session_id, cron_runs.session_id),
                       completed_at = null,
                       attempts = cron_runs.attempts + 1,
                       available_at = $5,
                       locked_at = $5,
                       lease_owner = $6,
                       lease_epoch = cron_runs.lease_epoch + 1,
                       last_error = null
                 where cron_runs.attempts < 3
                   and ((cron_runs.status = 'queued' and cron_runs.available_at <= $5)
                     or (cron_runs.status = 'running' and cron_runs.locked_at <= $8))
                   and (select count(*) from exhausted) >= 0
                 returning id, job_id, scheduled_for, status, session_id, started_at,
                           completed_at, attempts, available_at, locked_at, lease_owner,
                           lease_epoch, last_error, result_json
                 )
                 select * from claimed",
                &[
                    &Uuid::new_v4(),
                    &run.job_id,
                    &run.scheduled_for,
                    &run.session_id,
                    &now,
                    &owner_id,
                    &run.result_json,
                    &stale_before,
                    &exhausted_error,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        if let Some(row) = row {
            let record = row_to_cron_run(row);
            let epoch = record.lease_epoch;
            return Ok((
                CronLease {
                    run: record,
                    owner_id,
                    epoch,
                },
                true,
            ));
        }
        let row = self
            .client
            .query_one(
                "select id, job_id, scheduled_for, status, session_id, started_at,
                        completed_at, attempts, available_at, locked_at, lease_owner,
                        lease_epoch, last_error, result_json
                   from cron_runs
                  where job_id = $1 and scheduled_for = $2",
                &[&run.job_id, &run.scheduled_for],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let record = row_to_cron_run(row);
        let lease_owner = record.lease_owner.unwrap_or(owner_id);
        let epoch = record.lease_epoch;
        Ok((
            CronLease {
                run: record,
                owner_id: lease_owner,
                epoch,
            },
            false,
        ))
    }

    async fn record_cron_run(&self, run: NewCronRunRecord) -> Result<CronRunRecord> {
        let run = sanitize_cron_run_persistence(run)?;
        self.cron_job(&run.job_id).await?;
        if let Some(session_id) = run.session_id {
            self.get_session(session_id).await?;
        }
        let now = Utc::now();
        self.client
            .execute(
                "insert into cron_runs
                    (id, job_id, scheduled_for, status, session_id, started_at, completed_at,
                     attempts, available_at, locked_at, lease_owner, lease_epoch, last_error,
                     result_json)
                 values ($1, $2, $3, $4, $5, $6, null, 0, $6, null, null, 0, null, $7)
                 on conflict (job_id, scheduled_for) do nothing",
                &[
                    &Uuid::new_v4(),
                    &run.job_id,
                    &run.scheduled_for,
                    &run.status,
                    &run.session_id,
                    &now,
                    &run.result_json,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let row = self
            .client
            .query_one(
                "select id, job_id, scheduled_for, status, session_id, started_at,
                        completed_at, attempts, available_at, locked_at, lease_owner,
                        lease_epoch, last_error, result_json
                   from cron_runs
                  where job_id = $1 and scheduled_for = $2",
                &[&run.job_id, &run.scheduled_for],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(row_to_cron_run(row))
    }

    async fn heartbeat_cron_run(&self, lease: &CronLease, now: DateTime<Utc>) -> Result<CronLease> {
        let row = self
            .client
            .query_opt(
                "update cron_runs
                    set locked_at = $2
                  where id = $1 and status = 'running' and lease_owner = $3 and lease_epoch = $4
                  returning id, job_id, scheduled_for, status, session_id, started_at,
                            completed_at, attempts, available_at, locked_at, lease_owner,
                            lease_epoch, last_error, result_json",
                &[&lease.run.id, &now, &lease.owner_id, &lease.epoch],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "active cron lease {} owner {} epoch {}",
                    lease.run.id, lease.owner_id, lease.epoch
                ))
            })?;
        Ok(CronLease {
            run: row_to_cron_run(row),
            owner_id: lease.owner_id,
            epoch: lease.epoch,
        })
    }

    async fn complete_cron_run(
        &self,
        lease: &CronLease,
        status: &str,
        session_id: Option<Uuid>,
        result_json: Value,
    ) -> Result<CronRunRecord> {
        validate_persistence_identifier("cron completion status", status)?;
        let result_json = redact_persisted_json(result_json);
        if let Some(session_id) = session_id {
            self.get_session(session_id).await?;
        }
        let row = self
            .client
            .query_opt(
                "update cron_runs
                    set status = $2,
                        session_id = coalesce($3, session_id),
                        completed_at = now(),
                        locked_at = null,
                        lease_owner = null,
                        last_error = null,
                        result_json = $4
                  where id = $1 and status = 'running' and lease_owner = $5 and lease_epoch = $6
                  returning id, job_id, scheduled_for, status, session_id, started_at,
                            completed_at, attempts, available_at, locked_at, lease_owner,
                            lease_epoch, last_error, result_json",
                &[
                    &lease.run.id,
                    &status,
                    &session_id,
                    &result_json,
                    &lease.owner_id,
                    &lease.epoch,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "active cron lease {} owner {} epoch {}",
                    lease.run.id, lease.owner_id, lease.epoch
                ))
            })?;
        Ok(row_to_cron_run(row))
    }

    async fn fail_cron_run(
        &self,
        lease: &CronLease,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<CronRunRecord> {
        let error = redact_persisted_text(&error);
        let row = self
            .client
            .query_opt(
                "update cron_runs
                    set status = case when attempts >= $4 then 'failed' else 'queued' end,
                        available_at = $3,
                        completed_at = null,
                        locked_at = null,
                        lease_owner = null,
                        last_error = $2
                  where id = $1 and status = 'running' and lease_owner = $5 and lease_epoch = $6
                  returning id, job_id, scheduled_for, status, session_id, started_at,
                            completed_at, attempts, available_at, locked_at, lease_owner,
                            lease_epoch, last_error, result_json",
                &[
                    &lease.run.id,
                    &error,
                    &next_available_at,
                    &max_attempts,
                    &lease.owner_id,
                    &lease.epoch,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "active cron lease {} owner {} epoch {}",
                    lease.run.id, lease.owner_id, lease.epoch
                ))
            })?;
        Ok(row_to_cron_run(row))
    }

    async fn cron_runs(&self, job_id: &str, limit: usize) -> Result<Vec<CronRunRecord>> {
        self.cron_job(job_id).await?;
        let rows = self
            .client
            .query(
                "select id, job_id, scheduled_for, status, session_id, started_at,
                        completed_at, attempts, available_at, locked_at, lease_owner,
                        lease_epoch, last_error, result_json
                   from cron_runs
                  where job_id = $1
                  order by started_at desc
                  limit $2",
                &[&job_id, &(limit as i64)],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(rows.into_iter().map(row_to_cron_run).collect())
    }

    async fn runtime_metrics(&self, now: DateTime<Utc>) -> Result<StoreRuntimeMetrics> {
        let row = self
            .client
            .query_one(
                "select
                    (select count(*)::bigint from session_turns where status = 'queued') as turn_depth,
                    (select min(created_at) from session_turns where status = 'queued') as turn_oldest,
                    (select count(*)::bigint from dream_queue where status = 'queued') as dream_depth,
                    (select min(enqueued_at) from dream_queue where status = 'queued') as dream_oldest,
                    (select count(*)::bigint from cron_runs where status = 'queued') as cron_depth,
                    (select min(started_at) from cron_runs where status = 'queued') as cron_oldest,
                    (select min(next_run_at) from cron_jobs where enabled and next_run_at <= $1) as scheduler_cursor,
                    (select count(*)::bigint from approval_requests where status = 'pending') as pending_approvals,
                    (select count(*)::bigint from approval_effects where status = 'pending') as effect_depth,
                    (select min(created_at) from approval_effects where status = 'pending') as effect_oldest,
                    (select coalesce(sum(greatest(attempts - 1, 0)), 0)::bigint from dream_queue)
                      + (select coalesce(sum(greatest(attempts - 1, 0)), 0)::bigint from cron_runs)
                      + (select coalesce(sum(greatest(attempts - 1, 0)), 0)::bigint from approval_effects)
                      as lease_reclaims",
                &[&now],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let age_seconds = |timestamp: Option<DateTime<Utc>>| {
            timestamp
                .map(|timestamp| now.signed_duration_since(timestamp).num_seconds().max(0))
                .unwrap_or(0)
        };
        Ok(StoreRuntimeMetrics {
            turn_queue_depth: row.get("turn_depth"),
            turn_oldest_age_seconds: age_seconds(row.get("turn_oldest")),
            dream_queue_depth: row.get("dream_depth"),
            dream_oldest_age_seconds: age_seconds(row.get("dream_oldest")),
            cron_queue_depth: row.get("cron_depth"),
            cron_oldest_age_seconds: age_seconds(row.get("cron_oldest")),
            scheduler_lag_seconds: age_seconds(row.get("scheduler_cursor")),
            pending_approvals: row.get("pending_approvals"),
            approval_effect_queue_depth: row.get("effect_depth"),
            approval_effect_oldest_age_seconds: age_seconds(row.get("effect_oldest")),
            lease_reclaims: row.get("lease_reclaims"),
        })
    }
}

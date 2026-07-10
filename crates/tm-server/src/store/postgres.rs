mod rows;
mod schema;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rows::{
    row_to_approval_effect, row_to_approval_request, row_to_cron_job, row_to_cron_run,
    row_to_dream_record, row_to_memory_summary, row_to_message_record, row_to_project_item,
    row_to_session_record, row_to_session_turn, row_to_skill_proposal,
};
use serde_json::{Value, json};
use tm_memory::{
    DreamLease, DreamQueueRecord, DreamStatus, MemorySummaryRecord, NewDreamQueueRecord,
    NewMemorySummaryRecord, NewSkillProposalRecord, SkillProposalRecord, SkillProposalStatus,
};
use tokio_postgres::{Config as PostgresConfig, NoTls, Row, error::SqlState};
use uuid::Uuid;

use crate::{
    AuthDeviceRecord, AuthDeviceStore, NewAuthDevice, NewPairingCode, PairingCodeRecord, Result,
    ServerError,
};

use super::models::{
    redact_persisted_json, redact_persisted_text, sanitize_cron_job_persistence,
    sanitize_cron_run_persistence, sanitize_memory_summary_persistence,
    sanitize_project_item_persistence, sanitize_skill_proposal_persistence, turn_content_hash,
    validate_persistence_identifier, validate_profile_fact_persistence,
    validate_recall_chunk_persistence,
};

use super::{
    ApprovalEffectLease, ApprovalEffectRecord, ApprovalRequestRecord, CronJobRecord, CronLease,
    CronRunRecord, EndSessionDreamResult, MessageRecord, ModeState, NewApprovalRequest,
    NewApprovalResolution, NewCronJobRecord, NewCronRunRecord, NewProjectItem, NewSession,
    ProfileFactRecord, ProjectItemKind, ProjectItemRecord, RecallChunkRecord, SessionEvent,
    SessionRecord, SessionSummaryRecord, SessionTurnRecord, Store, StoreRuntimeMetrics,
};

pub struct PostgresStore {
    client: tokio_postgres::Client,
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
            .connect(NoTls)
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                let err = tm_memory::redact_dream_text(&err.to_string()).text;
                tracing::error!(%err, "postgres connection failed");
            }
        });
        let mut store = Self { client };
        store.ensure_schema().await?;
        Ok(store)
    }

    pub fn client(&self) -> &tokio_postgres::Client {
        &self.client
    }
}

#[async_trait]
impl AuthDeviceStore for PostgresStore {
    async fn auth_device_count(&self) -> Result<usize> {
        let count: i64 = self
            .client
            .query_one("select count(*)::bigint from auth_devices", &[])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .get(0);
        Ok(count.max(0) as usize)
    }

    async fn authenticate_device(
        &self,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<AuthDeviceRecord>> {
        let row = self
            .client
            .query_opt(
                "update auth_devices
                    set last_seen_at = $2
                  where token_hash = $1 and revoked_at is null
                  returning id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at",
                &[&token_hash, &now],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(row.map(row_to_auth_device))
    }

    async fn create_pairing_code(&self, code: NewPairingCode) -> Result<PairingCodeRecord> {
        let row = self
            .client
            .query_one(
                "insert into pairing_codes
                    (id, code_hash, created_at, expires_at, consumed_at, created_by_device_id)
                 values ($1, $2, $3, $4, null, $5)
                 returning id, code_hash, created_at, expires_at, consumed_at, created_by_device_id",
                &[
                    &code.id,
                    &code.code_hash,
                    &code.created_at,
                    &code.expires_at,
                    &code.created_by_device_id,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(row_to_pairing_code(row))
    }

    async fn consume_pairing_code(
        &self,
        code_hash: &str,
        device: NewAuthDevice,
        now: DateTime<Utc>,
    ) -> Result<AuthDeviceRecord> {
        let row = self
            .client
            .query_opt(
                "with consumed as (
                    update pairing_codes
                       set consumed_at = $2
                     where code_hash = $1
                       and consumed_at is null
                       and expires_at > $2
                     returning id
                 ), inserted as (
                    insert into auth_devices
                        (id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at)
                    select $3, $4, $5, $6, $7, $8, $8, null
                      from consumed
                    returning id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at
                 )
                 select id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at
                   from inserted",
                &[
                    &code_hash,
                    &now,
                    &device.id,
                    &device.owner_subject,
                    &device.name,
                    &device.platform,
                    &device.token_hash,
                    &device.created_at,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or(ServerError::Forbidden)?;
        Ok(row_to_auth_device(row))
    }

    async fn auth_devices(&self) -> Result<Vec<AuthDeviceRecord>> {
        let rows = self
            .client
            .query(
                "select id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at
                   from auth_devices
                  order by created_at desc",
                &[],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(rows.into_iter().map(row_to_auth_device).collect())
    }

    async fn revoke_auth_device(
        &self,
        device_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<AuthDeviceRecord> {
        let row = self
            .client
            .query_opt(
                "update auth_devices
                    set revoked_at = coalesce(revoked_at, $2)
                  where id = $1
                  returning id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at",
                &[&device_id, &now],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("auth device {device_id}")))?;
        Ok(row_to_auth_device(row))
    }
}

fn row_to_auth_device(row: Row) -> AuthDeviceRecord {
    AuthDeviceRecord {
        id: row.get("id"),
        owner_subject: row.get("owner_subject"),
        name: row.get("name"),
        platform: row.get("platform"),
        token_hash: row.get("token_hash"),
        created_at: row.get("created_at"),
        last_seen_at: row.get("last_seen_at"),
        revoked_at: row.get("revoked_at"),
    }
}

fn row_to_pairing_code(row: Row) -> PairingCodeRecord {
    PairingCodeRecord {
        id: row.get("id"),
        code_hash: row.get("code_hash"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        consumed_at: row.get("consumed_at"),
        created_by_device_id: row.get("created_by_device_id"),
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
                        coalesce((select count from changed), 0)::bigint as updated",
                &[&owner_subject],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        if !row.get::<_, bool>("configured") {
            return Err(ServerError::Conflict(
                "server is already configured for a different owner subject".to_string(),
            ));
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
        let row = self
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
            .map_err(|err| ServerError::Store(err.to_string()))?;
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
                    on conflict (approval_id) do nothing
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
                 )
                 select * from resolved",
                &[
                    &approval_id,
                    &session_id,
                    &resolution.status,
                    &resolution.selected_option_id,
                    &resolution.resolution_json,
                    &resolution.resolved_at,
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
        let row = self
            .client
            .query_opt(
                "with leased as (
                    select id, session_id
                      from approval_effects
                     where id = $1
                       and effect_type in ('memory_write', 'skill_write')
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
        let row = self
            .client
            .query_opt(
                "update approval_effects
                    set status = case when attempts >= $4 then 'failed' else 'pending' end,
                        available_at = $3, locked_at = null, lease_owner = null,
                        updated_at = now(), error_at = now(), last_error = $2
                  where id = $1 and status = 'claimed' and lease_owner = $5 and lease_epoch = $6
                  returning id, approval_id, session_id, effect_type, payload_json, status,
                            attempts, available_at, locked_at, lease_owner, lease_epoch,
                            applied_at, error_at, last_error, created_at, updated_at",
                &[
                    &lease.effect.id,
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
                    "active approval effect lease {} owner {} epoch {}",
                    lease.effect.id, lease.owner_id, lease.epoch
                ))
            })?;
        Ok(row_to_approval_effect(row))
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

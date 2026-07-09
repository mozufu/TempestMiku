mod rows;
mod schema;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rows::{
    row_to_cron_job, row_to_cron_run, row_to_dream_record, row_to_memory_summary,
    row_to_message_record, row_to_project_item, row_to_session_record, row_to_skill_proposal,
};
use serde_json::Value;
use tm_memory::{
    DreamQueueRecord, DreamReason, DreamStatus, MemorySummaryRecord, NewDreamQueueRecord,
    NewMemorySummaryRecord, NewSkillProposalRecord, SkillProposalRecord, SkillProposalStatus,
};
use tokio_postgres::NoTls;
use uuid::Uuid;

use crate::{Result, ServerError};

use super::{
    CronJobRecord, CronRunRecord, MessageRecord, ModeState, NewCronJobRecord, NewCronRunRecord,
    NewProjectItem, NewSession, ProfileFactRecord, ProjectItemKind, ProjectItemRecord,
    RecallChunkRecord, SessionEvent, SessionRecord, SessionSummaryRecord, Store,
};

pub struct PostgresStore {
    client: tokio_postgres::Client,
}

impl PostgresStore {
    const SCHEMA_LOCK_ID: i64 = 0x544d_5343_4845_4d41;

    pub async fn connect(dsn: &str) -> Result<Self> {
        let (client, connection) = tokio_postgres::connect(dsn, NoTls)
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                tracing::error!(%err, "postgres connection failed");
            }
        });
        let store = Self { client };
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
        let session = SessionRecord {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            status: "open".to_string(),
            mode: new.mode.clone(),
            mode_state: ModeState::new(new.mode, Utc::now()),
            persona_status: new.persona_status,
        };
        let mode = serde_json::to_value(session.mode.clone())
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let mode_state = serde_json::to_value(&session.mode_state)
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let persona_status = serde_json::to_value(&session.persona_status)
            .map_err(|err| ServerError::Store(err.to_string()))?;
        self.client
            .execute(
                "insert into sessions (id, created_at, updated_at, status, mode, mode_state_json, persona_status) values ($1, $2, $3, $4, $5, $6, $7)",
                &[&session.id, &session.created_at, &session.updated_at, &session.status, &mode, &mode_state, &persona_status],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(session)
    }

    async fn end_session(&self, session_id: Uuid) -> Result<SessionRecord> {
        self.get_session(session_id).await?;
        self.client
            .execute(
                "update sessions set status = 'ended', updated_at = now() where id = $1",
                &[&session_id],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        self.get_session(session_id).await
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
            .query_opt("select id, created_at, updated_at, status, mode, mode_state_json, persona_status from sessions where id = $1", &[&session_id])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        row_to_session_record(&row)
    }

    async fn session_messages(&self, session_id: Uuid) -> Result<Vec<MessageRecord>> {
        self.get_session(session_id).await?;
        let rows = self.client
            .query("select session_id, seq, role, content, created_at from messages where session_id = $1 order by seq asc", &[&session_id])
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
        self.get_session(session_id).await?;
        let session_key = session_id.to_string();
        let row = self
            .client
            .query_one(
                "with lock as (select pg_advisory_xact_lock(hashtext($1))),
                      next_seq as (select coalesce(max(seq) + 1, 1) as seq from messages, lock where session_id = $2),
                      inserted as (
                        insert into messages (session_id, seq, role, content, created_at)
                        select $2, next_seq.seq, $3, $4, now() from next_seq
                        returning seq, created_at
                      ),
                      touch as (
                        update sessions set updated_at = (select created_at from inserted) where id = $2
                      )
                 select seq, created_at from inserted",
                &[&session_key, &session_id, &role, &content],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(MessageRecord {
            session_id,
            seq: row.get("seq"),
            role: role.to_string(),
            content: content.to_string(),
            created_at: row.get("created_at"),
        })
    }

    async fn append_event(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
    ) -> Result<SessionEvent> {
        self.get_session(session_id).await?;
        let session_key = session_id.to_string();
        let output_refs = SessionEvent::output_refs(event_type, &payload_json);
        let row = self
            .client
            .query_one(
                "with lock as (select pg_advisory_xact_lock(hashtext($1))),
                      next_seq as (select coalesce(max(seq) + 1, 1) as seq from session_events, lock where session_id = $2),
                      inserted as (
                        insert into session_events (session_id, seq, event_type, payload_json, actor_id, artifact_uri, history_uri, created_at)
                        select $2, next_seq.seq, $3, $4, $5, $6, $7, now() from next_seq
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
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(SessionEvent::new(
            session_id,
            row.get("seq"),
            event_type,
            payload_json,
            row.get("created_at"),
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
            .query("select session_id, seq, event_type, payload_json, actor_id, artifact_uri, history_uri, created_at from session_events where session_id = $1 and seq > $2 order by seq asc", &[&session_id, &min_seq])
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
                created_at: row.get("created_at"),
            })
            .collect())
    }

    async fn add_profile_fact(&self, fact: ProfileFactRecord) -> Result<()> {
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
        self.get_session(new.session_id).await?;
        let reason = new.reason.as_str();
        let status = DreamStatus::Queued.as_str();
        let row = self
            .client
            .query_one(
                "insert into dream_queue (id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, last_error)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, 0, now(), $9, null, null)
                 on conflict (dedupe_key) do update
                   set source_event_seq = coalesce(dream_queue.source_event_seq, excluded.source_event_seq)
                 returning id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, last_error",
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
                "select id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, last_error
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
                "select id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, last_error
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
                "select id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, last_error
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
    ) -> Result<Option<DreamQueueRecord>> {
        let stale_before = now - lease_timeout;
        let queued = DreamStatus::Queued.as_str();
        let running = DreamStatus::Running.as_str();
        let row = self
            .client
            .query_opt(
                "with ready as (
                    select id
                      from dream_queue
                     where (status = $3 and available_at <= $1)
                        or (status = $4 and locked_at is not null and locked_at <= $2)
                     order by available_at asc, enqueued_at asc
                     limit 1
                     for update skip locked
                 )
                 update dream_queue
                    set status = $4,
                        attempts = attempts + 1,
                        locked_at = $1,
                        last_error = null
                  where id = (select id from ready)
                  returning id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, last_error",
                &[&now, &stale_before, &queued, &running],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        row.map(row_to_dream_record).transpose()
    }

    async fn heartbeat_dream(
        &self,
        dream_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<DreamQueueRecord> {
        let running = DreamStatus::Running.as_str();
        let row = self
            .client
            .query_opt(
                "update dream_queue
                    set locked_at = $2
                  where id = $1 and status = $3
                  returning id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, last_error",
                &[&dream_id, &now, &running],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("running dream {dream_id}")))?;
        row_to_dream_record(row)
    }

    async fn complete_dream(&self, dream_id: Uuid, now: DateTime<Utc>) -> Result<DreamQueueRecord> {
        let running = DreamStatus::Running.as_str();
        let completed = DreamStatus::Completed.as_str();
        let row = self
            .client
            .query_opt(
                "update dream_queue
                    set status = $3,
                        available_at = $2,
                        locked_at = null,
                        last_error = null
                  where id = $1 and status = $4
                  returning id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, last_error",
                &[&dream_id, &now, &completed, &running],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("running dream {dream_id}")))?;
        row_to_dream_record(row)
    }

    async fn fail_dream(
        &self,
        dream_id: Uuid,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<DreamQueueRecord> {
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
                        last_error = $2
                  where id = $1 and status = $7
                  returning id, session_id, subject, scope, reason, status, dedupe_key, source_event_seq, attempts, enqueued_at, available_at, locked_at, last_error",
                &[
                    &dream_id,
                    &error,
                    &next_available_at,
                    &max_attempts,
                    &failed,
                    &queued,
                    &running,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("running dream {dream_id}")))?;
        row_to_dream_record(row)
    }

    async fn upsert_memory_summary(
        &self,
        summary: NewMemorySummaryRecord,
    ) -> Result<MemorySummaryRecord> {
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
                       next_run_at = excluded.next_run_at,
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

    async fn claim_cron_run(&self, run: NewCronRunRecord) -> Result<(CronRunRecord, bool)> {
        self.cron_job(&run.job_id).await?;
        if let Some(session_id) = run.session_id {
            self.get_session(session_id).await?;
        }
        let existing = self
            .client
            .query_opt(
                "select id, job_id, scheduled_for, status, session_id, started_at, completed_at, result_json
                   from cron_runs
                  where job_id = $1 and scheduled_for = $2
                  order by started_at asc
                  limit 1",
                &[&run.job_id, &run.scheduled_for],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        if let Some(row) = existing {
            return Ok((row_to_cron_run(row), false));
        }
        let row = self
            .client
            .query_one(
                "insert into cron_runs (id, job_id, scheduled_for, status, session_id, started_at, completed_at, result_json)
                 values ($1, $2, $3, $4, $5, now(), null, $6)
                 returning id, job_id, scheduled_for, status, session_id, started_at, completed_at, result_json",
                &[
                    &Uuid::new_v4(),
                    &run.job_id,
                    &run.scheduled_for,
                    &run.status,
                    &run.session_id,
                    &run.result_json,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok((row_to_cron_run(row), true))
    }

    async fn record_cron_run(&self, run: NewCronRunRecord) -> Result<CronRunRecord> {
        let (record, _) = self.claim_cron_run(run).await?;
        Ok(record)
    }

    async fn complete_cron_run(
        &self,
        run_id: Uuid,
        status: &str,
        session_id: Option<Uuid>,
        result_json: Value,
    ) -> Result<CronRunRecord> {
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
                        result_json = $4
                  where id = $1
                  returning id, job_id, scheduled_for, status, session_id, started_at, completed_at, result_json",
                &[&run_id, &status, &session_id, &result_json],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("cron run {run_id}")))?;
        Ok(row_to_cron_run(row))
    }

    async fn cron_runs(&self, job_id: &str, limit: usize) -> Result<Vec<CronRunRecord>> {
        self.cron_job(job_id).await?;
        let rows = self
            .client
            .query(
                "select id, job_id, scheduled_for, status, session_id, started_at, completed_at, result_json
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
}

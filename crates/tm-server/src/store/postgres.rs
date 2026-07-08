use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use tm_memory::{
    DreamQueueRecord, DreamReason, DreamStatus, MemorySummaryKind, MemorySummaryRecord,
    NewDreamQueueRecord, NewMemorySummaryRecord, NewSkillProposalRecord, SkillProposalRecord,
    SkillProposalStatus, SkillVerification,
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

    async fn ensure_schema(&self) -> Result<()> {
        self.client
            .query_one("select pg_advisory_lock($1)", &[&Self::SCHEMA_LOCK_ID])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;

        let schema_result = self.ensure_schema_unlocked().await;
        let unlock_result = self
            .client
            .query_one("select pg_advisory_unlock($1)", &[&Self::SCHEMA_LOCK_ID])
            .await
            .map(|_| ())
            .map_err(|err| ServerError::Store(err.to_string()));

        match (schema_result, unlock_result) {
            (Err(err), _) => Err(err),
            (Ok(()), Err(err)) => Err(err),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    async fn ensure_schema_unlocked(&self) -> Result<()> {
        self.client
            .batch_execute(
                "create table if not exists sessions(id uuid primary key, created_at timestamptz not null, updated_at timestamptz not null, status text not null, mode jsonb not null, mode_state_json jsonb, persona_status jsonb not null);
                 alter table sessions add column if not exists mode_state_json jsonb;
                 create table if not exists session_events(session_id uuid not null references sessions(id) on delete cascade, seq bigint not null, event_type text not null, payload_json jsonb not null, actor_id text, artifact_uri text, history_uri text, created_at timestamptz not null, primary key(session_id, seq));
                 alter table session_events add column if not exists actor_id text;
                 alter table session_events add column if not exists artifact_uri text;
                 alter table session_events add column if not exists history_uri text;
                 update session_events
                    set actor_id = coalesce(actor_id, payload_json ->> 'actor_id', payload_json ->> 'actorId'),
                        artifact_uri = coalesce(artifact_uri, payload_json ->> 'artifact_uri', payload_json ->> 'artifactUri'),
                        history_uri = coalesce(history_uri, payload_json ->> 'history_uri', payload_json ->> 'historyUri')
                  where event_type in ('actor_completed', 'actor_resources_linked')
                    and ((actor_id is null and coalesce(payload_json ->> 'actor_id', payload_json ->> 'actorId') is not null)
                      or (artifact_uri is null and coalesce(payload_json ->> 'artifact_uri', payload_json ->> 'artifactUri') is not null)
                      or (history_uri is null and coalesce(payload_json ->> 'history_uri', payload_json ->> 'historyUri') is not null));
                 create table if not exists messages(session_id uuid not null references sessions(id) on delete cascade, seq bigint not null, role text not null, content text not null, created_at timestamptz not null, primary key(session_id, seq));
                 create table if not exists profile_facts(id uuid primary key, subject text not null, predicate text not null, object text not null, confidence real not null, provenance text not null, valid_from timestamptz not null, valid_to timestamptz);
                 create table if not exists recall_chunks(id uuid primary key, scope text not null, text text not null, source text not null, created_at timestamptz not null);
                 alter table profile_facts add column if not exists importance real not null default 0.5;
                 alter table recall_chunks add column if not exists importance real not null default 0.5;
                 create table if not exists project_items(id uuid primary key, project_id text not null, kind text not null, text text not null, target_uri text not null, source_session_id uuid not null references sessions(id) on delete cascade, source_event_seq bigint, source_uri text, dedupe_key text not null, provenance_json jsonb not null, created_at timestamptz not null, unique(project_id, kind, dedupe_key));
                 create table if not exists dream_queue(id uuid primary key, session_id uuid not null references sessions(id) on delete cascade, subject text not null, scope text not null, reason text not null, status text not null, dedupe_key text not null unique, source_event_seq bigint, attempts integer not null default 0, enqueued_at timestamptz not null, available_at timestamptz not null, locked_at timestamptz, last_error text);
                 create table if not exists memory_summaries(id uuid primary key, kind text not null, subject text not null, scope text not null, title text not null, body text not null, evidence_json jsonb not null, source_dream_id uuid not null, source_session_id uuid references sessions(id) on delete set null, dedupe_key text not null unique, created_at timestamptz not null, updated_at timestamptz not null);
                 create table if not exists skill_proposals(id uuid primary key, name text not null, description text not null, body text not null, trigger text not null, use_criteria text not null, evidence_json jsonb not null, self_critique text not null, verification_json jsonb not null, status text not null, dedupe_key text not null unique, source_dream_id uuid not null, source_session_id uuid not null references sessions(id) on delete cascade, created_at timestamptz not null, updated_at timestamptz not null);
                 create table if not exists cron_jobs(id text primary key, name text not null, schedule text not null, enabled boolean not null, cron_mode text not null, max_turns integer not null, script_timeout_seconds integer not null, next_run_at timestamptz, updated_at timestamptz not null);
                 create table if not exists cron_runs(id uuid primary key, job_id text not null references cron_jobs(id) on delete cascade, scheduled_for timestamptz not null, status text not null, session_id uuid references sessions(id) on delete set null, started_at timestamptz not null, completed_at timestamptz, result_json jsonb not null);
                 create index if not exists profile_facts_subject_idx on profile_facts(subject);
                 create index if not exists recall_chunks_scope_created_idx on recall_chunks(scope, created_at desc);
                 create index if not exists recall_chunks_text_fts_idx on recall_chunks using gin (to_tsvector('simple', text));
                 create index if not exists project_items_project_kind_idx on project_items(project_id, kind, created_at desc);
                 create index if not exists project_items_source_session_kind_idx on project_items(source_session_id, kind, created_at desc);
                 create index if not exists session_events_actor_outputs_idx on session_events(session_id, seq) where artifact_uri is not null or history_uri is not null;
                 create index if not exists dream_queue_session_idx on dream_queue(session_id, enqueued_at asc);
                 create index if not exists dream_queue_scope_enqueued_idx on dream_queue(scope, enqueued_at desc);
                 create index if not exists dream_queue_ready_idx on dream_queue(status, available_at asc) where status = 'queued';
                 create index if not exists dream_queue_status_available_idx on dream_queue(status, available_at asc);
                 create index if not exists dream_queue_running_locked_idx on dream_queue(status, locked_at asc) where status = 'running';
                 create index if not exists memory_summaries_scope_updated_idx on memory_summaries(scope, updated_at desc);
                 create index if not exists memory_summaries_source_session_idx on memory_summaries(source_session_id, updated_at desc);
                 create index if not exists skill_proposals_source_session_idx on skill_proposals(source_session_id, updated_at desc);
                 create index if not exists skill_proposals_status_updated_idx on skill_proposals(status, updated_at desc);
                 create index if not exists cron_jobs_next_run_idx on cron_jobs(enabled, next_run_at);
                 create index if not exists cron_runs_job_started_idx on cron_runs(job_id, started_at desc);
                 create index if not exists sessions_updated_at_idx on sessions(updated_at desc);",
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(())
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

fn row_to_cron_job(row: tokio_postgres::Row) -> CronJobRecord {
    CronJobRecord {
        id: row.get("id"),
        name: row.get("name"),
        schedule: row.get("schedule"),
        enabled: row.get("enabled"),
        cron_mode: row.get("cron_mode"),
        max_turns: row.get("max_turns"),
        script_timeout_seconds: row.get("script_timeout_seconds"),
        next_run_at: row.get("next_run_at"),
        updated_at: row.get("updated_at"),
    }
}

fn row_to_cron_run(row: tokio_postgres::Row) -> CronRunRecord {
    CronRunRecord {
        id: row.get("id"),
        job_id: row.get("job_id"),
        scheduled_for: row.get("scheduled_for"),
        status: row.get("status"),
        session_id: row.get("session_id"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
        result_json: row.get("result_json"),
    }
}

fn row_to_memory_summary(row: tokio_postgres::Row) -> Result<MemorySummaryRecord> {
    let kind: String = row.get("kind");
    let evidence_json: Value = row.get("evidence_json");
    Ok(MemorySummaryRecord {
        id: row.get("id"),
        kind: kind
            .parse::<MemorySummaryKind>()
            .map_err(|err| ServerError::Store(err.to_string()))?,
        subject: row.get("subject"),
        scope: row.get("scope"),
        title: row.get("title"),
        body: row.get("body"),
        evidence: serde_json::from_value(evidence_json)
            .map_err(|err| ServerError::Store(err.to_string()))?,
        source_dream_id: row.get("source_dream_id"),
        source_session_id: row.get("source_session_id"),
        dedupe_key: row.get("dedupe_key"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn row_to_skill_proposal(row: tokio_postgres::Row) -> Result<SkillProposalRecord> {
    let evidence_json: Value = row.get("evidence_json");
    let verification_json: Value = row.get("verification_json");
    let status: String = row.get("status");
    Ok(SkillProposalRecord {
        id: row.get("id"),
        name: row.get("name"),
        description: row.get("description"),
        body: row.get("body"),
        trigger: row.get("trigger"),
        use_criteria: row.get("use_criteria"),
        evidence: serde_json::from_value(evidence_json)
            .map_err(|err| ServerError::Store(err.to_string()))?,
        self_critique: row.get("self_critique"),
        verification: serde_json::from_value::<SkillVerification>(verification_json)
            .map_err(|err| ServerError::Store(err.to_string()))?,
        status: status
            .parse::<SkillProposalStatus>()
            .map_err(|err| ServerError::Store(err.to_string()))?,
        dedupe_key: row.get("dedupe_key"),
        source_dream_id: row.get("source_dream_id"),
        source_session_id: row.get("source_session_id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn row_to_dream_record(row: tokio_postgres::Row) -> Result<DreamQueueRecord> {
    let reason: String = row.get("reason");
    let status: String = row.get("status");
    Ok(DreamQueueRecord {
        id: row.get("id"),
        session_id: row.get("session_id"),
        subject: row.get("subject"),
        scope: row.get("scope"),
        reason: reason
            .parse::<DreamReason>()
            .map_err(|err| ServerError::Store(err.to_string()))?,
        status: status
            .parse::<DreamStatus>()
            .map_err(|err| ServerError::Store(err.to_string()))?,
        dedupe_key: row.get("dedupe_key"),
        source_event_seq: row.get("source_event_seq"),
        attempts: row.get("attempts"),
        enqueued_at: row.get("enqueued_at"),
        available_at: row.get("available_at"),
        locked_at: row.get("locked_at"),
        last_error: row.get("last_error"),
    })
}

fn row_to_project_item(row: tokio_postgres::Row) -> Result<ProjectItemRecord> {
    let kind: String = row.get("kind");
    Ok(ProjectItemRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        kind: kind.parse()?,
        text: row.get("text"),
        target_uri: row.get("target_uri"),
        source_session_id: row.get("source_session_id"),
        source_event_seq: row.get("source_event_seq"),
        source_uri: row.get("source_uri"),
        dedupe_key: row.get("dedupe_key"),
        provenance_json: row.get("provenance_json"),
        created_at: row.get("created_at"),
    })
}

fn row_to_session_record(row: &tokio_postgres::Row) -> Result<SessionRecord> {
    let mode: Value = row.get("mode");
    let mode: tm_modes::ModeId =
        serde_json::from_value(mode).map_err(|err| ServerError::Store(err.to_string()))?;
    let mode_state: Option<Value> = row.get("mode_state_json");
    let mode_state = match mode_state {
        Some(value) => {
            serde_json::from_value(value).map_err(|err| ServerError::Store(err.to_string()))?
        }
        None => ModeState::new(mode.clone(), row.get("updated_at")),
    };
    let persona_status: Value = row.get("persona_status");
    Ok(SessionRecord {
        id: row.get("id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        status: row.get("status"),
        mode,
        mode_state,
        persona_status: serde_json::from_value(persona_status)
            .map_err(|err| ServerError::Store(err.to_string()))?,
    })
}

fn row_to_message_record(row: tokio_postgres::Row) -> MessageRecord {
    MessageRecord {
        session_id: row.get("session_id"),
        seq: row.get("seq"),
        role: row.get("role"),
        content: row.get("content"),
        created_at: row.get("created_at"),
    }
}

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use tokio_postgres::NoTls;
use uuid::Uuid;

use crate::{Result, ServerError};

use super::{
    MessageRecord, ModeState, NewProjectItem, NewSession, ProfileFactRecord, ProjectItemKind,
    ProjectItemRecord, RecallChunkRecord, SessionEvent, SessionRecord, SessionSummaryRecord, Store,
};

pub struct PostgresStore {
    client: tokio_postgres::Client,
}

impl PostgresStore {
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
                 create table if not exists project_items(id uuid primary key, project_id text not null, kind text not null, text text not null, target_uri text not null, source_session_id uuid not null references sessions(id) on delete cascade, source_event_seq bigint, source_uri text, dedupe_key text not null, provenance_json jsonb not null, created_at timestamptz not null, unique(project_id, kind, dedupe_key));
                 create index if not exists profile_facts_subject_idx on profile_facts(subject);
                 create index if not exists recall_chunks_scope_created_idx on recall_chunks(scope, created_at desc);
                 create index if not exists project_items_project_kind_idx on project_items(project_id, kind, created_at desc);
                 create index if not exists project_items_source_session_kind_idx on project_items(source_session_id, kind, created_at desc);
                 create index if not exists session_events_actor_outputs_idx on session_events(session_id, seq) where artifact_uri is not null or history_uri is not null;
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
                "insert into profile_facts (id, subject, predicate, object, confidence, provenance, valid_from, valid_to) values ($1, $2, $3, $4, $5, $6, $7, $8)",
                &[&fact.id, &fact.subject, &fact.predicate, &fact.object, &fact.confidence, &fact.provenance, &fact.valid_from, &fact.valid_to],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(())
    }

    async fn add_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<()> {
        self.client
            .execute(
                "insert into recall_chunks (id, scope, text, source, created_at) values ($1, $2, $3, $4, $5)",
                &[&chunk.id, &chunk.scope, &chunk.text, &chunk.source, &chunk.created_at],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(())
    }

    async fn upsert_profile_fact(&self, fact: ProfileFactRecord) -> Result<ProfileFactRecord> {
        let row = self
            .client
            .query_one(
                "insert into profile_facts (id, subject, predicate, object, confidence, provenance, valid_from, valid_to)
                 values ($1, $2, $3, $4, $5, $6, $7, $8)
                 on conflict (id) do update
                   set subject = excluded.subject,
                       predicate = excluded.predicate,
                       object = excluded.object,
                       confidence = excluded.confidence,
                       provenance = excluded.provenance,
                       valid_from = excluded.valid_from,
                       valid_to = excluded.valid_to
                 returning id, subject, predicate, object, confidence, provenance, valid_from, valid_to",
                &[&fact.id, &fact.subject, &fact.predicate, &fact.object, &fact.confidence, &fact.provenance, &fact.valid_from, &fact.valid_to],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(ProfileFactRecord {
            id: row.get("id"),
            subject: row.get("subject"),
            predicate: row.get("predicate"),
            object: row.get("object"),
            confidence: row.get("confidence"),
            provenance: row.get("provenance"),
            valid_from: row.get("valid_from"),
            valid_to: row.get("valid_to"),
        })
    }

    async fn upsert_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<RecallChunkRecord> {
        let row = self
            .client
            .query_one(
                "insert into recall_chunks (id, scope, text, source, created_at)
                 values ($1, $2, $3, $4, $5)
                 on conflict (id) do update
                   set scope = excluded.scope,
                       text = excluded.text,
                       source = excluded.source,
                       created_at = excluded.created_at
                 returning id, scope, text, source, created_at",
                &[
                    &chunk.id,
                    &chunk.scope,
                    &chunk.text,
                    &chunk.source,
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
            created_at: row.get("created_at"),
        })
    }

    async fn profile_facts(&self, subject: &str) -> Result<Vec<ProfileFactRecord>> {
        let rows = self.client
            .query("select id, subject, predicate, object, confidence, provenance, valid_from, valid_to from profile_facts where subject = $1 and valid_to is null order by valid_from desc", &[&subject])
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
                provenance: row.get("provenance"),
                valid_from: row.get("valid_from"),
                valid_to: row.get("valid_to"),
            })
            .collect())
    }

    async fn profile_fact(&self, subject: &str, id: Uuid) -> Result<ProfileFactRecord> {
        let row = self.client
            .query_opt("select id, subject, predicate, object, confidence, provenance, valid_from, valid_to from profile_facts where subject = $1 and id = $2 and valid_to is null", &[&subject, &id])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("profile fact {subject}/{id}")))?;
        Ok(ProfileFactRecord {
            id: row.get("id"),
            subject: row.get("subject"),
            predicate: row.get("predicate"),
            object: row.get("object"),
            confidence: row.get("confidence"),
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
        let rows = self.client
            .query("select id, scope, text, source, created_at from recall_chunks where scope = $1 and text ilike $2 order by created_at desc limit $3", &[&scope, &pattern, &(limit as i64)])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| RecallChunkRecord {
                id: row.get("id"),
                scope: row.get("scope"),
                text: row.get("text"),
                source: row.get("source"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    async fn recall_chunk(&self, scope: &str, id: Uuid) -> Result<RecallChunkRecord> {
        let row = self
            .client
            .query_opt(
                "select id, scope, text, source, created_at from recall_chunks where scope = $1 and id = $2",
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

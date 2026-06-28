use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_postgres::NoTls;
use uuid::Uuid;

use tm_persona::{Mode, PersonaStatus};

use crate::{Result, ServerError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: String,
    pub mode: Mode,
    pub persona_status: PersonaStatus,
}

#[derive(Debug, Clone)]
pub struct NewSession {
    pub mode: Mode,
    pub persona_status: PersonaStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionEvent {
    pub session_id: Uuid,
    pub seq: i64,
    pub event_type: String,
    pub payload_json: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageRecord {
    pub session_id: Uuid,
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileFactRecord {
    pub id: Uuid,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32,
    pub provenance: String,
    pub valid_from: DateTime<Utc>,
    pub valid_to: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecallChunkRecord {
    pub id: Uuid,
    pub scope: String,
    pub text: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait Store: Send + Sync + 'static {
    async fn create_session(&self, new: NewSession) -> Result<SessionRecord>;
    async fn get_session(&self, session_id: Uuid) -> Result<SessionRecord>;
    async fn append_message(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
    ) -> Result<MessageRecord>;
    async fn append_event(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
    ) -> Result<SessionEvent>;
    async fn events_after(
        &self,
        session_id: Uuid,
        last_event_id: Option<i64>,
    ) -> Result<Vec<SessionEvent>>;
    async fn add_profile_fact(&self, fact: ProfileFactRecord) -> Result<()>;
    async fn add_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<()>;
    async fn profile_facts(&self, subject: &str) -> Result<Vec<ProfileFactRecord>>;
    async fn recall_chunks(
        &self,
        scope: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecallChunkRecord>>;
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    sessions: BTreeMap<Uuid, SessionRecord>,
    events: BTreeMap<Uuid, Vec<SessionEvent>>,
    messages: BTreeMap<Uuid, Vec<MessageRecord>>,
    profile_facts: Vec<ProfileFactRecord>,
    recall_chunks: Vec<RecallChunkRecord>,
}

#[async_trait]
impl Store for InMemoryStore {
    async fn create_session(&self, new: NewSession) -> Result<SessionRecord> {
        let now = Utc::now();
        let session = SessionRecord {
            id: Uuid::new_v4(),
            created_at: now,
            updated_at: now,
            status: "open".to_string(),
            mode: new.mode,
            persona_status: new.persona_status,
        };
        let mut inner = self.inner.lock();
        inner.sessions.insert(session.id, session.clone());
        Ok(session)
    }

    async fn get_session(&self, session_id: Uuid) -> Result<SessionRecord> {
        self.inner
            .lock()
            .sessions
            .get(&session_id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))
    }

    async fn append_message(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
    ) -> Result<MessageRecord> {
        self.get_session(session_id).await?;
        let mut inner = self.inner.lock();
        let messages = inner.messages.entry(session_id).or_default();
        let seq = messages.len() as i64 + 1;
        let message = MessageRecord {
            session_id,
            seq,
            role: role.to_string(),
            content: content.to_string(),
            created_at: Utc::now(),
        };
        messages.push(message.clone());
        Ok(message)
    }

    async fn append_event(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
    ) -> Result<SessionEvent> {
        self.get_session(session_id).await?;
        let mut inner = self.inner.lock();
        let events = inner.events.entry(session_id).or_default();
        let seq = events.len() as i64 + 1;
        let event = SessionEvent {
            session_id,
            seq,
            event_type: event_type.to_string(),
            payload_json,
            created_at: Utc::now(),
        };
        events.push(event.clone());
        Ok(event)
    }

    async fn events_after(
        &self,
        session_id: Uuid,
        last_event_id: Option<i64>,
    ) -> Result<Vec<SessionEvent>> {
        self.get_session(session_id).await?;
        let min_seq = last_event_id.unwrap_or(0);
        Ok(self
            .inner
            .lock()
            .events
            .get(&session_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|event| event.seq > min_seq)
            .collect())
    }

    async fn add_profile_fact(&self, fact: ProfileFactRecord) -> Result<()> {
        self.inner.lock().profile_facts.push(fact);
        Ok(())
    }

    async fn add_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<()> {
        self.inner.lock().recall_chunks.push(chunk);
        Ok(())
    }

    async fn profile_facts(&self, subject: &str) -> Result<Vec<ProfileFactRecord>> {
        Ok(self
            .inner
            .lock()
            .profile_facts
            .iter()
            .filter(|fact| fact.subject == subject && fact.valid_to.is_none())
            .cloned()
            .collect())
    }

    async fn recall_chunks(
        &self,
        scope: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecallChunkRecord>> {
        let query = query.to_lowercase();
        Ok(self
            .inner
            .lock()
            .recall_chunks
            .iter()
            .filter(|chunk| chunk.scope == scope && chunk.text.to_lowercase().contains(&query))
            .take(limit)
            .cloned()
            .collect())
    }
}

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
                "create table if not exists sessions(id uuid primary key, created_at timestamptz not null, updated_at timestamptz not null, status text not null, mode jsonb not null, persona_status jsonb not null);
                 create table if not exists session_events(session_id uuid not null references sessions(id) on delete cascade, seq bigint not null, event_type text not null, payload_json jsonb not null, created_at timestamptz not null, primary key(session_id, seq));
                 create table if not exists messages(session_id uuid not null references sessions(id) on delete cascade, seq bigint not null, role text not null, content text not null, created_at timestamptz not null, primary key(session_id, seq));
                 create table if not exists profile_facts(id uuid primary key, subject text not null, predicate text not null, object text not null, confidence real not null, provenance text not null, valid_from timestamptz not null, valid_to timestamptz);
                 create table if not exists recall_chunks(id uuid primary key, scope text not null, text text not null, source text not null, created_at timestamptz not null);
                 create index if not exists profile_facts_subject_idx on profile_facts(subject);
                 create index if not exists recall_chunks_scope_created_idx on recall_chunks(scope, created_at desc);",
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
            mode: new.mode,
            persona_status: new.persona_status,
        };
        let mode = serde_json::to_value(session.mode)
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let persona_status = serde_json::to_value(&session.persona_status)
            .map_err(|err| ServerError::Store(err.to_string()))?;
        self.client
            .execute(
                "insert into sessions (id, created_at, updated_at, status, mode, persona_status) values ($1, $2, $3, $4, $5, $6)",
                &[&session.id, &session.created_at, &session.updated_at, &session.status, &mode, &persona_status],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(session)
    }

    async fn get_session(&self, session_id: Uuid) -> Result<SessionRecord> {
        let row = self.client
            .query_opt("select id, created_at, updated_at, status, mode, persona_status from sessions where id = $1", &[&session_id])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        let mode: Value = row.get("mode");
        let persona_status: Value = row.get("persona_status");
        Ok(SessionRecord {
            id: row.get("id"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
            status: row.get("status"),
            mode: serde_json::from_value(mode)
                .map_err(|err| ServerError::Store(err.to_string()))?,
            persona_status: serde_json::from_value(persona_status)
                .map_err(|err| ServerError::Store(err.to_string()))?,
        })
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
                "with lock as (select pg_advisory_xact_lock(hashtext($1))), next_seq as (select coalesce(max(seq) + 1, 1) as seq from messages, lock where session_id = $2) insert into messages (session_id, seq, role, content, created_at) select $2, next_seq.seq, $3, $4, now() from next_seq returning seq, created_at",
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
        let row = self
            .client
            .query_one(
                "with lock as (select pg_advisory_xact_lock(hashtext($1))), next_seq as (select coalesce(max(seq) + 1, 1) as seq from session_events, lock where session_id = $2) insert into session_events (session_id, seq, event_type, payload_json, created_at) select $2, next_seq.seq, $3, $4, now() from next_seq returning seq, created_at",
                &[&session_key, &session_id, &event_type, &payload_json],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(SessionEvent {
            session_id,
            seq: row.get("seq"),
            event_type: event_type.to_string(),
            payload_json,
            created_at: row.get("created_at"),
        })
    }

    async fn events_after(
        &self,
        session_id: Uuid,
        last_event_id: Option<i64>,
    ) -> Result<Vec<SessionEvent>> {
        self.get_session(session_id).await?;
        let min_seq = last_event_id.unwrap_or(0);
        let rows = self.client
            .query("select session_id, seq, event_type, payload_json, created_at from session_events where session_id = $1 and seq > $2 order by seq asc", &[&session_id, &min_seq])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| SessionEvent {
                session_id: row.get("session_id"),
                seq: row.get("seq"),
                event_type: row.get("event_type"),
                payload_json: row.get("payload_json"),
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

    async fn profile_facts(&self, subject: &str) -> Result<Vec<ProfileFactRecord>> {
        let rows = self.client
            .query("select id, subject, predicate, object, confidence, provenance, valid_from, valid_to from profile_facts where subject = $1 and valid_to is null", &[&subject])
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StoreEvent {
    Text {
        delta: String,
    },
    Mode {
        mode: Mode,
        label: String,
        voice_cap: String,
        persona_status: PersonaStatus,
    },
    Final {
        text: String,
    },
}

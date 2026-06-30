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
    pub mode_state: ModeState,
    pub persona_status: PersonaStatus,
}

#[derive(Debug, Clone)]
pub struct NewSession {
    pub mode: Mode,
    pub persona_status: PersonaStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModeState {
    pub mode: Mode,
    pub router_reason: Option<String>,
    pub lock_source: Option<String>,
    pub override_source: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl ModeState {
    pub fn new(mode: Mode, updated_at: DateTime<Utc>) -> Self {
        Self {
            mode,
            router_reason: None,
            lock_source: None,
            override_source: None,
            updated_at,
        }
    }

    pub fn label(&self) -> &'static str {
        self.mode.label()
    }

    pub fn voice_cap(&self) -> &'static str {
        self.mode.voice_cap()
    }
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ProjectItemKind {
    Status,
    Summary,
    OpenLoop,
    Decision,
    NextAction,
    Resource,
    Artifact,
    Workspace,
    Linked,
}

impl ProjectItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Summary => "summary",
            Self::OpenLoop => "open_loop",
            Self::Decision => "decision",
            Self::NextAction => "next_action",
            Self::Resource => "resource",
            Self::Artifact => "artifact",
            Self::Workspace => "workspace",
            Self::Linked => "linked",
        }
    }
}

impl std::str::FromStr for ProjectItemKind {
    type Err = ServerError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "status" => Ok(Self::Status),
            "summary" => Ok(Self::Summary),
            "open_loop" => Ok(Self::OpenLoop),
            "decision" => Ok(Self::Decision),
            "next_action" => Ok(Self::NextAction),
            "resource" => Ok(Self::Resource),
            "artifact" => Ok(Self::Artifact),
            "workspace" => Ok(Self::Workspace),
            "linked" => Ok(Self::Linked),
            other => Err(ServerError::Store(format!(
                "unknown project item kind {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectItemRecord {
    pub id: Uuid,
    pub project_id: String,
    pub kind: ProjectItemKind,
    pub text: String,
    pub target_uri: String,
    pub source_session_id: Uuid,
    pub source_event_seq: Option<i64>,
    pub source_uri: Option<String>,
    pub dedupe_key: String,
    pub provenance_json: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewProjectItem {
    pub project_id: String,
    pub kind: ProjectItemKind,
    pub text: String,
    pub target_uri: String,
    pub source_session_id: Uuid,
    pub source_event_seq: Option<i64>,
    pub source_uri: Option<String>,
    pub dedupe_key: String,
    pub provenance_json: Value,
}

#[async_trait]
pub trait Store: Send + Sync + 'static {
    async fn create_session(&self, new: NewSession) -> Result<SessionRecord>;
    async fn get_session(&self, session_id: Uuid) -> Result<SessionRecord>;
    async fn set_mode_state(
        &self,
        session_id: Uuid,
        mode_state: ModeState,
    ) -> Result<SessionRecord>;
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
    async fn upsert_project_item(&self, item: NewProjectItem) -> Result<ProjectItemRecord>;
    async fn project_items(
        &self,
        project_id: &str,
        kind: Option<ProjectItemKind>,
    ) -> Result<Vec<ProjectItemRecord>>;
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
    project_items: Vec<ProjectItemRecord>,
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
            mode_state: ModeState::new(new.mode, now),
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

    async fn set_mode_state(
        &self,
        session_id: Uuid,
        mode_state: ModeState,
    ) -> Result<SessionRecord> {
        let mut inner = self.inner.lock();
        let session = inner
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        session.mode = mode_state.mode;
        session.mode_state = mode_state;
        session.updated_at = Utc::now();
        Ok(session.clone())
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

    async fn upsert_project_item(&self, item: NewProjectItem) -> Result<ProjectItemRecord> {
        self.get_session(item.source_session_id).await?;
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.project_items.iter_mut().find(|existing| {
            existing.project_id == item.project_id
                && existing.kind == item.kind
                && existing.dedupe_key == item.dedupe_key
        }) {
            existing.text = item.text;
            existing.target_uri = item.target_uri;
            existing.source_event_seq = item.source_event_seq;
            existing.source_uri = item.source_uri;
            existing.provenance_json = item.provenance_json;
            return Ok(existing.clone());
        }
        let record = ProjectItemRecord {
            id: Uuid::new_v4(),
            project_id: item.project_id,
            kind: item.kind,
            text: item.text,
            target_uri: item.target_uri,
            source_session_id: item.source_session_id,
            source_event_seq: item.source_event_seq,
            source_uri: item.source_uri,
            dedupe_key: item.dedupe_key,
            provenance_json: item.provenance_json,
            created_at: Utc::now(),
        };
        inner.project_items.push(record.clone());
        Ok(record)
    }

    async fn project_items(
        &self,
        project_id: &str,
        kind: Option<ProjectItemKind>,
    ) -> Result<Vec<ProjectItemRecord>> {
        let mut items = self
            .inner
            .lock()
            .project_items
            .iter()
            .filter(|item| item.project_id == project_id)
            .filter(|item| kind.is_none_or(|kind| item.kind == kind))
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| item.created_at);
        Ok(items)
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
                "create table if not exists sessions(id uuid primary key, created_at timestamptz not null, updated_at timestamptz not null, status text not null, mode jsonb not null, mode_state_json jsonb, persona_status jsonb not null);
                 alter table sessions add column if not exists mode_state_json jsonb;
                 create table if not exists session_events(session_id uuid not null references sessions(id) on delete cascade, seq bigint not null, event_type text not null, payload_json jsonb not null, created_at timestamptz not null, primary key(session_id, seq));
                 create table if not exists messages(session_id uuid not null references sessions(id) on delete cascade, seq bigint not null, role text not null, content text not null, created_at timestamptz not null, primary key(session_id, seq));
                 create table if not exists profile_facts(id uuid primary key, subject text not null, predicate text not null, object text not null, confidence real not null, provenance text not null, valid_from timestamptz not null, valid_to timestamptz);
                 create table if not exists recall_chunks(id uuid primary key, scope text not null, text text not null, source text not null, created_at timestamptz not null);
                 create table if not exists project_items(id uuid primary key, project_id text not null, kind text not null, text text not null, target_uri text not null, source_session_id uuid not null references sessions(id) on delete cascade, source_event_seq bigint, source_uri text, dedupe_key text not null, provenance_json jsonb not null, created_at timestamptz not null, unique(project_id, kind, dedupe_key));
                 create index if not exists profile_facts_subject_idx on profile_facts(subject);
                 create index if not exists recall_chunks_scope_created_idx on recall_chunks(scope, created_at desc);
                 create index if not exists project_items_project_kind_idx on project_items(project_id, kind, created_at desc);",
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
            mode_state: ModeState::new(new.mode, Utc::now()),
            persona_status: new.persona_status,
        };
        let mode = serde_json::to_value(session.mode)
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

    async fn get_session(&self, session_id: Uuid) -> Result<SessionRecord> {
        let row = self.client
            .query_opt("select id, created_at, updated_at, status, mode, mode_state_json, persona_status from sessions where id = $1", &[&session_id])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        let mode: Value = row.get("mode");
        let mode =
            serde_json::from_value(mode).map_err(|err| ServerError::Store(err.to_string()))?;
        let mode_state: Option<Value> = row.get("mode_state_json");
        let mode_state = match mode_state {
            Some(value) => {
                serde_json::from_value(value).map_err(|err| ServerError::Store(err.to_string()))?
            }
            None => ModeState::new(mode, row.get("updated_at")),
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

    async fn set_mode_state(
        &self,
        session_id: Uuid,
        mode_state: ModeState,
    ) -> Result<SessionRecord> {
        self.get_session(session_id).await?;
        let mode = serde_json::to_value(mode_state.mode)
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StoreEvent {
    Text {
        delta: String,
    },
    ModeChanged {
        from: Option<Mode>,
        mode: Mode,
        label: String,
        voice_cap: String,
        router_reason: Option<String>,
        lock_source: Option<String>,
        override_source: Option<String>,
        updated_at: DateTime<Utc>,
        persona_status: PersonaStatus,
    },
    Final {
        text: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn postgres_test_dsn() -> Option<String> {
        (std::env::var("TM_POSTGRES_TESTS").ok().as_deref() == Some("1"))
            .then(|| {
                std::env::var("TM_TEST_DATABASE_URL")
                    .or_else(|_| std::env::var("TM_DATABASE_URL"))
                    .ok()
            })
            .flatten()
    }

    #[tokio::test]
    async fn gated_postgres_covers_replay_memory_approvals_and_project_refs() {
        let Some(dsn) = postgres_test_dsn() else {
            return;
        };
        let store = PostgresStore::connect(&dsn).await.unwrap();
        let session = store
            .create_session(NewSession {
                mode: Mode::PersonalAssistant,
                persona_status: PersonaStatus::Degraded {
                    warning: "test".to_string(),
                },
            })
            .await
            .unwrap();

        let mut mode_state = session.mode_state.clone();
        mode_state.mode = Mode::SeriousEngineer;
        mode_state.router_reason = Some("postgres replay test".to_string());
        mode_state.updated_at = Utc::now();
        let updated = store
            .set_mode_state(session.id, mode_state.clone())
            .await
            .unwrap();
        assert_eq!(updated.mode_state.mode, Mode::SeriousEngineer);

        store
            .append_event(
                session.id,
                "mode",
                serde_json::to_value(StoreEvent::ModeChanged {
                    from: Some(Mode::PersonalAssistant),
                    mode: Mode::SeriousEngineer,
                    label: Mode::SeriousEngineer.label().to_string(),
                    voice_cap: Mode::SeriousEngineer.voice_cap().to_string(),
                    router_reason: mode_state.router_reason,
                    lock_source: None,
                    override_source: None,
                    updated_at: Utc::now(),
                    persona_status: updated.persona_status,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .append_event(
                session.id,
                "approval",
                json!({"approvalId": Uuid::new_v4(), "backend": "postgres-test"}),
            )
            .await
            .unwrap();
        store
            .append_event(
                session.id,
                "approval_resolved",
                json!({"backend": "postgres-test", "outcome": "selected"}),
            )
            .await
            .unwrap();
        let replay = store.events_after(session.id, Some(1)).await.unwrap();
        assert_eq!(
            replay
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["approval", "approval_resolved"]
        );

        store
            .add_recall_chunk(RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: "project:postgres-test".to_string(),
                text: "memory row for P1 project view".to_string(),
                source: format!("session:{}", session.id),
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        let memory = store
            .recall_chunks("project:postgres-test", "P1 project", 5)
            .await
            .unwrap();
        assert_eq!(memory.len(), 1);

        let project_id = format!("postgres-test-{}", Uuid::new_v4());
        let item = store
            .upsert_project_item(NewProjectItem {
                project_id: project_id.clone(),
                kind: ProjectItemKind::Artifact,
                text: "artifact://0".to_string(),
                target_uri: format!("project://{project_id}/artifacts/0"),
                source_session_id: session.id,
                source_event_seq: Some(2),
                source_uri: Some("artifact://0".to_string()),
                dedupe_key: "artifact://0".to_string(),
                provenance_json: json!({
                    "sourceSession": session.id,
                    "sourceEventId": 2,
                    "sourceUri": "artifact://0",
                }),
            })
            .await
            .unwrap();
        let duplicate = store
            .upsert_project_item(NewProjectItem {
                project_id: project_id.clone(),
                kind: ProjectItemKind::Artifact,
                text: "artifact://0".to_string(),
                target_uri: format!("project://{project_id}/artifacts/0"),
                source_session_id: session.id,
                source_event_seq: Some(2),
                source_uri: Some("artifact://0".to_string()),
                dedupe_key: "artifact://0".to_string(),
                provenance_json: json!({"sourceSession": session.id}),
            })
            .await
            .unwrap();
        assert_eq!(item.id, duplicate.id);
        assert_eq!(
            store
                .project_items(&project_id, Some(ProjectItemKind::Artifact))
                .await
                .unwrap()
                .len(),
            1
        );
    }
}

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use serde_json::Value;
use uuid::Uuid;

use crate::{Result, ServerError};

use super::{
    MessageRecord, ModeState, NewProjectItem, NewSession, ProfileFactRecord, ProjectItemKind,
    ProjectItemRecord, RecallChunkRecord, SessionEvent, SessionRecord, SessionSummaryRecord, Store,
};

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
            mode: new.mode.clone(),
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

    async fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummaryRecord>> {
        let inner = self.inner.lock();
        let mut summaries = inner
            .sessions
            .values()
            .map(|session| {
                let messages = inner.messages.get(&session.id).cloned().unwrap_or_default();
                let title = messages
                    .iter()
                    .find(|message| message.role == "user")
                    .map(|message| message.content.clone());
                let project_summary = inner
                    .project_items
                    .iter()
                    .filter(|item| {
                        item.source_session_id == session.id
                            && item.kind == ProjectItemKind::Summary
                    })
                    .max_by_key(|item| item.created_at)
                    .map(|item| item.text.clone());
                let latest_assistant = messages
                    .iter()
                    .rev()
                    .find(|message| message.role == "assistant")
                    .map(|message| message.content.clone());
                let summary = project_summary.or(latest_assistant);
                let preview = messages.last().map(|message| message.content.clone());
                let last_event_id = inner
                    .events
                    .get(&session.id)
                    .and_then(|events| events.last())
                    .map(|event| event.seq);
                SessionSummaryRecord {
                    session: session.clone(),
                    summary,
                    title,
                    preview,
                    message_count: messages.len() as i64,
                    last_event_id,
                }
            })
            .collect::<Vec<_>>();
        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.session.updated_at));
        summaries.truncate(limit);
        Ok(summaries)
    }

    async fn session_messages(&self, session_id: Uuid) -> Result<Vec<MessageRecord>> {
        self.get_session(session_id).await?;
        Ok(self
            .inner
            .lock()
            .messages
            .get(&session_id)
            .cloned()
            .unwrap_or_default())
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
        session.mode = mode_state.mode.clone();
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
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            session.updated_at = message.created_at;
        }
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
        let event = SessionEvent::new(session_id, seq, event_type, payload_json, Utc::now());
        events.push(event.clone());
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            session.updated_at = event.created_at;
        }
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

    async fn upsert_profile_fact(&self, fact: ProfileFactRecord) -> Result<ProfileFactRecord> {
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .profile_facts
            .iter_mut()
            .find(|existing| existing.id == fact.id)
        {
            *existing = fact.clone();
            return Ok(existing.clone());
        }
        inner.profile_facts.push(fact.clone());
        Ok(fact)
    }

    async fn upsert_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<RecallChunkRecord> {
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .recall_chunks
            .iter_mut()
            .find(|existing| existing.id == chunk.id)
        {
            *existing = chunk.clone();
            return Ok(existing.clone());
        }
        inner.recall_chunks.push(chunk.clone());
        Ok(chunk)
    }

    async fn profile_facts(&self, subject: &str) -> Result<Vec<ProfileFactRecord>> {
        let mut facts = self
            .inner
            .lock()
            .profile_facts
            .iter()
            .filter(|fact| fact.subject == subject && fact.valid_to.is_none())
            .cloned()
            .collect::<Vec<_>>();
        facts.sort_by_key(|fact| std::cmp::Reverse(fact.valid_from));
        Ok(facts)
    }

    async fn profile_fact(&self, subject: &str, id: Uuid) -> Result<ProfileFactRecord> {
        self.inner
            .lock()
            .profile_facts
            .iter()
            .find(|fact| fact.subject == subject && fact.id == id && fact.valid_to.is_none())
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("profile fact {subject}/{id}")))
    }

    async fn recall_chunks(
        &self,
        scope: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecallChunkRecord>> {
        let query = query.to_lowercase();
        let mut chunks = self
            .inner
            .lock()
            .recall_chunks
            .iter()
            .filter(|chunk| chunk.scope == scope && chunk.text.to_lowercase().contains(&query))
            .cloned()
            .collect::<Vec<_>>();
        chunks.sort_by_key(|chunk| std::cmp::Reverse(chunk.created_at));
        chunks.truncate(limit);
        Ok(chunks)
    }

    async fn recall_chunk(&self, scope: &str, id: Uuid) -> Result<RecallChunkRecord> {
        self.inner
            .lock()
            .recall_chunks
            .iter()
            .find(|chunk| chunk.scope == scope && chunk.id == id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("recall chunk {scope}/{id}")))
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

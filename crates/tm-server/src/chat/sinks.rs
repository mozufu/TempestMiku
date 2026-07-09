use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use tm_core::EventSink;
use uuid::Uuid;

use crate::{CodingEventSink, Result, ServerError, SessionEvent, Store, StoreEvent};

pub struct PersistingEventSink<S> {
    session_id: Uuid,
    store: Arc<S>,
    sender: tokio::sync::broadcast::Sender<SessionEvent>,
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl<S> PersistingEventSink<S> {
    pub fn new(
        session_id: Uuid,
        store: Arc<S>,
        sender: tokio::sync::broadcast::Sender<SessionEvent>,
    ) -> Self {
        Self {
            session_id,
            store,
            sender,
            events: Mutex::new(Vec::new()),
        }
    }

    fn push_event(&self, event_type: &str, payload: serde_json::Value) {
        self.events
            .lock()
            .expect("event buffer lock poisoned")
            .push((event_type.to_string(), payload));
    }
}

impl<S> EventSink for PersistingEventSink<S>
where
    S: Store,
{
    fn on_text(&self, delta: &str) {
        self.push_event(
            "text",
            serde_json::to_value(StoreEvent::Text {
                delta: delta.to_string(),
            })
            .expect("text event serializes"),
        );
    }

    fn on_reasoning(&self, delta: &str) {
        self.push_event("reasoning", json!({ "delta": delta }));
    }

    fn on_tool_call(&self, name: &str) {
        self.push_event("tool_call", json!({ "name": name }));
    }

    fn on_cell_start(&self, code: &str) {
        self.push_event("cell_start", json!({ "code": code }));
    }

    fn on_cell_result(&self, shaped: &str) {
        self.push_event("cell_result", json!({ "shaped": shaped }));
    }

    fn on_final(&self, text: &str) {
        self.push_event(
            "final",
            serde_json::to_value(StoreEvent::Final {
                text: text.to_string(),
            })
            .expect("final event serializes"),
        );
    }
}

impl<S> PersistingEventSink<S>
where
    S: Store,
{
    pub async fn flush(&self) -> Result<()> {
        let pending = {
            let mut events = self.events.lock().expect("event buffer lock poisoned");
            std::mem::take(&mut *events)
        };
        for (event_type, payload) in pending {
            let event = self
                .store
                .append_event(self.session_id, &event_type, payload)
                .await?;
            let _ = self.sender.send(event);
        }
        Ok(())
    }
}

// ─── RosterCodingEventSink ───────────────────────────────────────────────────

/// Persists coding-style events through the actor roster's raw event hook.
///
/// This is used by child actor approval policies. Child actors run on their own
/// worker threads, but their user-visible approval prompts still belong to the
/// parent session event log.
pub struct RosterCodingEventSink {
    session_id: Uuid,
    roster: Arc<tm_agents::MailboxRegistry>,
}

impl RosterCodingEventSink {
    pub fn new(session_id: Uuid, roster: Arc<tm_agents::MailboxRegistry>) -> Self {
        Self { session_id, roster }
    }
}

#[async_trait]
impl CodingEventSink for RosterCodingEventSink {
    async fn emit(
        &self,
        event_type: &str,
        payload_json: serde_json::Value,
    ) -> Result<SessionEvent> {
        self.roster
            .emit_raw_event(
                &self.session_id.to_string(),
                event_type,
                payload_json.clone(),
            )
            .await
            .map_err(ServerError::Store)?;
        Ok(SessionEvent::new(
            self.session_id,
            0,
            event_type,
            payload_json,
            chrono::Utc::now(),
        ))
    }
}

use std::{
    marker::PhantomData,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::json;
use tm_core::{Error as CoreError, EventSink};
use tokio::{sync::mpsc, task::JoinHandle};
use uuid::Uuid;

use crate::{CodingEventSink, Result, ServerError, SessionEvent, Store, StoreEvent};

pub struct PersistingEventSink<S> {
    event_tx: Mutex<Option<mpsc::Sender<PendingEvent>>>,
    writer: Mutex<Option<JoinHandle<Result<()>>>>,
    failure: Arc<Mutex<Option<String>>>,
    store_type: PhantomData<fn() -> S>,
}

#[derive(Debug)]
struct PendingEvent {
    event_type: String,
    payload: serde_json::Value,
}

const EVENT_WRITER_CAPACITY: usize = 4_096;

impl<S> PersistingEventSink<S>
where
    S: Store,
{
    pub fn new(
        session_id: Uuid,
        store: Arc<S>,
        sender: tokio::sync::broadcast::Sender<SessionEvent>,
    ) -> Self {
        Self::for_turn(session_id, None, store, sender)
    }

    pub fn for_turn(
        session_id: Uuid,
        turn_id: Option<Uuid>,
        store: Arc<S>,
        sender: tokio::sync::broadcast::Sender<SessionEvent>,
    ) -> Self {
        let (event_tx, event_rx) = mpsc::channel(EVENT_WRITER_CAPACITY);
        let failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let writer_failure = Arc::clone(&failure);
        let writer = tokio::spawn(async move {
            let result = write_events(session_id, turn_id, store, sender, event_rx).await;
            if let Err(error) = &result {
                *writer_failure
                    .lock()
                    .expect("event writer failure lock poisoned") = Some(error.to_string());
            }
            result
        });
        Self {
            event_tx: Mutex::new(Some(event_tx)),
            writer: Mutex::new(Some(writer)),
            failure,
            store_type: PhantomData,
        }
    }

    fn push_event(&self, event_type: &str, payload: serde_json::Value) -> tm_core::Result<()> {
        if let Some(error) = self
            .failure
            .lock()
            .expect("event writer failure lock poisoned")
            .clone()
        {
            return Err(CoreError::EventSink(error));
        }
        let sender = self
            .event_tx
            .lock()
            .expect("event writer sender lock poisoned")
            .clone()
            .ok_or_else(|| CoreError::EventSink("event writer is closed".to_string()))?;
        let result = sender
            .try_send(PendingEvent {
                event_type: event_type.to_string(),
                payload,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => {
                    CoreError::EventSink("bounded event writer is full".to_string())
                }
                mpsc::error::TrySendError::Closed(_) => CoreError::EventSink(
                    self.failure
                        .lock()
                        .expect("event writer failure lock poisoned")
                        .clone()
                        .unwrap_or_else(|| "event writer stopped".to_string()),
                ),
            });
        if let Err(error) = &result {
            *self
                .failure
                .lock()
                .expect("event writer failure lock poisoned") = Some(error.to_string());
        }
        result
    }

    fn preserve_legacy_result(&self, result: tm_core::Result<()>) {
        if let Err(error) = result {
            *self
                .failure
                .lock()
                .expect("event writer failure lock poisoned") = Some(error.to_string());
        }
    }
}

impl<S> EventSink for PersistingEventSink<S>
where
    S: Store,
{
    fn on_text(&self, delta: &str) {
        self.preserve_legacy_result(self.try_on_text(delta));
    }

    fn try_on_text(&self, delta: &str) -> tm_core::Result<()> {
        self.push_event(
            "text",
            serde_json::to_value(StoreEvent::Text {
                delta: delta.to_string(),
            })
            .expect("text event serializes"),
        )
    }

    fn on_reasoning(&self, delta: &str) {
        self.preserve_legacy_result(self.try_on_reasoning(delta));
    }

    fn try_on_reasoning(&self, delta: &str) -> tm_core::Result<()> {
        self.push_event("reasoning", json!({ "delta": delta }))
    }

    fn on_tool_call(&self, name: &str) {
        self.preserve_legacy_result(self.try_on_tool_call(name));
    }

    fn try_on_tool_call(&self, name: &str) -> tm_core::Result<()> {
        self.push_event("tool_call", json!({ "name": name }))
    }

    fn on_cell_start(&self, code: &str) {
        self.preserve_legacy_result(self.try_on_cell_start(code));
    }

    fn try_on_cell_start(&self, code: &str) -> tm_core::Result<()> {
        self.push_event("cell_start", json!({ "code": code }))
    }

    fn on_cell_result(&self, shaped: &str) {
        self.preserve_legacy_result(self.try_on_cell_result(shaped));
    }

    fn try_on_cell_result(&self, shaped: &str) -> tm_core::Result<()> {
        self.push_event("cell_result", json!({ "shaped": shaped }))
    }

    fn on_runtime_reset(&self) {
        self.preserve_legacy_result(self.try_on_runtime_reset());
    }

    fn try_on_runtime_reset(&self) -> tm_core::Result<()> {
        self.push_event(
            "runtime_reset",
            json!({ "reason": "sandbox_runtime_reopened" }),
        )
    }

    fn on_runtime_event(&self, event_type: &str, payload: &serde_json::Value) {
        self.preserve_legacy_result(self.try_on_runtime_event(event_type, payload));
    }

    fn try_on_runtime_event(
        &self,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> tm_core::Result<()> {
        self.push_event(event_type, payload.clone())
    }

    fn on_final(&self, text: &str) {
        self.preserve_legacy_result(self.try_on_final(text));
    }

    fn try_on_final(&self, text: &str) -> tm_core::Result<()> {
        self.push_event(
            "final",
            serde_json::to_value(StoreEvent::Final {
                text: text.to_string(),
            })
            .expect("final event serializes"),
        )
    }
}

impl<S> PersistingEventSink<S>
where
    S: Store,
{
    pub async fn flush(&self) -> Result<()> {
        self.event_tx
            .lock()
            .expect("event writer sender lock poisoned")
            .take();
        let writer = self
            .writer
            .lock()
            .expect("event writer task lock poisoned")
            .take();
        if let Some(writer) = writer {
            writer.await.map_err(|error| {
                ServerError::Store(format!("event writer task failed: {error}"))
            })??;
        } else if let Some(error) = self
            .failure
            .lock()
            .expect("event writer failure lock poisoned")
            .clone()
        {
            return Err(ServerError::Store(error));
        }
        Ok(())
    }
}

async fn write_events<S: Store>(
    session_id: Uuid,
    turn_id: Option<Uuid>,
    store: Arc<S>,
    sender: tokio::sync::broadcast::Sender<SessionEvent>,
    mut events: mpsc::Receiver<PendingEvent>,
) -> Result<()> {
    let mut deferred: Option<PendingEvent> = None;
    loop {
        let next = match deferred.take() {
            Some(event) => Some(event),
            None => events.recv().await,
        };
        let Some(mut pending) = next else {
            break;
        };
        if pending.event_type == "text" {
            while let Ok(next) = events.try_recv() {
                if next.event_type != "text" {
                    deferred = Some(next);
                    break;
                }
                if let (Some(delta), Some(next_delta)) = (
                    pending
                        .payload
                        .get_mut("delta")
                        .and_then(|value| value.as_str()),
                    next.payload
                        .get("delta")
                        .and_then(serde_json::Value::as_str),
                ) {
                    let mut combined = delta.to_string();
                    combined.push_str(next_delta);
                    pending.payload["delta"] = serde_json::Value::String(combined);
                }
            }
        }
        let event = store
            .append_event_for_turn(session_id, &pending.event_type, pending.payload, turn_id)
            .await?;
        let _ = sender.send(event);
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use tm_modes::{AssetStatus, ModeId};

    use super::*;
    use crate::{InMemoryStore, NewSession};

    #[tokio::test]
    async fn bounded_writer_persists_and_broadcasts_before_flush() {
        let store = Arc::new(InMemoryStore::default());
        let session = store
            .create_session(NewSession {
                mode: ModeId::from("general"),
                persona_status: AssetStatus::Degraded {
                    warning: "test assets".to_string(),
                },
            })
            .await
            .unwrap();
        let (sender, mut receiver) = tokio::sync::broadcast::channel(8);
        let sink = PersistingEventSink::new(session.id, Arc::clone(&store), sender);

        sink.try_on_text("streamed").unwrap();
        let live = tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
            .await
            .expect("delta should be broadcast before flush")
            .unwrap();
        assert_eq!(live.event_type, "text");
        assert_eq!(live.payload_json["delta"], "streamed");
        assert_eq!(
            store.events_after(session.id, None).await.unwrap(),
            vec![live]
        );

        sink.flush().await.unwrap();
    }

    #[tokio::test]
    async fn structured_runtime_events_persist_in_replay_order() {
        let store = Arc::new(InMemoryStore::default());
        let session = store
            .create_session(NewSession {
                mode: ModeId::from("general"),
                persona_status: AssetStatus::Degraded {
                    warning: "test assets".into(),
                },
            })
            .await
            .unwrap();
        let (sender, _receiver) = tokio::sync::broadcast::channel(8);
        let sink = PersistingEventSink::new(session.id, Arc::clone(&store), sender);
        for (kind, payload) in [
            (
                "effect_start",
                json!({"cellId":"cell-1","nodeId":"cell-1-node-1","argsPreview":"[redacted]"}),
            ),
            (
                "effect_suspended",
                json!({"cellId":"cell-1","nodeId":"cell-1-node-1"}),
            ),
            (
                "effect_resumed",
                json!({"cellId":"cell-1","nodeId":"cell-1-node-1"}),
            ),
            (
                "effect_result",
                json!({"cellId":"cell-1","nodeId":"cell-1-node-1","status":"completed"}),
            ),
        ] {
            sink.try_on_runtime_event(kind, &payload).unwrap();
        }
        sink.flush().await.unwrap();
        let events = store.events_after(session.id, None).await.unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec![
                "effect_start",
                "effect_suspended",
                "effect_resumed",
                "effect_result"
            ]
        );
        assert!(events.windows(2).all(|pair| pair[0].seq + 1 == pair[1].seq));
        assert_eq!(events[0].payload_json["argsPreview"], "[redacted]");
    }
}

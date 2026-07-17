use std::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::json;
use tm_core::{Error as CoreError, EventSink};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{CodingEventSink, Result, ServerError, SessionEvent, Store, StoreEvent};

pub struct PersistingEventSink<S> {
    event_tx: Mutex<Option<mpsc::Sender<PendingEvent>>>,
    writer: Mutex<Option<JoinHandle<Result<()>>>>,
    failure: Arc<Mutex<Option<String>>>,
    store_type: PhantomData<fn() -> S>,
}

#[derive(Debug)]
enum PendingEvent {
    Event {
        event_type: String,
        payload: serde_json::Value,
        confirmation: Option<oneshot::Sender<std::result::Result<(), String>>>,
    },
    Barrier {
        confirmation: oneshot::Sender<std::result::Result<(), String>>,
    },
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
        self.push_event_with_confirmation(event_type, payload, None)
    }

    fn push_event_with_confirmation(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        confirmation: Option<oneshot::Sender<std::result::Result<(), String>>>,
    ) -> tm_core::Result<()> {
        self.push_pending(PendingEvent::Event {
            event_type: event_type.to_string(),
            payload,
            confirmation,
        })
    }

    fn push_pending(&self, pending: PendingEvent) -> tm_core::Result<()> {
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
        let result = sender.try_send(pending).map_err(|error| match error {
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

    // The core callbacks contain the complete model-authored source and shaped result. tm-lang
    // emits bounded, redacted events with the same event names through `try_on_runtime_event`, so
    // persisting these legacy payloads would both duplicate the cell and bypass that boundary.
    fn try_on_cell_start(&self, _code: &str) -> tm_core::Result<()> {
        Ok(())
    }

    fn try_on_cell_result(&self, _shaped: &str) -> tm_core::Result<()> {
        Ok(())
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

    fn try_on_runtime_reset_confirmed(
        &self,
    ) -> Pin<Box<dyn Future<Output = tm_core::Result<()>> + Send + '_>> {
        Box::pin(async move {
            let (confirmation, response) = oneshot::channel();
            self.push_event_with_confirmation(
                "runtime_reset",
                json!({ "reason": "sandbox_runtime_reopened" }),
                Some(confirmation),
            )?;
            response
                .await
                .map_err(|_| {
                    CoreError::EventSink(
                        self.failure
                            .lock()
                            .expect("event writer failure lock poisoned")
                            .clone()
                            .unwrap_or_else(|| "event writer stopped".to_string()),
                    )
                })?
                .map_err(CoreError::EventSink)
        })
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

    fn try_on_runtime_event_confirmed<'a>(
        &'a self,
        event_type: &'a str,
        payload: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = tm_core::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let (confirmation, response) = oneshot::channel();
            self.push_event_with_confirmation(event_type, payload.clone(), Some(confirmation))?;
            response
                .await
                .map_err(|_| {
                    CoreError::EventSink(
                        self.failure
                            .lock()
                            .expect("event writer failure lock poisoned")
                            .clone()
                            .unwrap_or_else(|| "event writer stopped".to_string()),
                    )
                })?
                .map_err(CoreError::EventSink)
        })
    }

    fn try_on_event_barrier_confirmed(
        &self,
    ) -> Pin<Box<dyn Future<Output = tm_core::Result<()>> + Send + '_>> {
        Box::pin(async move {
            let (confirmation, response) = oneshot::channel();
            self.push_pending(PendingEvent::Barrier { confirmation })?;
            response
                .await
                .map_err(|_| {
                    CoreError::EventSink(
                        self.failure
                            .lock()
                            .expect("event writer failure lock poisoned")
                            .clone()
                            .unwrap_or_else(|| "event writer stopped".to_string()),
                    )
                })?
                .map_err(CoreError::EventSink)
        })
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
        }
        if let Some(error) = self
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
        let Some(pending) = next else {
            break;
        };
        let (event_type, mut payload, confirmation) = match pending {
            PendingEvent::Barrier { confirmation } => {
                let _ = confirmation.send(Ok(()));
                continue;
            }
            PendingEvent::Event {
                event_type,
                payload,
                confirmation,
            } => (event_type, payload, confirmation),
        };
        if event_type == "text" && confirmation.is_none() {
            while let Ok(next) = events.try_recv() {
                if merge_unconfirmed_text(&mut payload, &next) {
                    continue;
                }
                deferred = Some(next);
                break;
            }
        }
        match store
            .append_event_for_turn(session_id, &event_type, payload, turn_id)
            .await
        {
            Ok(event) => {
                let _ = sender.send(event);
                if let Some(confirmation) = confirmation {
                    let _ = confirmation.send(Ok(()));
                }
            }
            Err(error) => {
                if let Some(confirmation) = confirmation {
                    let _ = confirmation.send(Err(error.to_string()));
                }
                return Err(error);
            }
        }
    }
    Ok(())
}

fn merge_unconfirmed_text(payload: &mut serde_json::Value, next: &PendingEvent) -> bool {
    let PendingEvent::Event {
        event_type,
        payload: next_payload,
        confirmation: None,
    } = next
    else {
        return false;
    };
    if event_type != "text" {
        return false;
    }
    let (Some(delta), Some(next_delta)) = (
        payload.get("delta").and_then(serde_json::Value::as_str),
        next_payload
            .get("delta")
            .and_then(serde_json::Value::as_str),
    ) else {
        return false;
    };
    let mut combined = String::with_capacity(delta.len().saturating_add(next_delta.len()));
    combined.push_str(delta);
    combined.push_str(next_delta);
    payload["delta"] = serde_json::Value::String(combined);
    true
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

    #[tokio::test]
    async fn confirmed_runtime_event_reports_store_failure_after_enqueue() {
        let store = Arc::new(InMemoryStore::default());
        let (sender, _receiver) = tokio::sync::broadcast::channel(8);
        let sink = PersistingEventSink::new(Uuid::nil(), store, sender);

        let error = sink
            .try_on_runtime_event_confirmed(
                "binding_committed",
                &json!({"cellId":"cell-1","bindingCount":1}),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("session"), "{error}");
        assert!(sink.flush().await.is_err());
    }

    #[tokio::test]
    async fn event_barrier_reports_prior_unconfirmed_store_failure() {
        let store = Arc::new(InMemoryStore::default());
        let (sender, _receiver) = tokio::sync::broadcast::channel(8);
        let sink = PersistingEventSink::new(Uuid::nil(), store, sender);

        sink.try_on_final("not durable yet").unwrap();
        let error = sink.try_on_event_barrier_confirmed().await.unwrap_err();

        assert!(error.to_string().contains("session"), "{error}");
        assert!(sink.flush().await.is_err());
    }

    #[tokio::test]
    async fn text_batching_preserves_confirmations_and_malformed_events() {
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
        let (event_tx, event_rx) = mpsc::channel(8);
        let (confirmation, response) = oneshot::channel();
        for pending in [
            PendingEvent::Event {
                event_type: "text".to_string(),
                payload: json!({"delta":"a"}),
                confirmation: None,
            },
            PendingEvent::Event {
                event_type: "text".to_string(),
                payload: json!({"delta":"b"}),
                confirmation: Some(confirmation),
            },
            PendingEvent::Event {
                event_type: "text".to_string(),
                payload: json!({"malformed":true}),
                confirmation: None,
            },
            PendingEvent::Event {
                event_type: "text".to_string(),
                payload: json!({"delta":"c"}),
                confirmation: None,
            },
        ] {
            event_tx.try_send(pending).unwrap();
        }
        drop(event_tx);

        write_events(session.id, None, Arc::clone(&store), sender, event_rx)
            .await
            .unwrap();
        response.await.unwrap().unwrap();
        let events = store.events_after(session.id, None).await.unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].payload_json["delta"], "a");
        assert_eq!(events[1].payload_json["delta"], "b");
        assert_eq!(events[2].payload_json["malformed"], true);
        assert_eq!(events[3].payload_json["delta"], "c");
    }

    #[tokio::test]
    async fn sensitive_cell_boundary_persists_only_structured_previews() {
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

        sink.try_on_cell_start("@fs.patch {patch: \"secret-source-value\"}")
            .unwrap();
        sink.try_on_cell_result("diff:\n+secret-result-value")
            .unwrap();
        sink.try_on_runtime_event(
            "cell_start",
            &json!({"cellId":"cell-1","sourcePreview":"[redacted]"}),
        )
        .unwrap();
        sink.try_on_runtime_event(
            "cell_result",
            &json!({"cellId":"cell-1","status":"completed","resultPreview":"[redacted]"}),
        )
        .unwrap();
        sink.flush().await.unwrap();

        let events = store.events_after(session.id, None).await.unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["cell_start", "cell_result"]
        );
        assert_eq!(events[0].payload_json["sourcePreview"], "[redacted]");
        assert_eq!(events[1].payload_json["resultPreview"], "[redacted]");
        let encoded = serde_json::to_string(&events).unwrap();
        assert!(!encoded.contains("secret-source-value"), "{encoded}");
        assert!(!encoded.contains("secret-result-value"), "{encoded}");
        assert!(!encoded.contains("\"code\""), "{encoded}");
        assert!(!encoded.contains("\"shaped\""), "{encoded}");
    }
}

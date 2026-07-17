use std::{future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;
use serde_json::{Value, json};
use tm_core::EventSink;
use tm_host::{HostError, HostEventSink};
use tokio::sync::mpsc;

use crate::{CodingEventSink, Result, ServerError, StoreEvent};

pub(super) enum NativeEvent {
    Reasoning(String),
    Text(String),
    ToolCall(String),
    RuntimeReset {
        persisted: Option<tokio::sync::oneshot::Sender<std::result::Result<(), String>>>,
    },
    Runtime {
        event_type: String,
        payload: Value,
        persisted: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    },
    Final(String),
}

pub(super) struct ForwardingEventSink {
    pub(super) sender: mpsc::Sender<NativeEvent>,
}

impl ForwardingEventSink {
    fn send(&self, event: NativeEvent) -> tm_core::Result<()> {
        self.sender.try_send(event).map_err(|error| {
            let message = match error {
                mpsc::error::TrySendError::Full(_) => {
                    "native tm bounded event channel is full".to_string()
                }
                mpsc::error::TrySendError::Closed(_) => {
                    "native tm event forwarder stopped".to_string()
                }
            };
            tm_core::Error::EventSink(message)
        })
    }
}

impl EventSink for ForwardingEventSink {
    fn on_text(&self, delta: &str) {
        let _ = self.try_on_text(delta);
    }

    fn try_on_text(&self, delta: &str) -> tm_core::Result<()> {
        self.send(NativeEvent::Text(delta.to_string()))
    }

    fn on_reasoning(&self, delta: &str) {
        let _ = self.try_on_reasoning(delta);
    }

    fn try_on_reasoning(&self, delta: &str) -> tm_core::Result<()> {
        self.send(NativeEvent::Reasoning(delta.to_string()))
    }

    fn on_tool_call(&self, name: &str) {
        let _ = self.try_on_tool_call(name);
    }

    fn try_on_tool_call(&self, name: &str) -> tm_core::Result<()> {
        self.send(NativeEvent::ToolCall(name.to_string()))
    }

    // Structured tm-lang events carry the bounded/redacted cell previews. Do not forward the
    // legacy core callbacks, which contain complete source and shaped results.
    fn try_on_cell_start(&self, _code: &str) -> tm_core::Result<()> {
        Ok(())
    }

    fn try_on_cell_result(&self, _shaped: &str) -> tm_core::Result<()> {
        Ok(())
    }

    fn on_runtime_reset(&self) {
        let _ = self.try_on_runtime_reset();
    }

    fn try_on_runtime_reset(&self) -> tm_core::Result<()> {
        self.send(NativeEvent::RuntimeReset { persisted: None })
    }

    fn try_on_runtime_reset_confirmed(
        &self,
    ) -> Pin<Box<dyn Future<Output = tm_core::Result<()>> + Send + '_>> {
        Box::pin(async move {
            let (persisted, response) = tokio::sync::oneshot::channel();
            self.send(NativeEvent::RuntimeReset {
                persisted: Some(persisted),
            })?;
            response
                .await
                .map_err(|_| {
                    tm_core::Error::EventSink("native tm event forwarder stopped".to_string())
                })?
                .map_err(tm_core::Error::EventSink)
        })
    }

    fn on_final(&self, text: &str) {
        let _ = self.try_on_final(text);
    }

    fn try_on_final(&self, text: &str) -> tm_core::Result<()> {
        self.send(NativeEvent::Final(text.to_string()))
    }
}

#[async_trait]
impl HostEventSink for ForwardingEventSink {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        let (persisted, response) = tokio::sync::oneshot::channel();
        self.send(NativeEvent::Runtime {
            event_type: event_type.to_string(),
            payload: payload_json,
            persisted,
        })
        .map_err(|error| HostError::HostCall(error.to_string()))?;
        response
            .await
            .map_err(|_| HostError::HostCall("native tm event forwarder stopped".into()))?
            .map_err(HostError::HostCall)
    }
}

pub(super) async fn forward_events(
    mut events: mpsc::Receiver<NativeEvent>,
    sink: Arc<dyn CodingEventSink>,
) -> Result<()> {
    while let Some(event) = events.recv().await {
        let (event_type, payload): (&str, Value) = match event {
            NativeEvent::Reasoning(delta) => ("reasoning", json!({ "delta": delta })),
            NativeEvent::Text(delta) => (
                "text",
                serde_json::to_value(StoreEvent::Text { delta })
                    .map_err(|err| ServerError::Store(err.to_string()))?,
            ),
            NativeEvent::ToolCall(name) => ("tool_call", json!({ "name": name })),
            NativeEvent::RuntimeReset { persisted } => {
                match sink
                    .emit(
                        "runtime_reset",
                        json!({ "reason": "sandbox_runtime_reopened" }),
                    )
                    .await
                {
                    Ok(_) => {
                        if let Some(persisted) = persisted {
                            let _ = persisted.send(Ok(()));
                        }
                    }
                    Err(error) => {
                        if let Some(persisted) = persisted {
                            let _ = persisted.send(Err(error.to_string()));
                        }
                        return Err(error);
                    }
                }
                continue;
            }
            NativeEvent::Runtime {
                event_type,
                payload,
                persisted,
            } => {
                match sink.emit(&event_type, payload).await {
                    Ok(_) => {
                        let _ = persisted.send(Ok(()));
                    }
                    Err(error) => {
                        let _ = persisted.send(Err(error.to_string()));
                        return Err(error);
                    }
                }
                continue;
            }
            NativeEvent::Final(text) => (
                "final",
                serde_json::to_value(StoreEvent::Final { text })
                    .map_err(|err| ServerError::Store(err.to_string()))?,
            ),
        };
        sink.emit(event_type, payload).await?;
    }
    Ok(())
}

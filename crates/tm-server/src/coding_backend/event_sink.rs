use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{Result, SessionEvent, Store};

use super::CodingEventSink;

pub struct StoreCodingEventSink<S> {
    session_id: Uuid,
    turn_id: Option<Uuid>,
    store: Arc<S>,
    sender: broadcast::Sender<SessionEvent>,
}

impl<S> StoreCodingEventSink<S> {
    pub fn new(session_id: Uuid, store: Arc<S>, sender: broadcast::Sender<SessionEvent>) -> Self {
        Self {
            session_id,
            turn_id: None,
            store,
            sender,
        }
    }

    pub fn for_turn(
        session_id: Uuid,
        turn_id: Uuid,
        store: Arc<S>,
        sender: broadcast::Sender<SessionEvent>,
    ) -> Self {
        Self {
            session_id,
            turn_id: Some(turn_id),
            store,
            sender,
        }
    }
}

#[async_trait]
impl<S> CodingEventSink for StoreCodingEventSink<S>
where
    S: Store,
{
    async fn emit(&self, event_type: &str, payload_json: Value) -> Result<SessionEvent> {
        let event = self
            .store
            .append_event_for_turn(self.session_id, event_type, payload_json, self.turn_id)
            .await?;
        let _ = self.sender.send(event.clone());
        Ok(event)
    }

    async fn publish_persisted(&self, event: SessionEvent) -> Result<()> {
        let _ = self.sender.send(event);
        Ok(())
    }

    fn turn_id(&self) -> Option<Uuid> {
        self.turn_id
    }
}

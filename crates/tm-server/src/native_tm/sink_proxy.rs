use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use tm_host::{HostError, HostEventSink};
use uuid::Uuid;

use crate::{CodingEventSink, Result, ServerError};

#[derive(Default)]
pub(super) struct SwappableCodingSink {
    target: Mutex<Option<Arc<dyn CodingEventSink>>>,
}

impl SwappableCodingSink {
    pub(super) fn bind(&self, sink: Arc<dyn CodingEventSink>) {
        *self.target.lock().expect("coding sink proxy lock poisoned") = Some(sink);
    }

    pub(super) fn clear(&self) {
        self.target
            .lock()
            .expect("coding sink proxy lock poisoned")
            .take();
    }
}

#[async_trait]
impl CodingEventSink for SwappableCodingSink {
    async fn emit(&self, event_type: &str, payload_json: Value) -> Result<crate::SessionEvent> {
        let target = self
            .target
            .lock()
            .expect("coding sink proxy lock poisoned")
            .clone()
            .ok_or_else(|| {
                ServerError::Store("native tm event sink has no active turn".to_string())
            })?;
        target.emit(event_type, payload_json).await
    }

    async fn publish_persisted(&self, event: crate::SessionEvent) -> Result<()> {
        let target = self
            .target
            .lock()
            .expect("coding sink proxy lock poisoned")
            .clone()
            .ok_or_else(|| {
                ServerError::Store("native tm event sink has no active turn".to_string())
            })?;
        target.publish_persisted(event).await
    }

    fn turn_id(&self) -> Option<Uuid> {
        let target = self
            .target
            .lock()
            .expect("coding sink proxy lock poisoned")
            .clone();
        target.and_then(|target| target.turn_id())
    }
}

#[derive(Default)]
pub(super) struct SwappableHostEventSink {
    target: Mutex<Option<Arc<dyn HostEventSink>>>,
}

impl SwappableHostEventSink {
    pub(super) fn bind(&self, sink: Arc<dyn HostEventSink>) {
        *self.target.lock().expect("host sink proxy lock poisoned") = Some(sink);
    }

    pub(super) fn clear(&self) {
        self.target
            .lock()
            .expect("host sink proxy lock poisoned")
            .take();
    }
}

#[async_trait]
impl HostEventSink for SwappableHostEventSink {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        let target = self
            .target
            .lock()
            .expect("host sink proxy lock poisoned")
            .clone()
            .ok_or_else(|| {
                HostError::HostCall("native tm host event sink has no active turn".into())
            })?;
        target.emit(event_type, payload_json).await
    }

    fn effect_scope_id(&self) -> Option<String> {
        self.target
            .lock()
            .expect("host sink proxy lock poisoned")
            .as_ref()
            .and_then(|target| target.effect_scope_id())
    }
}

use super::*;

use sha2::Digest;
use tm_host::{
    ApprovalDecision, ApprovalPolicy, CapabilityGrants, DefaultDenyApprovalPolicy, HostEventSink,
};

#[derive(Default)]
struct RecordingHostEventSink {
    events: parking_lot::Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl HostEventSink for RecordingHostEventSink {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        self.events
            .lock()
            .push((event_type.to_string(), payload_json));
        Ok(())
    }
}

impl RecordingHostEventSink {
    fn events(&self) -> Vec<(String, Value)> {
        self.events.lock().clone()
    }
}

struct StaticApproval(ApprovalDecision);

#[async_trait]
impl ApprovalPolicy for StaticApproval {
    async fn request(
        &self,
        _action: &str,
        _timeout: std::time::Duration,
    ) -> tm_host::Result<ApprovalDecision> {
        Ok(self.0)
    }
}

fn store() -> (tempfile::TempDir, InMemoryDriveStore) {
    let dir = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(dir.path(), "drive").unwrap();
    let store = InMemoryDriveStore::new(artifacts);
    (dir, store)
}

mod host_put;
mod links_events;
mod local_store;
mod organizer;
mod scope_research;

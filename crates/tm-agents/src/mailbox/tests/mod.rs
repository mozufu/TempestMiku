mod lifecycle;
mod messages;
mod roster;
mod session_isolation;

use chrono::Utc;

use crate::actor::{ActorBudget, ActorId, ActorRecord, ActorStatus};

pub(super) fn test_record(id: &str) -> ActorRecord {
    ActorRecord {
        id: ActorId::new(id).unwrap(),
        parent: None,
        status: ActorStatus::Running,
        mode: None,
        budget: ActorBudget::default(),
        spawned_at: Utc::now(),
        completed_at: None,
        cancelled: false,
        failure_reason: None,
        last_summary: None,
        artifact_uri: None,
        history_uri: None,
    }
}

pub(super) fn test_record_with_parent(id: &str, parent: &str) -> ActorRecord {
    ActorRecord {
        parent: Some(ActorId::new(parent).unwrap()),
        ..test_record(id)
    }
}

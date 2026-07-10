use std::{fmt, str::FromStr};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MemoryError {
    #[error("unknown dream reason {0}")]
    UnknownDreamReason(String),
    #[error("unknown dream status {0}")]
    UnknownDreamStatus(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DreamReason {
    SessionEnded,
    ManualReflect,
    Scheduled,
}

impl DreamReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionEnded => "session_ended",
            Self::ManualReflect => "manual_reflect",
            Self::Scheduled => "scheduled",
        }
    }
}

impl fmt::Display for DreamReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DreamReason {
    type Err = MemoryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "session_ended" => Ok(Self::SessionEnded),
            "manual_reflect" => Ok(Self::ManualReflect),
            "scheduled" => Ok(Self::Scheduled),
            other => Err(MemoryError::UnknownDreamReason(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DreamStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl DreamStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for DreamStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DreamStatus {
    type Err = MemoryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            other => Err(MemoryError::UnknownDreamStatus(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DreamQueueRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub subject: String,
    pub scope: String,
    pub reason: DreamReason,
    pub status: DreamStatus,
    pub dedupe_key: String,
    pub source_event_seq: Option<i64>,
    pub attempts: i32,
    pub enqueued_at: DateTime<Utc>,
    pub available_at: DateTime<Utc>,
    pub locked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub lease_owner: Option<Uuid>,
    #[serde(default)]
    pub lease_epoch: i64,
    #[serde(default)]
    pub heartbeat_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub error_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DreamLease {
    pub dream: DreamQueueRecord,
    pub owner_id: Uuid,
    pub epoch: i64,
}

impl DreamLease {
    pub fn new(dream: DreamQueueRecord, owner_id: Uuid, epoch: i64) -> Self {
        Self {
            dream,
            owner_id,
            epoch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDreamQueueRecord {
    pub session_id: Uuid,
    pub subject: String,
    pub scope: String,
    pub reason: DreamReason,
    pub dedupe_key: String,
    pub source_event_seq: Option<i64>,
    pub available_at: DateTime<Utc>,
}

impl NewDreamQueueRecord {
    pub fn session_end(
        session_id: Uuid,
        subject: impl Into<String>,
        scope: impl Into<String>,
        source_event_seq: Option<i64>,
    ) -> Self {
        let subject = clean_or_default(subject.into(), "brian");
        let scope = clean_or_default(scope.into(), "global");
        let reason = DreamReason::SessionEnded;
        let dedupe_key = format!("dream:{}:{session_id}", reason.as_str());
        Self {
            session_id,
            subject,
            scope,
            reason,
            dedupe_key,
            source_event_seq,
            available_at: Utc::now(),
        }
    }
}

fn clean_or_default(value: String, default: &str) -> String {
    let cleaned = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        default.to_string()
    } else {
        cleaned
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DreamWorkerReport {
    pub attempted: usize,
    pub completed: usize,
    pub proposals: usize,
}

#[async_trait]
pub trait DreamWorker: Send + Sync {
    async fn run_once(&self) -> DreamWorkerReport;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoopDreamWorker;

#[async_trait]
impl DreamWorker for NoopDreamWorker {
    async fn run_once(&self) -> DreamWorkerReport {
        DreamWorkerReport::default()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn session_end_record_has_stable_normalized_dedupe_key() {
        let session_id = Uuid::new_v4();

        let first = NewDreamQueueRecord::session_end(
            session_id,
            " Brian ",
            " project:TempestMiku ",
            Some(7),
        );
        let second =
            NewDreamQueueRecord::session_end(session_id, "brian", "project:tempestmiku", Some(8));

        assert_eq!(first.subject, "Brian");
        assert_eq!(first.scope, "project:TempestMiku");
        assert_eq!(first.reason, DreamReason::SessionEnded);
        assert_eq!(first.source_event_seq, Some(7));
        assert_eq!(first.dedupe_key, second.dedupe_key);
    }

    #[test]
    fn dream_record_serializes_public_wire_shape() {
        let now = Utc::now();
        let record = DreamQueueRecord {
            id: Uuid::nil(),
            session_id: Uuid::nil(),
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            status: DreamStatus::Queued,
            dedupe_key: "dream:session_ended:test".to_string(),
            source_event_seq: Some(3),
            attempts: 0,
            enqueued_at: now,
            available_at: now,
            locked_at: None,
            lease_owner: None,
            lease_epoch: 0,
            heartbeat_at: None,
            completed_at: None,
            error_at: None,
            last_error: None,
        };

        let value = serde_json::to_value(record).expect("serialize dream record");

        assert_eq!(value["sessionId"], json!(Uuid::nil()));
        assert_eq!(value["reason"], json!("session_ended"));
        assert_eq!(value["status"], json!("queued"));
        assert_eq!(value["sourceEventSeq"], json!(3));
        assert!(value.get("session_id").is_none());
    }

    #[test]
    fn reason_and_status_parse_store_values() {
        assert_eq!(
            "session_ended".parse::<DreamReason>().unwrap(),
            DreamReason::SessionEnded
        );
        assert_eq!(
            "queued".parse::<DreamStatus>().unwrap(),
            DreamStatus::Queued
        );
        assert!("unknown".parse::<DreamReason>().is_err());
        assert!("unknown".parse::<DreamStatus>().is_err());
    }

    #[tokio::test]
    async fn noop_worker_does_not_extract_or_complete_jobs() {
        let report = NoopDreamWorker.run_once().await;

        assert_eq!(report, DreamWorkerReport::default());
    }
}

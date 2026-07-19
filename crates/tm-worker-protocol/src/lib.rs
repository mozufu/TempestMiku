//! Versioned, authenticated wire contracts between the authoritative `tm-server` coordinator and
//! a bounded remote `tm-worker` host connector.

mod client;
mod signing;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tm_host::HostErrorPayload;
use uuid::Uuid;

pub use client::{RemoteWorkerConfig, RemoteWorkerConnector};
pub use signing::{
    RequestAuth, SignatureError, SigningKey, canonical_request, current_unix_seconds,
};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_REQUEST_BODY_BYTES: usize = 256 * 1024;
pub const MAX_CLOCK_SKEW_SECONDS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobRequest {
    pub protocol_version: u16,
    pub job_id: Uuid,
    pub worker_id: String,
    pub operation: WorkerOperation,
    pub authority: WorkerAuthority,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerOperation {
    Invoke {
        capability: String,
        args: Value,
    },
    ResourceRead {
        uri: String,
        selector: Option<String>,
    },
    ResourcePreview {
        uri: String,
    },
    ResourceList {
        uri: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerAuthority {
    pub session_id: String,
    pub actor_id: Option<String>,
    pub session_scope: Option<String>,
    pub grants: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    AwaitingApproval,
    Succeeded,
    Failed,
    Cancelled,
    Indeterminate,
}

impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Indeterminate
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobStatus {
    pub protocol_version: u16,
    pub job_id: Uuid,
    pub worker_id: String,
    pub state: JobState,
    pub action: Option<String>,
    pub action_sha256: Option<String>,
    pub result: Option<Value>,
    pub error: Option<HostErrorPayload>,
    #[serde(default)]
    pub events: Vec<WorkerEvent>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerEvent {
    pub event_type: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResolution {
    Approved,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveApprovalRequest {
    pub protocol_version: u16,
    pub action_sha256: String,
    pub resolution: ApprovalResolution,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthResponse {
    pub protocol_version: u16,
    pub worker_id: String,
    pub ready: bool,
}

pub fn validate_worker_id(worker_id: &str) -> Result<(), &'static str> {
    let valid = !worker_id.is_empty()
        && worker_id.len() <= 64
        && worker_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    valid
        .then_some(())
        .ok_or("worker_id must be 1-64 ASCII alphanumeric, '-' or '_'")
}

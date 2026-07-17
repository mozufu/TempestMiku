use std::{sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::coding_backend::CodingEventSink;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPrompt {
    pub action: String,
    pub scope: Value,
    pub options: Vec<ApprovalOption>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResolveDecision {
    Approve,
    Deny,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveApprovalRequest {
    pub decision: ApprovalResolveDecision,
    #[serde(default)]
    pub option_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Selected { option_id: String },
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Approved,
    Denied,
    TimedOut,
    Cancelled,
}

impl ApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailedApprovalOutcome {
    pub outcome: ApprovalOutcome,
    pub status: ApprovalStatus,
}

pub struct DurableApprovalSpec {
    pub session_id: Uuid,
    pub origin: String,
    pub prompt: ApprovalPrompt,
    pub timeout: Duration,
    pub effect_type: String,
    pub effect_payload_json: Value,
    pub resumable: bool,
    pub sink: Arc<dyn CodingEventSink>,
}

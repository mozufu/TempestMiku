use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::MemoryEvidenceRef;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillProposalStatus {
    Pending,
    Approved,
    Denied,
    TimedOut,
    Cancelled,
}

impl SkillProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for SkillProposalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown skill proposal status {0}")]
pub struct UnknownSkillProposalStatus(pub String);

impl std::str::FromStr for SkillProposalStatus {
    type Err = UnknownSkillProposalStatus;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "denied" => Ok(Self::Denied),
            "timed_out" => Ok(Self::TimedOut),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(UnknownSkillProposalStatus(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillVerification {
    pub passed: bool,
    pub checks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillProposalRecord {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub body: String,
    pub trigger: String,
    pub use_criteria: String,
    pub evidence: Vec<MemoryEvidenceRef>,
    pub self_critique: String,
    pub verification: SkillVerification,
    pub status: SkillProposalStatus,
    pub dedupe_key: String,
    pub source_dream_id: Uuid,
    pub source_session_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSkillProposalRecord {
    pub name: String,
    pub description: String,
    pub body: String,
    pub trigger: String,
    pub use_criteria: String,
    pub evidence: Vec<MemoryEvidenceRef>,
    pub self_critique: String,
    pub verification: SkillVerification,
    pub dedupe_key: String,
    pub source_dream_id: Uuid,
    pub source_session_id: Uuid,
}

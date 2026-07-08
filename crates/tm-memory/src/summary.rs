use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemorySummaryKind {
    Session,
    Reflection,
    Daily,
    Weekly,
    TopicProject,
}

impl MemorySummaryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Reflection => "reflection",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::TopicProject => "topic_project",
        }
    }
}

impl std::fmt::Display for MemorySummaryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown memory summary kind {0}")]
pub struct UnknownMemorySummaryKind(pub String);

impl std::str::FromStr for MemorySummaryKind {
    type Err = UnknownMemorySummaryKind;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "session" => Ok(Self::Session),
            "reflection" => Ok(Self::Reflection),
            "daily" => Ok(Self::Daily),
            "weekly" => Ok(Self::Weekly),
            "topic_project" => Ok(Self::TopicProject),
            other => Err(UnknownMemorySummaryKind(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEvidenceRef {
    pub session_id: Uuid,
    pub event_seq: Option<i64>,
    pub message_seq: Option<i64>,
    pub uri: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemorySummaryRecord {
    pub id: Uuid,
    pub kind: MemorySummaryKind,
    pub subject: String,
    pub scope: String,
    pub title: String,
    pub body: String,
    pub evidence: Vec<MemoryEvidenceRef>,
    pub source_dream_id: Uuid,
    pub source_session_id: Option<Uuid>,
    pub dedupe_key: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMemorySummaryRecord {
    pub kind: MemorySummaryKind,
    pub subject: String,
    pub scope: String,
    pub title: String,
    pub body: String,
    pub evidence: Vec<MemoryEvidenceRef>,
    pub source_dream_id: Uuid,
    pub source_session_id: Option<Uuid>,
    pub dedupe_key: String,
}

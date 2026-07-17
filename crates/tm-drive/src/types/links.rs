use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::common::initial_record_version;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveLinkPlan {
    pub alias: String,
    pub canonical_root: String,
    pub mode: String,
    pub linked_uri: String,
    pub memory_scope: String,
    pub project: String,
    #[serde(default)]
    pub requires_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveUnlinkResult {
    pub alias: String,
    pub canonical_root: String,
    pub linked_uri: String,
    pub memory_scope: String,
    pub revoked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DriveLinkStatus {
    #[default]
    Active,
    Revoked,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveLinkRecord {
    pub alias: String,
    #[serde(default = "initial_record_version")]
    pub version: u64,
    pub canonical_root: String,
    pub mode: String,
    pub linked_uri: String,
    pub memory_scope: String,
    pub project: String,
    #[serde(default)]
    pub status: DriveLinkStatus,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub revoked_at: Option<DateTime<Utc>>,
}

impl DriveLinkRecord {
    pub fn from_plan(plan: &DriveLinkPlan, now: DateTime<Utc>) -> Self {
        Self {
            alias: plan.alias.clone(),
            version: initial_record_version(),
            canonical_root: plan.canonical_root.clone(),
            mode: plan.mode.clone(),
            linked_uri: plan.linked_uri.clone(),
            memory_scope: plan.memory_scope.clone(),
            project: plan.project.clone(),
            status: DriveLinkStatus::Active,
            metadata: BTreeMap::new(),
            created_at: now,
            updated_at: now,
            revoked_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveCorrectionRecord {
    pub id: Uuid,
    #[serde(default = "initial_record_version")]
    pub version: u64,
    pub from: String,
    pub to: String,
    pub created_at: DateTime<Utc>,
}

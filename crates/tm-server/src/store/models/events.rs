use super::*;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StoreEvent {
    Text { delta: String },
    ModeChanged(Box<ModeChangedStoreEvent>),
    Final { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModeChangedStoreEvent {
    pub from: Option<ModeId>,
    pub mode: ModeId,
    pub label: String,
    pub capabilities: Vec<String>,
    #[serde(rename = "activeSkills")]
    pub active_skills: Vec<String>,
    pub router_reason: Option<String>,
    pub lock_source: Option<String>,
    pub override_source: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub persona_status: AssetStatus,
}

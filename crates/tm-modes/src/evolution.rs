use serde::{Deserialize, Serialize};

use crate::ModeId;

pub const REVIEW_PROPOSAL_WIRE_VERSION: u16 = 1;
pub const MAX_REVIEW_PROPOSAL_CHANGES: usize = 16;
pub const MAX_REVIEW_METADATA_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ReviewProposalTarget {
    Persona { persona_id: String },
    Mode { mode_id: ModeId },
}

impl ReviewProposalTarget {
    pub fn id(&self) -> &str {
        match self {
            Self::Persona { persona_id } => persona_id,
            Self::Mode { mode_id } => mode_id.as_str(),
        }
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Persona { .. } => "persona",
            Self::Mode { .. } => "mode",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAddendumSection {
    BehaviorGuidance,
    VoiceGuidance,
    ToneGuidance,
    AddressGuidance,
    InteractionPreference,
    Description,
    RoutingGuidance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewMetadata {
    pub label: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewAddendumChange {
    pub section: ReviewAddendumSection,
    pub before: Option<ReviewMetadata>,
    pub after: ReviewMetadata,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewProposalStatus {
    Pending,
    Approved,
    Denied,
    TimedOut,
    Cancelled,
}

impl ReviewProposalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for ReviewProposalStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewApplyContract {
    Disabled,
    VersionedModeAddendum,
    VersionedPersonaAddendum,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn typed_targets_reject_raw_patch_fields() {
        let raw = json!({
            "kind": "persona",
            "persona_id": "miku",
            "path": "SOUL.md",
        });
        assert!(serde_json::from_value::<ReviewProposalTarget>(raw).is_err());
    }

    #[test]
    fn review_contract_serializes_explicit_apply_authority() {
        assert_eq!(
            serde_json::to_value(ReviewApplyContract::Disabled).unwrap(),
            json!("disabled")
        );
        assert_eq!(
            serde_json::to_value(ReviewApplyContract::VersionedModeAddendum).unwrap(),
            json!("versioned_mode_addendum")
        );
        assert_eq!(
            serde_json::to_value(ReviewApplyContract::VersionedPersonaAddendum).unwrap(),
            json!("versioned_persona_addendum")
        );
    }
}

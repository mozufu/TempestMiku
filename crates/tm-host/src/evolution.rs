use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, de};
use uuid::Uuid;

pub const EVOLUTION_WIRE_VERSION: u16 = 1;
pub const MAX_EVOLUTION_ACTOR_ID_BYTES: usize = 128;
pub const MAX_EVOLUTION_TARGET_ID_BYTES: usize = 128;
pub const MAX_EVOLUTION_PREVIEW_BYTES: usize = 1_024;
pub const MAX_EVOLUTION_RESOURCE_URI_BYTES: usize = 2_048;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfEvolutionTier {
    Off,
    #[default]
    Conservative,
    Moderate,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SelfEvolutionConfig {
    #[serde(default)]
    pub tier: SelfEvolutionTier,
}

impl SelfEvolutionTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Conservative => "conservative",
            Self::Moderate => "moderate",
        }
    }
}

impl fmt::Display for SelfEvolutionTier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelfEvolutionTierParseError {
    #[error("self-evolution tier aggressive is unsupported in P7.0")]
    UnsupportedAggressive,
    #[error("unknown self-evolution tier {0}")]
    Unknown(String),
}

impl FromStr for SelfEvolutionTier {
    type Err = SelfEvolutionTierParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off),
            "conservative" => Ok(Self::Conservative),
            "moderate" => Ok(Self::Moderate),
            "aggressive" => Err(SelfEvolutionTierParseError::UnsupportedAggressive),
            other => Err(SelfEvolutionTierParseError::Unknown(other.to_string())),
        }
    }
}

impl<'de> Deserialize<'de> for SelfEvolutionTier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionTargetClass {
    ProfileFact,
    ScopedMemory,
    SkillProposal,
    PersonaProposal,
    ModeProposal,
}

impl EvolutionTargetClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProfileFact => "profile_fact",
            Self::ScopedMemory => "scoped_memory",
            Self::SkillProposal => "skill_proposal",
            Self::PersonaProposal => "persona_proposal",
            Self::ModeProposal => "mode_proposal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct EvolutionTargetId(String);

impl EvolutionTargetId {
    pub fn new(value: impl Into<String>) -> Result<Self, EvolutionWireError> {
        let value = value.into();
        validate_bounded_text("target id", &value, MAX_EVOLUTION_TARGET_ID_BYTES)?;
        if value == "."
            || value == ".."
            || value.contains('/')
            || value.contains('\\')
            || value.contains(':')
            || value.chars().any(char::is_control)
        {
            return Err(EvolutionWireError::InvalidTargetId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for EvolutionTargetId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionTarget {
    pub class: EvolutionTargetClass,
    pub id: EvolutionTargetId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionPolicyOutcome {
    Allowed,
    ApprovalRequired,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvolutionPolicyReason {
    #[serde(rename = "evolution_disabled")]
    DisabledTier,
    #[serde(rename = "evolution_insufficient_tier")]
    InsufficientTier,
    #[serde(rename = "evolution_aggressive_unsupported")]
    UnsupportedAggressive,
    #[serde(rename = "evolution_unknown_target")]
    UnknownTarget,
    #[serde(rename = "evolution_approval_required")]
    ApprovalRequired,
    #[serde(rename = "evolution_stale_approval")]
    StaleApproval,
    #[serde(rename = "evolution_invalid_payload")]
    InvalidPayload,
}

impl EvolutionPolicyReason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::DisabledTier => "evolution_disabled",
            Self::InsufficientTier => "evolution_insufficient_tier",
            Self::UnsupportedAggressive => "evolution_aggressive_unsupported",
            Self::UnknownTarget => "evolution_unknown_target",
            Self::ApprovalRequired => "evolution_approval_required",
            Self::StaleApproval => "evolution_stale_approval",
            Self::InvalidPayload => "evolution_invalid_payload",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionPolicyDecision {
    pub outcome: EvolutionPolicyOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<EvolutionPolicyReason>,
}

impl EvolutionPolicyDecision {
    pub const fn allowed() -> Self {
        Self {
            outcome: EvolutionPolicyOutcome::Allowed,
            reason: None,
        }
    }

    pub const fn approval_required() -> Self {
        Self {
            outcome: EvolutionPolicyOutcome::ApprovalRequired,
            reason: Some(EvolutionPolicyReason::ApprovalRequired),
        }
    }

    pub const fn denied(reason: EvolutionPolicyReason) -> Self {
        Self {
            outcome: EvolutionPolicyOutcome::Denied,
            reason: Some(reason),
        }
    }
}

pub const fn decide_evolution_target(
    tier: SelfEvolutionTier,
    target: EvolutionTargetClass,
) -> EvolutionPolicyDecision {
    match (tier, target) {
        (SelfEvolutionTier::Off, _) => {
            EvolutionPolicyDecision::denied(EvolutionPolicyReason::DisabledTier)
        }
        (
            SelfEvolutionTier::Conservative | SelfEvolutionTier::Moderate,
            EvolutionTargetClass::ProfileFact
            | EvolutionTargetClass::ScopedMemory
            | EvolutionTargetClass::SkillProposal,
        ) => EvolutionPolicyDecision::allowed(),
        (
            SelfEvolutionTier::Moderate,
            EvolutionTargetClass::PersonaProposal | EvolutionTargetClass::ModeProposal,
        ) => EvolutionPolicyDecision::approval_required(),
        (
            SelfEvolutionTier::Conservative,
            EvolutionTargetClass::PersonaProposal | EvolutionTargetClass::ModeProposal,
        ) => EvolutionPolicyDecision::denied(EvolutionPolicyReason::InsufficientTier),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct EvolutionDigest(String);

impl EvolutionDigest {
    pub fn new(value: impl Into<String>) -> Result<Self, EvolutionWireError> {
        let value = value.into();
        let Some(hex) = value.strip_prefix("sha256:") else {
            return Err(EvolutionWireError::InvalidDigest);
        };
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(EvolutionWireError::InvalidDigest);
        }
        Ok(Self(format!("sha256:{}", hex.to_ascii_lowercase())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for EvolutionDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct EvolutionResourceUri(String);

impl EvolutionResourceUri {
    pub fn new(value: impl Into<String>) -> Result<Self, EvolutionWireError> {
        let value = value.into();
        validate_bounded_text("resource uri", &value, MAX_EVOLUTION_RESOURCE_URI_BYTES)?;
        if !matches_safe_resource_scheme(&value) || value.chars().any(char::is_control) {
            return Err(EvolutionWireError::InvalidResourceUri);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for EvolutionResourceUri {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct EvolutionPreview(String);

impl EvolutionPreview {
    pub fn new(value: impl Into<String>) -> Result<Self, EvolutionWireError> {
        let value = value.into();
        if value.len() > MAX_EVOLUTION_PREVIEW_BYTES {
            return Err(EvolutionWireError::FieldTooLarge {
                field: "preview",
                max_bytes: MAX_EVOLUTION_PREVIEW_BYTES,
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for EvolutionPreview {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionContentRef {
    pub digest: EvolutionDigest,
    pub resource_uri: EvolutionResourceUri,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct EvolutionActorId(String);

impl EvolutionActorId {
    pub fn new(value: impl Into<String>) -> Result<Self, EvolutionWireError> {
        let value = value.into();
        validate_bounded_text("actor id", &value, MAX_EVOLUTION_ACTOR_ID_BYTES)?;
        if value.chars().any(char::is_control) {
            return Err(EvolutionWireError::InvalidActorId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for EvolutionActorId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionOrigin {
    pub actor_id: EvolutionActorId,
    pub session_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dream_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionProposalEnvelope {
    pub version: u16,
    pub proposal_id: Uuid,
    pub origin: EvolutionOrigin,
    pub target: EvolutionTarget,
    pub content: EvolutionContentRef,
    pub preview: EvolutionPreview,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionAuditStatus {
    Attempted,
    Denied,
    AwaitingApproval,
    Approved,
    TimedOut,
    Superseded,
    Failed,
    Applied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionAuditRecord {
    pub version: u16,
    pub id: Uuid,
    pub proposal_id: Uuid,
    pub origin: EvolutionOrigin,
    pub target: EvolutionTarget,
    pub content_digest: EvolutionDigest,
    pub configured_tier: SelfEvolutionTier,
    pub decision: EvolutionPolicyDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_id: Option<Uuid>,
    pub status: EvolutionAuditStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<EvolutionPolicyReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvolutionWireError {
    #[error("invalid evolution actor id")]
    InvalidActorId,
    #[error("invalid evolution target id")]
    InvalidTargetId,
    #[error("evolution digest must be sha256 followed by 64 hexadecimal characters")]
    InvalidDigest,
    #[error("evolution resource uri must use memory, artifact, or blob sha256")]
    InvalidResourceUri,
    #[error("{field} must not be empty")]
    EmptyField { field: &'static str },
    #[error("{field} exceeds {max_bytes} bytes")]
    FieldTooLarge {
        field: &'static str,
        max_bytes: usize,
    },
}

fn validate_bounded_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), EvolutionWireError> {
    if value.is_empty() {
        return Err(EvolutionWireError::EmptyField { field });
    }
    if value.len() > max_bytes {
        return Err(EvolutionWireError::FieldTooLarge { field, max_bytes });
    }
    Ok(())
}

fn matches_safe_resource_scheme(value: &str) -> bool {
    value.starts_with("memory://")
        || value.starts_with("artifact://")
        || value
            .strip_prefix("blob:sha256:")
            .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    #[test]
    fn tier_wire_rejects_aggressive_and_unknown_values() {
        assert_eq!(
            SelfEvolutionTier::default(),
            SelfEvolutionTier::Conservative
        );
        for (wire, expected) in [
            ("off", SelfEvolutionTier::Off),
            ("conservative", SelfEvolutionTier::Conservative),
            ("moderate", SelfEvolutionTier::Moderate),
        ] {
            assert_eq!(
                serde_json::from_value::<SelfEvolutionTier>(json!(wire)).unwrap(),
                expected
            );
            assert_eq!(serde_json::to_value(expected).unwrap(), json!(wire));
        }

        let aggressive = serde_json::from_value::<SelfEvolutionTier>(json!("aggressive"))
            .unwrap_err()
            .to_string();
        assert!(aggressive.contains("unsupported in P7.0"));
        assert!(serde_json::from_value::<SelfEvolutionTier>(json!("future")).is_err());
    }

    #[test]
    fn config_defaults_conservative_and_rejects_aggressive() {
        assert_eq!(
            serde_json::from_value::<SelfEvolutionConfig>(json!({}))
                .unwrap()
                .tier,
            SelfEvolutionTier::Conservative
        );
        assert_eq!(
            serde_json::from_value::<SelfEvolutionConfig>(json!({ "tier": "off" }))
                .unwrap()
                .tier,
            SelfEvolutionTier::Off
        );
        assert!(
            serde_json::from_value::<SelfEvolutionConfig>(json!({ "tier": "aggressive" }))
                .unwrap_err()
                .to_string()
                .contains("unsupported in P7.0")
        );

        let host: crate::P0HostConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(host.self_evolution.tier, SelfEvolutionTier::Conservative);
        assert!(
            serde_json::from_value::<crate::P0HostConfig>(json!({
                "self_evolution": { "tier": "aggressive" }
            }))
            .unwrap_err()
            .to_string()
            .contains("unsupported in P7.0")
        );
    }

    #[test]
    fn tier_target_matrix_is_exhaustive_and_stable() {
        use EvolutionPolicyOutcome::{Allowed, ApprovalRequired, Denied};
        use EvolutionTargetClass::{
            ModeProposal, PersonaProposal, ProfileFact, ScopedMemory, SkillProposal,
        };
        use SelfEvolutionTier::{Conservative, Moderate, Off};

        let cases = [
            (Off, ProfileFact, Denied),
            (Off, ScopedMemory, Denied),
            (Off, SkillProposal, Denied),
            (Off, PersonaProposal, Denied),
            (Off, ModeProposal, Denied),
            (Conservative, ProfileFact, Allowed),
            (Conservative, ScopedMemory, Allowed),
            (Conservative, SkillProposal, Allowed),
            (Conservative, PersonaProposal, Denied),
            (Conservative, ModeProposal, Denied),
            (Moderate, ProfileFact, Allowed),
            (Moderate, ScopedMemory, Allowed),
            (Moderate, SkillProposal, Allowed),
            (Moderate, PersonaProposal, ApprovalRequired),
            (Moderate, ModeProposal, ApprovalRequired),
        ];
        for (tier, target, outcome) in cases {
            assert_eq!(decide_evolution_target(tier, target).outcome, outcome);
        }
    }

    #[test]
    fn target_wire_rejects_unknown_classes_and_path_authority() {
        assert!(
            serde_json::from_value::<EvolutionTarget>(json!({
                "class": "source_code",
                "id": "server"
            }))
            .is_err()
        );
        for id in ["../SOUL.md", "modes/general", "C:\\config", "file:secret"] {
            assert!(
                serde_json::from_value::<EvolutionTarget>(json!({
                    "class": "profile_fact",
                    "id": id
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn proposal_envelope_is_bounded_and_forward_field_compatible() {
        let value = json!({
            "version": EVOLUTION_WIRE_VERSION,
            "proposalId": Uuid::nil(),
            "origin": {
                "actorId": "dream-worker",
                "sessionId": Uuid::nil(),
                "dreamId": Uuid::nil(),
                "futureOriginField": true
            },
            "target": { "class": "skill_proposal", "id": "weekly-ledger" },
            "content": {
                "digest": format!("sha256:{}", "a".repeat(64)),
                "resourceUri": "memory://skill-proposals/00000000-0000-0000-0000-000000000000",
                "sizeBytes": 42,
                "futureContentField": "ignored"
            },
            "preview": "bounded preview",
            "createdAt": "2026-07-13T00:00:00Z",
            "futureEnvelopeField": { "ignored": true }
        });
        let envelope: EvolutionProposalEnvelope = serde_json::from_value(value).unwrap();
        assert_eq!(envelope.preview.as_str(), "bounded preview");
        assert_eq!(envelope.target.id.as_str(), "weekly-ledger");

        let mut oversized = serde_json::to_value(&envelope).unwrap();
        oversized["preview"] = json!("x".repeat(MAX_EVOLUTION_PREVIEW_BYTES + 1));
        assert!(serde_json::from_value::<EvolutionProposalEnvelope>(oversized).is_err());

        let mut unsafe_uri = serde_json::to_value(&envelope).unwrap();
        unsafe_uri["content"]["resourceUri"] = json!("file:///tmp/secret");
        assert!(serde_json::from_value::<EvolutionProposalEnvelope>(unsafe_uri).is_err());
    }

    #[test]
    fn audit_record_round_trips_without_candidate_content() {
        let timestamp = Utc.with_ymd_and_hms(2026, 7, 13, 0, 0, 0).unwrap();
        let record = EvolutionAuditRecord {
            version: EVOLUTION_WIRE_VERSION,
            id: Uuid::nil(),
            proposal_id: Uuid::nil(),
            origin: EvolutionOrigin {
                actor_id: EvolutionActorId::new("dream-worker").unwrap(),
                session_id: Uuid::nil(),
                dream_id: Some(Uuid::nil()),
            },
            target: EvolutionTarget {
                class: EvolutionTargetClass::PersonaProposal,
                id: EvolutionTargetId::new("general-addendum").unwrap(),
            },
            content_digest: EvolutionDigest::new(format!("sha256:{}", "b".repeat(64))).unwrap(),
            configured_tier: SelfEvolutionTier::Moderate,
            decision: EvolutionPolicyDecision::approval_required(),
            approval_id: Some(Uuid::nil()),
            effect_id: None,
            status: EvolutionAuditStatus::AwaitingApproval,
            created_at: timestamp,
            updated_at: timestamp,
            error_code: None,
        };
        let wire = serde_json::to_value(&record).unwrap();
        assert!(wire.get("body").is_none());
        assert!(wire.get("content").is_none());
        assert_eq!(
            wire["decision"]["reason"],
            json!("evolution_approval_required")
        );
        assert_eq!(
            serde_json::from_value::<EvolutionAuditRecord>(wire).unwrap(),
            record
        );
    }

    #[test]
    fn policy_reason_codes_are_stable() {
        let cases = [
            (EvolutionPolicyReason::DisabledTier, "evolution_disabled"),
            (
                EvolutionPolicyReason::InsufficientTier,
                "evolution_insufficient_tier",
            ),
            (
                EvolutionPolicyReason::UnsupportedAggressive,
                "evolution_aggressive_unsupported",
            ),
            (
                EvolutionPolicyReason::UnknownTarget,
                "evolution_unknown_target",
            ),
            (
                EvolutionPolicyReason::ApprovalRequired,
                "evolution_approval_required",
            ),
            (
                EvolutionPolicyReason::StaleApproval,
                "evolution_stale_approval",
            ),
            (
                EvolutionPolicyReason::InvalidPayload,
                "evolution_invalid_payload",
            ),
        ];
        for (reason, code) in cases {
            assert_eq!(reason.code(), code);
        }
    }
}

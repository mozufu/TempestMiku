use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
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

pub const MAX_SKILL_PROPOSAL_BODY_BYTES: usize = 64 * 1024;
pub const MAX_SKILL_PROPOSAL_REFERENCES: usize = 64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillConflictPolicy {
    RejectNameCollision,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillRollbackContract {
    AtomicVersionPointer,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillCatalogReloadContract {
    OnNextLoad,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillProposalLifecycle {
    pub version: u16,
    pub normalized_name: String,
    pub content_digest: String,
    pub source_dream_id: Uuid,
    pub source_session_id: Uuid,
    pub conflict_policy: SkillConflictPolicy,
    pub rollback: SkillRollbackContract,
    pub catalog_reload: SkillCatalogReloadContract,
    pub reviewable: bool,
    pub installable: bool,
    pub violations: Vec<String>,
}

pub fn skill_proposal_lifecycle(proposal: &SkillProposalRecord) -> SkillProposalLifecycle {
    skill_lifecycle(
        &proposal.name,
        &proposal.body,
        proposal.verification.passed,
        proposal.source_dream_id,
        proposal.source_session_id,
    )
}

pub fn new_skill_proposal_lifecycle(proposal: &NewSkillProposalRecord) -> SkillProposalLifecycle {
    skill_lifecycle(
        &proposal.name,
        &proposal.body,
        proposal.verification.passed,
        proposal.source_dream_id,
        proposal.source_session_id,
    )
}

fn skill_lifecycle(
    name: &str,
    body: &str,
    verification_passed: bool,
    source_dream_id: Uuid,
    source_session_id: Uuid,
) -> SkillProposalLifecycle {
    let normalized_name = normalize_skill_name(name);
    let mut violations = Vec::new();
    if normalized_name != name || normalized_name.is_empty() || normalized_name.len() > 64 {
        violations.push("invalid_normalized_name".to_string());
    }
    if body.len() > MAX_SKILL_PROPOSAL_BODY_BYTES {
        violations.push("body_too_large".to_string());
    }
    if !body.starts_with(&format!("# {name}\n")) {
        violations.push("title_mismatch".to_string());
    }
    for (heading, code) in [
        ("## Trigger", "missing_trigger"),
        ("## Procedure", "missing_procedure"),
        ("## Guardrails", "missing_guardrails"),
    ] {
        if !body.contains(heading) {
            violations.push(code.to_string());
        }
    }
    validate_skill_references(body, &mut violations);
    validate_skill_authority(body, &mut violations);
    if !verification_passed {
        violations.push("verification_failed".to_string());
    }

    let reviewable = violations.is_empty();
    SkillProposalLifecycle {
        version: 2,
        normalized_name,
        content_digest: format!("sha256:{:x}", Sha256::digest(body.as_bytes())),
        source_dream_id,
        source_session_id,
        conflict_policy: SkillConflictPolicy::RejectNameCollision,
        rollback: SkillRollbackContract::AtomicVersionPointer,
        catalog_reload: SkillCatalogReloadContract::OnNextLoad,
        reviewable,
        installable: reviewable,
        violations,
    }
}

fn normalize_skill_name(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn validate_skill_references(body: &str, violations: &mut Vec<String>) {
    let references = body
        .match_indices("](")
        .filter_map(|(start, _)| {
            let rest = &body[start + 2..];
            rest.find(')').map(|end| &rest[..end])
        })
        .collect::<Vec<_>>();
    if references.len() > MAX_SKILL_PROPOSAL_REFERENCES {
        violations.push("too_many_references".to_string());
    }
    if references.iter().any(|reference| {
        reference.is_empty()
            || reference.starts_with('/')
            || reference.contains("..")
            || reference.contains('\\')
            || reference.contains(':')
            || reference.chars().any(char::is_control)
    }) {
        violations.push("unsafe_reference".to_string());
    }
}

fn validate_skill_authority(body: &str, violations: &mut Vec<String>) {
    let prohibited = [
        "soul.md",
        "capability configuration",
        "fs.write",
        "code.edit",
        "proc.run",
        "secrets.",
        "install automatically",
        "activate automatically",
        "reload the skill",
    ];
    let has_prohibited_authority = body.lines().any(|line| {
        let line = line.trim().to_ascii_lowercase();
        !line.starts_with("- do not ")
            && !line.starts_with("do not ")
            && prohibited.iter().any(|needle| line.contains(needle))
    });
    if has_prohibited_authority {
        violations.push("prohibited_authority".to_string());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    fn proposal(body: String) -> NewSkillProposalRecord {
        NewSkillProposalRecord {
            name: "safe-workflow".to_string(),
            description: "A safe workflow".to_string(),
            body,
            trigger: "When requested".to_string(),
            use_criteria: "Only when requested".to_string(),
            evidence: Vec::new(),
            self_critique: "Narrow and reviewable".to_string(),
            verification: SkillVerification {
                passed: true,
                checks: vec!["shape:pass".to_string()],
            },
            dedupe_key: "skill:safe-workflow".to_string(),
            source_dream_id: Uuid::nil(),
            source_session_id: Uuid::nil(),
        }
    }

    #[test]
    fn valid_skill_is_installable_with_versioned_reload_contract() {
        let lifecycle = new_skill_proposal_lifecycle(&proposal(
            "# safe-workflow\n\n## Trigger\nNow\n\n## Procedure\n- Review.\n\n## Guardrails\n- Do not edit SOUL.md.\n"
                .to_string(),
        ));
        assert!(lifecycle.reviewable);
        assert!(lifecycle.installable);
        assert_eq!(
            lifecycle.catalog_reload,
            SkillCatalogReloadContract::OnNextLoad
        );
        assert_eq!(
            lifecycle.rollback,
            SkillRollbackContract::AtomicVersionPointer
        );
        assert!(lifecycle.content_digest.starts_with("sha256:"));
    }

    #[test]
    fn unsafe_paths_authority_and_oversized_bodies_are_not_reviewable() {
        let body = format!(
            "# safe-workflow\n\n## Trigger\nNow\n\n## Procedure\n[read](/etc/passwd)\nUse fs.write.\n\n## Guardrails\n{}",
            "x".repeat(MAX_SKILL_PROPOSAL_BODY_BYTES)
        );
        let lifecycle = new_skill_proposal_lifecycle(&proposal(body));
        assert!(!lifecycle.reviewable);
        assert!(!lifecycle.installable);
        assert!(
            lifecycle
                .violations
                .contains(&"unsafe_reference".to_string())
        );
        assert!(
            lifecycle
                .violations
                .contains(&"prohibited_authority".to_string())
        );
        assert!(lifecycle.violations.contains(&"body_too_large".to_string()));
    }
}

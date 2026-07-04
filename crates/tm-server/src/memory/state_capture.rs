use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::Result;

use super::MemoryWriteProposal;
use super::util::normalize_for_dedupe;

const MAX_STATE_CAPTURE_PROPOSALS_PER_TURN: usize = 6;
const STATE_CAPTURE_MAX_TEXT_CHARS: usize = 280;
pub(crate) const STATE_CAPTURE_PROVENANCE_LABEL: &str = "personal-assistant-state-capture";
/// The mode id this state capture belongs to; must resolve in the bundled `ModeCatalog`.
const MODE_ID: &str = "personal_assistant";

pub fn personal_assistant_state_capture_proposals(
    subject: &str,
    scope: &str,
    session_id: Uuid,
    user_content: &str,
    created_at: DateTime<Utc>,
) -> Result<Vec<MemoryWriteProposal>> {
    if should_skip_state_capture_input(user_content) {
        return Ok(Vec::new());
    }

    let source = format!("session:{session_id}:state-capture");
    let mut seen = BTreeSet::new();
    let mut proposals = Vec::new();
    for unit in state_capture_units(user_content) {
        if proposals.len() >= MAX_STATE_CAPTURE_PROPOSALS_PER_TURN {
            break;
        }
        if should_skip_state_capture_unit(&unit) {
            continue;
        }
        let Some(candidate) = state_capture_candidate(&unit) else {
            continue;
        };
        let dedupe = format!(
            "{}:{}",
            candidate.category.as_str(),
            normalize_for_dedupe(candidate.body.as_str())
        );
        if !seen.insert(dedupe) {
            continue;
        }
        let provenance = json!({
            "label": STATE_CAPTURE_PROVENANCE_LABEL,
            "source": source,
            "sourceSession": session_id,
            "sourceTurn": "user",
            "mode": MODE_ID,
            "scope": scope,
            "capturedCategory": candidate.category.as_str(),
            "proposedAt": created_at,
        });
        match candidate.category {
            StateCaptureCategory::StablePreference => {
                proposals.push(MemoryWriteProposal::profile_fact(
                    subject.to_string(),
                    "prefers".to_string(),
                    candidate.body,
                    0.82,
                    source.clone(),
                    STATE_CAPTURE_PROVENANCE_LABEL.to_string(),
                    provenance,
                    created_at,
                )?);
            }
            _ => {
                proposals.push(MemoryWriteProposal::recall_chunk(
                    subject.to_string(),
                    scope.to_string(),
                    candidate.recall_text(),
                    source.clone(),
                    STATE_CAPTURE_PROVENANCE_LABEL.to_string(),
                    provenance,
                    created_at,
                )?);
            }
        }
    }
    Ok(proposals)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateCaptureCategory {
    StablePreference,
    PersonalReminder,
    ActiveProjectOpenLoop,
    CommitmentDeadline,
    Decision,
    ShippedArtifact,
    ReusableWorkflow,
    RecurringBlindSpot,
}

impl StateCaptureCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::StablePreference => "stable_preference",
            Self::PersonalReminder => "personal_reminder",
            Self::ActiveProjectOpenLoop => "active_project_open_loop",
            Self::CommitmentDeadline => "commitment_deadline",
            Self::Decision => "decision",
            Self::ShippedArtifact => "shipped_artifact",
            Self::ReusableWorkflow => "reusable_workflow",
            Self::RecurringBlindSpot => "recurring_blind_spot",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::StablePreference => "Preference",
            Self::PersonalReminder => "Reminder",
            Self::ActiveProjectOpenLoop => "Open loop",
            Self::CommitmentDeadline => "Commitment/deadline",
            Self::Decision => "Decision",
            Self::ShippedArtifact => "Shipped artifact",
            Self::ReusableWorkflow => "Reusable workflow",
            Self::RecurringBlindSpot => "Recurring blind spot",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StateCaptureCandidate {
    category: StateCaptureCategory,
    body: String,
}

impl StateCaptureCandidate {
    fn recall_text(&self) -> String {
        format!("{}: {}", self.category.label(), self.body)
    }
}

fn state_capture_candidate(unit: &str) -> Option<StateCaptureCandidate> {
    let lower = unit.to_lowercase();
    if let Some(body) = extract_preference_body(unit) {
        return Some(StateCaptureCandidate {
            category: StateCaptureCategory::StablePreference,
            body,
        });
    }
    if let Some(body) = extract_reminder_body(unit) {
        return Some(StateCaptureCandidate {
            category: StateCaptureCategory::PersonalReminder,
            body,
        });
    }
    if contains_any(
        &lower,
        &[
            "workflow:",
            "playbook:",
            "reusable workflow",
            "reuse this workflow",
            "process:",
            "my process is",
            "our process is",
            "when i ask",
        ],
    ) {
        return recall_candidate(StateCaptureCategory::ReusableWorkflow, unit);
    }
    if contains_commitment_deadline_signal(&lower)
        || (contains_any(&lower, &["i will ", "i need to ", "i have to "])
            && contains_any(&lower, &[" by ", " before ", " deadline", " due"]))
    {
        return recall_candidate(StateCaptureCategory::CommitmentDeadline, unit);
    }
    if contains_any(
        &lower,
        &[
            "decision:",
            "decided:",
            "i decided",
            "we decided",
            "final decision",
            "decision is",
            "going with",
        ],
    ) {
        return recall_candidate(StateCaptureCategory::Decision, unit);
    }
    if contains_any(
        &lower,
        &[
            "shipped",
            "released",
            "launched",
            "published",
            "delivered",
            "artifact://",
            "workspace://session",
        ],
    ) {
        return recall_candidate(StateCaptureCategory::ShippedArtifact, unit);
    }
    if contains_any(
        &lower,
        &[
            "open loop",
            "todo:",
            "to-do:",
            "follow up",
            "follow-up",
            "active project",
            "currently working on",
            "i'm working on",
            "i am working on",
            "track this",
        ],
    ) {
        return recall_candidate(StateCaptureCategory::ActiveProjectOpenLoop, unit);
    }
    if contains_any(
        &lower,
        &[
            "recurring blind spot",
            "i keep forgetting",
            "i keep missing",
            "i often forget",
        ],
    ) {
        return recall_candidate(StateCaptureCategory::RecurringBlindSpot, unit);
    }
    None
}

fn recall_candidate(category: StateCaptureCategory, unit: &str) -> Option<StateCaptureCandidate> {
    let body = clean_state_capture_body(unit)?;
    Some(StateCaptureCandidate { category, body })
}

fn extract_preference_body(unit: &str) -> Option<String> {
    let lower = unit.to_lowercase();
    for marker in [
        "please remember that i prefer ",
        "please remember i prefer ",
        "remember that i prefer ",
        "remember i prefer ",
        "my preference is ",
        "my default is ",
        "i prefer ",
        "i'd rather ",
        "i would rather ",
    ] {
        if let Some(index) = lower.find(marker) {
            let start = index + marker.len();
            return clean_state_capture_body(&unit[start..]);
        }
    }
    None
}

fn extract_reminder_body(unit: &str) -> Option<String> {
    let lower = unit.to_lowercase();
    for marker in [
        "please remind me to ",
        "remind me to ",
        "reminder: ",
        "personal reminder: ",
        "don't let me forget to ",
        "do not let me forget to ",
        "don't let me forget ",
        "do not let me forget ",
        "i need to remember to ",
    ] {
        if let Some(index) = lower.find(marker) {
            let start = index + marker.len();
            return clean_state_capture_body(&unit[start..]);
        }
    }
    None
}

fn should_skip_state_capture_input(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty()
        || trimmed.chars().count() > 2_000
        || trimmed.lines().count() > 20
        || contains_secret_signal(trimmed)
        || contains_sensitive_pii_signal(trimmed)
        || looks_like_raw_log(trimmed)
        || ((!has_durable_capture_signal(trimmed))
            && (contains_transient_mood(trimmed) || contains_one_off_complaint(trimmed)))
}

fn should_skip_state_capture_unit(unit: &str) -> bool {
    contains_secret_signal(unit)
        || contains_sensitive_pii_signal(unit)
        || looks_like_raw_log(unit)
        || looks_like_project_command(unit)
        || ((!has_durable_capture_signal(unit))
            && (contains_transient_mood(unit) || contains_one_off_complaint(unit)))
}

fn has_durable_capture_signal(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "remember",
            "remind me",
            "reminder:",
            "personal reminder",
            "don't let me forget",
            "do not let me forget",
            "i prefer",
            "preference",
            "default is",
            "open loop",
            "todo:",
            "to-do:",
            "follow up",
            "follow-up",
            "active project",
            "working on",
            "commitment",
            "deadline",
            "due:",
            "due by",
            "due on",
            "due tomorrow",
            "due monday",
            "due tuesday",
            "due wednesday",
            "due thursday",
            "due friday",
            "due saturday",
            "due sunday",
            " by ",
            "decision",
            "decided",
            "shipped",
            "released",
            "launched",
            "published",
            "delivered",
            "artifact://",
            "workflow",
            "playbook",
            "process:",
            "recurring blind spot",
            "i keep forgetting",
            "i keep missing",
        ],
    )
}

fn contains_commitment_deadline_signal(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "deadline:",
            "due:",
            "commitment:",
            "i promised",
            "i committed",
            "due by",
            "due on",
            "due tomorrow",
            "due monday",
            "due tuesday",
            "due wednesday",
            "due thursday",
            "due friday",
            "due saturday",
            "due sunday",
            " by tomorrow",
            " by monday",
            " by tuesday",
            " by wednesday",
            " by thursday",
            " by friday",
            " by saturday",
            " by sunday",
        ],
    )
}

fn contains_secret_signal(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "password",
            "passphrase",
            "api key",
            "apikey",
            "access token",
            "auth token",
            "bearer ",
            "secret",
            "private key",
            "begin private key",
            "credential",
            "oauth",
        ],
    )
}

fn contains_sensitive_pii_signal(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "ssn",
            "social security",
            "passport",
            "driver license",
            "driver's license",
            "home address",
            "phone number",
            "date of birth",
            "birthdate",
            "bank account",
            "credit card",
        ],
    )
}

fn looks_like_raw_log(text: &str) -> bool {
    if text.contains("```") {
        return true;
    }
    let mut log_markers = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();
        if trimmed.starts_with("at ")
            || trimmed.starts_with("thread '")
            || lower.contains("traceback")
            || lower.contains("stack backtrace")
            || lower.contains("error:")
            || lower.contains("warn:")
            || lower.contains("info:")
            || lower.contains("debug:")
            || lower.contains("exception")
            || trimmed.starts_with("[INFO")
            || trimmed.starts_with("[WARN")
            || trimmed.starts_with("[ERROR")
            || trimmed.starts_with("[DEBUG")
            || trimmed
                .get(0..4)
                .is_some_and(|prefix| prefix.chars().all(|c| c.is_ascii_digit()))
                && trimmed.get(4..5) == Some("-")
        {
            log_markers += 1;
        }
    }
    log_markers >= 2
}

fn looks_like_project_command(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "cargo ", "npm ", "pnpm ", "yarn ", "git ", "make ", "docker ", "kubectl ", "deno ",
            "python ",
        ],
    )
}

fn contains_transient_mood(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "i feel ",
            "i'm sad",
            "i am sad",
            "i'm tired",
            "i am tired",
            "i'm exhausted",
            "i am exhausted",
            "overwhelmed",
            "spiraling",
            "self-deprecating",
            "i'm useless",
            "i am useless",
            "grumpy",
            "bad mood",
        ],
    )
}

fn contains_one_off_complaint(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "one-off complaint",
            "just venting",
            "rant:",
            "i hate ",
            "that sucked",
            "was annoying",
            "is annoying",
            "annoyed me",
        ],
    )
}

fn state_capture_units(text: &str) -> Vec<String> {
    let mut units = Vec::new();
    for line in text.lines() {
        let line = trim_state_capture_unit(line);
        if line.is_empty() {
            continue;
        }
        if line.chars().count() <= STATE_CAPTURE_MAX_TEXT_CHARS {
            units.push(line);
            continue;
        }
        for sentence in line.split(['.', '!', '?']) {
            let sentence = trim_state_capture_unit(sentence);
            if !sentence.is_empty() {
                units.push(sentence);
            }
        }
    }
    if units.is_empty() {
        let unit = trim_state_capture_unit(text);
        if !unit.is_empty() {
            units.push(unit);
        }
    }
    units
}

fn trim_state_capture_unit(text: &str) -> String {
    text.trim()
        .trim_start_matches(['-', '*', '•'])
        .trim_start()
        .trim_end_matches(['.', '!', '?'])
        .trim()
        .to_string()
}

fn clean_state_capture_body(text: &str) -> Option<String> {
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned
        .trim()
        .trim_matches(['.', ',', ';', ':', '!', '?', '"'])
        .trim()
        .to_string();
    if cleaned.is_empty()
        || cleaned.chars().count() > STATE_CAPTURE_MAX_TEXT_CHARS
        || contains_secret_signal(&cleaned)
        || contains_sensitive_pii_signal(&cleaned)
        || looks_like_raw_log(&cleaned)
    {
        return None;
    }
    Some(cleaned)
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::MODE_ID;
    use tm_persona::{ModeId, PersonaConfig};

    #[test]
    fn mode_id_constant_resolves_in_bundled_catalog() {
        let assets = PersonaConfig::default().load_assets();
        assert!(
            assets.mode_profile(&ModeId::from(MODE_ID)).is_some(),
            "STATE_CAPTURE MODE_ID '{MODE_ID}' not found in bundled modes.json; \
             update the constant to match the catalog"
        );
    }
}

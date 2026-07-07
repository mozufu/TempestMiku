use std::collections::HashSet;

use crate::{ModeCatalog, ModeProfile, SkillActivation};

/// Compose the ordered, deduplicated list of skills active for one turn:
///
/// 1. always-on layered skills (`catalog.skills` entries with `activation: always`),
/// 2. the active mode's own declared skills (`profile.active_skills`, in order),
/// 3. triggered layered skills whose trigger words appear in `message`.
///
/// Modes are capability envelopes; this is the seam that lets conversational postures
/// (e.g. ambiguity-grill, negative-state-grounding) layer onto whichever mode is active
/// instead of requiring their own mode switch.
pub fn resolve_active_skills(
    catalog: &ModeCatalog,
    profile: &ModeProfile,
    message: &str,
) -> Vec<String> {
    let lower_message = message.to_lowercase();
    let mut candidates: Vec<&str> = Vec::new();

    for entry in &catalog.skills {
        if entry.activation == SkillActivation::Always {
            candidates.push(entry.name.as_str());
        }
    }
    for skill in &profile.active_skills {
        candidates.push(skill.as_str());
    }
    for entry in &catalog.skills {
        if entry.activation == SkillActivation::Triggered
            && entry
                .triggers
                .iter()
                .any(|trigger| lower_message.contains(&trigger.to_lowercase()))
        {
            candidates.push(entry.name.as_str());
        }
    }

    let mut seen: HashSet<&str> = HashSet::new();
    let mut resolved = Vec::with_capacity(candidates.len());
    for name in candidates {
        if seen.insert(name) {
            resolved.push(name.to_string());
        }
    }
    resolved
}

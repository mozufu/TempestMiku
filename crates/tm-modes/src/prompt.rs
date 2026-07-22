use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::ManagedSkillVersion;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedSkillPromptSnapshot {
    pub version: ManagedSkillVersion,
    pub body: String,
}

impl ManagedSkillPromptSnapshot {
    pub fn name(&self) -> &str {
        &self.version.name
    }

    pub fn content_digest(&self) -> &str {
        &self.version.content_digest
    }
}

use crate::assets::{
    BUNDLED_SOUL, BUNDLED_TM_LANG_FLUENCY_SKILL, MISSING_SKILL_PROMPT_FALLBACK,
    missing_layered_skill_reference_warning, missing_skill_reference_warning,
};
use crate::{AssetStatus, ModeId, ModeProfile, ModesConfig, resolve_active_skills};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedPrompt {
    pub system_prompt: String,
    pub profile: ModeProfile,
    pub status: AssetStatus,
    pub warnings: Vec<String>,
}

impl ModesConfig {
    pub fn build_system_prompt(
        &self,
        mode: &ModeId,
        base_system_prompt: &str,
        capability_notes: &str,
        message: &str,
        suppressed_skills: &BTreeSet<String>,
    ) -> ComposedPrompt {
        self.build_system_prompt_with_managed_snapshot(
            mode,
            base_system_prompt,
            capability_notes,
            message,
            suppressed_skills,
            &[],
        )
    }

    pub fn build_system_prompt_with_managed_snapshot(
        &self,
        mode: &ModeId,
        base_system_prompt: &str,
        capability_notes: &str,
        message: &str,
        suppressed_skills: &BTreeSet<String>,
        managed_skills: &[ManagedSkillPromptSnapshot],
    ) -> ComposedPrompt {
        let mut assets = self.load_assets();
        if let Ok(states) = self.managed_skills() {
            let selected = managed_skills
                .iter()
                .map(|skill| skill.version.name.as_str())
                .collect::<BTreeSet<_>>();
            for state in states {
                if selected.contains(state.active.name.as_str()) {
                    continue;
                }
                assets.skills.remove(&state.active.name);
                assets
                    .modes
                    .skills
                    .retain(|entry| entry.name != state.active.name);
            }
        }
        overlay_managed_prompt_snapshot(&mut assets, managed_skills);
        let mut warnings = assets.warnings.clone();
        let profile = match assets.mode_profile(mode) {
            Some(profile) => profile.clone(),
            None => {
                warnings.push(format!(
                    "mode profile {mode} unavailable; using unknown runtime fallback"
                ));
                ModeProfile::unknown(mode.clone())
            }
        };
        let mut prompt = String::new();

        push_section(&mut prompt, "Core runtime", base_system_prompt);
        push_section(&mut prompt, "Current mode", &current_mode_section(&profile));
        push_raw(
            &mut prompt,
            strip_frontmatter(BUNDLED_TM_LANG_FLUENCY_SKILL),
        );
        match &assets.soul {
            Some(soul) => push_section(&mut prompt, "SOUL.md", soul),
            None => push_section(&mut prompt, "SOUL.md", BUNDLED_SOUL),
        }

        if self.managed_persona_addenda_path().is_some() {
            match self.active_managed_persona_addendum("miku") {
                Ok(Some(addendum)) => {
                    let guidance = addendum
                        .changes
                        .iter()
                        .map(|change| format!("{}: {}", change.after.label, change.after.summary))
                        .collect::<Vec<_>>()
                        .join("\n");
                    push_section(&mut prompt, "Approved persona addendum", &guidance);
                }
                Ok(None) => {}
                Err(error) => warnings.push(format!(
                    "managed persona addendum miku is unavailable: {error}"
                )),
            }
        }

        for skill in resolve_active_skills(&assets.modes, &profile, message, suppressed_skills) {
            match assets.skills.get(skill.as_str()) {
                Some(contents) => push_raw(&mut prompt, strip_frontmatter(contents)),
                None => {
                    // Mode-declared skills and catalog-level layered skills get distinct
                    // wording so the warning correctly names where the reference lives.
                    let warning = if profile.active_skills.contains(&skill) {
                        missing_skill_reference_warning(&profile.mode, &skill)
                    } else {
                        missing_layered_skill_reference_warning(&skill)
                    };
                    if !warnings.iter().any(|existing| existing == &warning) {
                        warnings.push(warning);
                    }
                    push_raw(&mut prompt, MISSING_SKILL_PROMPT_FALLBACK);
                }
            }
        }

        if self.managed_mode_addenda_path().is_some() {
            match self.active_managed_mode_addendum(mode) {
                Ok(Some(addendum)) => {
                    let guidance = addendum
                        .changes
                        .iter()
                        .map(|change| format!("{}: {}", change.after.label, change.after.summary))
                        .collect::<Vec<_>>()
                        .join("\n");
                    push_section(&mut prompt, "Approved mode addendum", &guidance);
                }
                Ok(None) => {}
                Err(error) => warnings.push(format!(
                    "managed mode addendum {mode} is unavailable: {error}"
                )),
            }
        }

        if !capability_notes.trim().is_empty() {
            push_section(&mut prompt, "Runtime capabilities", capability_notes);
        }

        if !warnings.is_empty() {
            push_section(&mut prompt, "Mode asset warnings", &warnings.join("\n"));
        }

        ComposedPrompt {
            system_prompt: prompt,
            profile,
            status: assets.status,
            warnings,
        }
    }
}

fn overlay_managed_prompt_snapshot(
    assets: &mut crate::ModeAssets,
    managed_skills: &[ManagedSkillPromptSnapshot],
) {
    if managed_skills.is_empty() {
        return;
    }
    let selected = managed_skills
        .iter()
        .map(|skill| (skill.name(), skill))
        .collect::<BTreeMap<_, _>>();
    assets
        .modes
        .skills
        .retain(|entry| !selected.contains_key(entry.name.as_str()));
    for skill in managed_skills {
        assets
            .skills
            .insert(skill.version.name.clone(), skill.body.clone());
        assets.modes.skills.push(crate::SkillTrigger {
            name: skill.version.name.clone(),
            activation: crate::SkillActivation::Triggered,
            triggers: skill.version.triggers.clone(),
        });
    }
}

/// Names the active mode so the model always knows which capability envelope and posture it is
/// operating under. Modes otherwise only shape the prompt implicitly (which skills/addenda load);
/// this states it explicitly.
fn current_mode_section(profile: &ModeProfile) -> String {
    let mut section = format!(
        "You are operating in **{}** mode (`{}`, {} class).",
        profile.label, profile.mode, profile.capability_class
    );
    let description = profile.description.trim();
    if !description.is_empty() {
        section.push(' ');
        section.push_str(description);
    }
    section
}

fn push_section(target: &mut String, title: &str, content: &str) {
    if !target.is_empty() {
        target.push_str("\n\n");
    }
    target.push_str("## ");
    target.push_str(title);
    target.push('\n');
    target.push_str(content.trim());
}

fn push_raw(target: &mut String, content: &str) {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return;
    }
    if !target.is_empty() {
        target.push_str("\n\n");
    }
    target.push_str(trimmed);
}

/// Frontmatter (name/description/tags) is router bookkeeping, not something the model needs to read.
fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    let Some(rest) = trimmed.strip_prefix("---\n") else {
        return trimmed;
    };
    match rest.find("\n---") {
        Some(end) => rest[end + 4..].trim_start_matches('\n'),
        None => trimmed,
    }
}

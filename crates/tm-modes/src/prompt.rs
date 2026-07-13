use crate::assets::{
    BUNDLED_SOUL, MISSING_SKILL_PROMPT_FALLBACK, missing_layered_skill_reference_warning,
    missing_skill_reference_warning,
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
    ) -> ComposedPrompt {
        let assets = self.load_assets();
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
        match &assets.soul {
            Some(soul) => push_section(&mut prompt, "SOUL.md", soul),
            None => push_section(&mut prompt, "SOUL.md", BUNDLED_SOUL),
        }

        for skill in resolve_active_skills(&assets.modes, &profile, message) {
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

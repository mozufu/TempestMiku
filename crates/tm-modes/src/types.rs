use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModeId(String);

pub type Mode = ModeId;

impl ModeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for ModeId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for ModeId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeCatalog {
    pub default_mode: ModeId,
    pub modes: Vec<ModeProfile>,
    /// Layered skills that activate independent of the currently selected mode (always-on or
    /// keyword-triggered). `#[serde(default)]` so catalogs/fixtures written before this field
    /// existed still parse.
    #[serde(default)]
    pub skills: Vec<SkillTrigger>,
}

impl ModeCatalog {
    pub fn profile(&self, mode: &ModeId) -> Option<&ModeProfile> {
        self.modes.iter().find(|profile| &profile.mode == mode)
    }

    pub fn default_profile(&self) -> &ModeProfile {
        self.profile(&self.default_mode)
            .or_else(|| self.modes.first())
            .expect("mode catalog must contain at least one mode")
    }

    pub fn default_mode(&self) -> ModeId {
        self.default_profile().mode.clone()
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.modes.is_empty() {
            return Err("mode catalog must contain at least one mode".to_string());
        }
        if self.profile(&self.default_mode).is_none() {
            return Err(format!(
                "default mode {} is not present in mode catalog",
                self.default_mode
            ));
        }
        for profile in &self.modes {
            if profile.mode.as_str().trim().is_empty() {
                return Err("mode id must not be empty".to_string());
            }
            if profile.label.trim().is_empty() {
                return Err(format!("mode {} label must not be empty", profile.mode));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeRoute {
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub triggers: Vec<String>,
}

/// How a layered skill activates, independent of which mode is currently selected.
///
/// Modes are capability envelopes; skills are prompt payloads layered on top of whichever
/// mode is active. `Always` skills load on every turn; `Triggered` skills load only when the
/// current message matches one of their trigger words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillActivation {
    Always,
    Triggered,
}

/// A skill activation entry in the catalog's top-level `skills` array. Distinct from a mode's
/// `activeSkills`: these skills are not owned by any single mode and can layer onto any active
/// mode's prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillTrigger {
    pub name: String,
    pub activation: SkillActivation,
    #[serde(default)]
    pub triggers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeProfile {
    pub mode: ModeId,
    pub label: String,
    pub voice_cap: String,
    pub default_scope: String,
    #[serde(default)]
    pub active_skills: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub capability_class: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub route: ModeRoute,
}

impl ModeProfile {
    pub fn unknown(mode: ModeId) -> Self {
        Self {
            label: mode.as_str().to_string(),
            mode,
            voice_cap: "medium".to_string(),
            default_scope: "global".to_string(),
            active_skills: Vec::new(),
            capabilities: Vec::new(),
            capability_class: "conversation".to_string(),
            description: "Runtime mode profile unavailable.".to_string(),
            route: ModeRoute::default(),
        }
    }

    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities
            .iter()
            .any(|declared| capability_matches(declared, capability))
    }

    pub fn captures_personal_state(&self) -> bool {
        self.active_skills
            .iter()
            .any(|skill| skill == "personal-assistant-state-capture")
    }
}

fn capability_matches(declared: &str, capability: &str) -> bool {
    if declared == capability {
        return true;
    }
    let Some(prefix) = declared.strip_suffix(".*") else {
        return false;
    };
    capability
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.starts_with('.'))
}

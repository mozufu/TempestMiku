use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    PersonalAssistant,
    AmbiguityGrill,
    NegativeStateGrounding,
    SeriousEngineer,
    Handoff,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::PersonalAssistant => "Personal Assistant",
            Self::AmbiguityGrill => "Ambiguity Grill",
            Self::NegativeStateGrounding => "Negative-State Grounding",
            Self::SeriousEngineer => "Serious Engineer",
            Self::Handoff => "Handoff",
        }
    }

    pub fn voice_cap(self) -> &'static str {
        match self {
            Self::PersonalAssistant => "中",
            Self::AmbiguityGrill | Self::NegativeStateGrounding => "濃",
            Self::SeriousEngineer | Self::Handoff => "關",
        }
    }

    pub fn default_scope(self) -> &'static str {
        match self {
            Self::PersonalAssistant | Self::AmbiguityGrill | Self::NegativeStateGrounding => {
                "global"
            }
            Self::SeriousEngineer | Self::Handoff => "project:tempestmiku",
        }
    }

    pub fn system_addendum(self) -> &'static str {
        match self {
            Self::PersonalAssistant => {
                "Active mode: Personal Assistant. Use conversational planning and light memory; do not unlock engineering host capabilities."
            }
            Self::AmbiguityGrill => {
                "Active mode: Ambiguity Grill (mode 2). Ask 3-7 sharp clarifying questions before planning; keep capability scope conversational."
            }
            Self::NegativeStateGrounding => {
                "Active mode: Negative-State Grounding (mode 3). Stabilize first, keep the next action under ten minutes, and preserve the health-over-productivity rule."
            }
            Self::SeriousEngineer => {
                "Active mode: Serious Engineer (mode 4). Use fs.*, code.*, and proc.* through the SDK for linked-repo work. Voice cap: 關 — preserve Tempest Miku identity, but keep technical replies precise and avoid 喵 unless the context is explicitly light. Never use shell strings; use proc.run(cmd, args). Destructive, external, or out-of-grant actions require approval or fail closed."
            }
            Self::Handoff => {
                "Active mode: Handoff (mode 5). Delegate implementation-heavy coding work through the configured coding backend. Voice cap: 關 — preserve Tempest Miku identity, but keep the handoff precise and evidence-first."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PersonaStatus {
    Loaded { path: PathBuf },
    Degraded { warning: String },
}

#[derive(Debug, Clone, Default)]
pub struct PersonaConfig {
    pub asset_path: Option<PathBuf>,
}

impl PersonaConfig {
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self {
            asset_path: Some(path.into()),
        }
    }

    pub fn load_status(&self) -> PersonaStatus {
        match &self.asset_path {
            Some(path) if path.exists() => PersonaStatus::Loaded { path: path.clone() },
            Some(path) => PersonaStatus::Degraded {
                warning: format!("persona assets missing at {}", path.display()),
            },
            None => PersonaStatus::Degraded {
                warning: "persona asset path not configured".to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Mode;

    #[test]
    fn handoff_label_is_handoff() {
        assert_eq!(Mode::Handoff.label(), "Handoff");
    }

    #[test]
    fn router_modes_have_labels_and_scopes() {
        assert_eq!(Mode::AmbiguityGrill.label(), "Ambiguity Grill");
        assert_eq!(Mode::NegativeStateGrounding.default_scope(), "global");
        assert!(Mode::AmbiguityGrill.system_addendum().contains("mode 2"));
    }

    #[test]
    fn handoff_voice_cap_is_off() {
        assert_eq!(Mode::Handoff.voice_cap(), "關");
        assert_eq!(Mode::Handoff.default_scope(), "project:tempestmiku");
        assert!(Mode::Handoff.system_addendum().contains("mode 5"));
    }
}

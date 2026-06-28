use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    PersonalAssistant,
    SeriousEngineer,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::PersonalAssistant => "Personal Assistant",
            Self::SeriousEngineer => "Serious Engineer",
        }
    }

    pub fn voice_cap(self) -> &'static str {
        match self {
            Self::PersonalAssistant => "中",
            Self::SeriousEngineer => "關",
        }
    }

    pub fn default_scope(self) -> &'static str {
        match self {
            Self::PersonalAssistant => "global",
            Self::SeriousEngineer => "project:tempestmiku",
        }
    }

    pub fn system_addendum(self) -> &'static str {
        match self {
            Self::PersonalAssistant => {
                "Active mode: Personal Assistant. Use conversational planning and light memory; do not unlock engineering host capabilities."
            }
            Self::SeriousEngineer => {
                "Active mode: Serious Engineer (mode 4). Use fs.*, code.*, and proc.* through the SDK for linked-repo work. Voice cap: 關 — preserve Tempest Miku identity, but keep technical replies precise and avoid 喵 unless the context is explicitly light. Never use shell strings; use proc.run(cmd, args). Destructive, external, or out-of-grant actions require approval or fail closed."
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

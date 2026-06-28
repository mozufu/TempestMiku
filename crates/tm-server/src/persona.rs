use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    PersonalAssistant,
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

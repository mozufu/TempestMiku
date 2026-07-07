use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{ModeCatalog, ModeId, ModeProfile};

const BUNDLED_MODES_SOURCE: &str = "bundled:tm-modes/default-modes";
pub(crate) const BUNDLED_SOUL: &str = include_str!("../assets/SOUL.md");
const BUNDLED_MODES: &str = include_str!("../assets/modes.json");
pub(crate) const MISSING_SKILL_PROMPT_FALLBACK: &str = "Guidance for this situation is temporarily unavailable. Default to careful, capability-appropriate behavior and ask before uncertain or destructive actions.";
const BUNDLED_SKILLS: &[(&str, &str)] = &[
    (
        "miku-voice",
        include_str!("../assets/skills/miku-voice/SKILL.md"),
    ),
    (
        "ambiguity-grill",
        include_str!("../assets/skills/ambiguity-grill/SKILL.md"),
    ),
    (
        "negative-state-grounding",
        include_str!("../assets/skills/negative-state-grounding/SKILL.md"),
    ),
    (
        "oh-my-pi-handoff",
        include_str!("../assets/skills/oh-my-pi-handoff/SKILL.md"),
    ),
    (
        "personal-assistant-state-capture",
        include_str!("../assets/skills/personal-assistant-state-capture/SKILL.md"),
    ),
    (
        "scope-guard",
        include_str!("../assets/skills/scope-guard/SKILL.md"),
    ),
    (
        "weekly-ship-ledger",
        include_str!("../assets/skills/weekly-ship-ledger/SKILL.md"),
    ),
    (
        "serious-engineer-ops",
        include_str!("../assets/skills/serious-engineer-ops/SKILL.md"),
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AssetStatus {
    Loaded { path: PathBuf },
    Degraded { warning: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeAssets {
    pub status: AssetStatus,
    pub soul: Option<String>,
    pub skills: BTreeMap<String, String>,
    pub modes: ModeCatalog,
    pub warnings: Vec<String>,
}

impl ModeAssets {
    pub fn mode_profile(&self, mode: &ModeId) -> Option<&ModeProfile> {
        self.modes.profile(mode)
    }

    pub fn profile_or_unknown(&self, mode: &ModeId) -> ModeProfile {
        self.mode_profile(mode)
            .cloned()
            .unwrap_or_else(|| ModeProfile::unknown(mode.clone()))
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModesConfig {
    pub asset_path: Option<PathBuf>,
}

impl ModesConfig {
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self {
            asset_path: Some(path.into()),
        }
    }

    pub fn load_status(&self) -> AssetStatus {
        self.load_assets().status
    }

    pub fn default_mode(&self) -> ModeId {
        self.load_assets().modes.default_mode()
    }

    pub fn load_assets(&self) -> ModeAssets {
        let Some(root) = &self.asset_path else {
            return bundled_assets();
        };

        if !root.exists() {
            let warning = format!(
                "mode assets missing at {}; using bundled defaults",
                root.display()
            );
            let (skills, modes) = bundled_catalog_assets();
            return ModeAssets {
                status: AssetStatus::Degraded {
                    warning: warning.clone(),
                },
                soul: Some(BUNDLED_SOUL.to_string()),
                skills,
                modes,
                warnings: vec![warning],
            };
        }

        let mut warnings = Vec::new();
        let soul_path = root.join("SOUL.md");
        let soul = match fs::read_to_string(&soul_path) {
            Ok(contents) => Some(contents),
            Err(err) => {
                warnings.push(format!(
                    "missing or unreadable SOUL.md: {err}; using bundled default"
                ));
                Some(BUNDLED_SOUL.to_string())
            }
        };

        let modes = load_configured_modes(root, &mut warnings);
        let skills = load_configured_skills(root, &mut warnings);
        warn_missing_skill_references(&modes, &skills, &mut warnings);

        let status = if warnings.is_empty() {
            AssetStatus::Loaded { path: root.clone() }
        } else {
            AssetStatus::Degraded {
                warning: warnings.join("; "),
            }
        };

        ModeAssets {
            status,
            soul,
            skills,
            modes,
            warnings,
        }
    }
}

fn load_configured_modes(root: &Path, warnings: &mut Vec<String>) -> ModeCatalog {
    let mode_path = root.join("modes.json");
    match fs::read_to_string(&mode_path) {
        Ok(contents) => match parse_mode_catalog(&contents) {
            Ok(catalog) => catalog,
            Err(err) => {
                warnings.push(format!(
                    "unreadable mode catalog {}: {err}; using bundled defaults",
                    mode_path.display()
                ));
                bundled_mode_catalog()
            }
        },
        Err(err) => {
            warnings.push(format!(
                "missing or unreadable modes.json: {err}; using bundled defaults"
            ));
            bundled_mode_catalog()
        }
    }
}

fn load_configured_skills(root: &Path, warnings: &mut Vec<String>) -> BTreeMap<String, String> {
    let mut skills = bundled_skill_map();
    let skills_path = root.join("skills");
    let entries = match fs::read_dir(&skills_path) {
        Ok(entries) => entries,
        Err(err) => {
            warnings.push(format!(
                "missing or unreadable skills directory: {err}; using bundled defaults"
            ));
            return skills;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                warnings.push(format!("unreadable skill directory entry: {err}"));
                continue;
            }
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(skill_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let skill_path = path.join("SKILL.md");
        match fs::read_to_string(&skill_path) {
            Ok(contents) => {
                skills.insert(skill_name.to_string(), contents);
            }
            Err(_) => warnings.push(format!(
                "missing or unreadable skills/{skill_name}/SKILL.md; using bundled default if available"
            )),
        }
    }

    skills
}

fn bundled_assets() -> ModeAssets {
    let (skills, modes) = bundled_catalog_assets();
    ModeAssets {
        status: AssetStatus::Loaded {
            path: PathBuf::from(BUNDLED_MODES_SOURCE),
        },
        soul: Some(BUNDLED_SOUL.to_string()),
        skills,
        modes,
        warnings: Vec::new(),
    }
}

fn bundled_catalog_assets() -> (BTreeMap<String, String>, ModeCatalog) {
    let skills = bundled_skill_map();
    let modes = bundled_mode_catalog();
    let missing = missing_skill_references(&modes, &skills);
    assert!(
        missing.is_empty(),
        "bundled modes.json references missing skills: {}",
        missing
            .iter()
            .map(|(mode, skill)| format!("{mode}:{skill}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let missing_layered = missing_layered_skill_references(&modes, &skills);
    assert!(
        missing_layered.is_empty(),
        "bundled modes.json top-level skills array references missing skills: {}",
        missing_layered.join(", ")
    );
    (skills, modes)
}

fn bundled_skill_map() -> BTreeMap<String, String> {
    BUNDLED_SKILLS
        .iter()
        .map(|(name, contents)| ((*name).to_string(), (*contents).to_string()))
        .collect()
}

fn bundled_mode_catalog() -> ModeCatalog {
    parse_mode_catalog(BUNDLED_MODES).expect("bundled modes.json is valid")
}

fn parse_mode_catalog(contents: &str) -> std::result::Result<ModeCatalog, String> {
    let catalog: ModeCatalog = serde_json::from_str(contents).map_err(|err| err.to_string())?;
    catalog.validate()?;
    Ok(catalog)
}

fn warn_missing_skill_references(
    catalog: &ModeCatalog,
    skills: &BTreeMap<String, String>,
    warnings: &mut Vec<String>,
) {
    for (mode, skill) in missing_skill_references(catalog, skills) {
        warnings.push(missing_skill_reference_warning(&mode, &skill));
    }
    for skill in missing_layered_skill_references(catalog, skills) {
        warnings.push(missing_layered_skill_reference_warning(&skill));
    }
}

fn missing_skill_references(
    catalog: &ModeCatalog,
    skills: &BTreeMap<String, String>,
) -> Vec<(ModeId, String)> {
    let mut missing = Vec::new();
    for profile in &catalog.modes {
        for skill in &profile.active_skills {
            if !skills.contains_key(skill) {
                missing.push((profile.mode.clone(), skill.clone()));
            }
        }
    }
    missing
}

fn missing_layered_skill_references(
    catalog: &ModeCatalog,
    skills: &BTreeMap<String, String>,
) -> Vec<String> {
    catalog
        .skills
        .iter()
        .map(|entry| entry.name.clone())
        .filter(|name| !skills.contains_key(name.as_str()))
        .collect()
}

pub(crate) fn missing_skill_reference_warning(mode: &ModeId, skill: &str) -> String {
    format!(
        "active skill {skill} referenced by mode {mode} is missing at skills/{skill}/SKILL.md; prompt will use the missing-skill fallback"
    )
}

pub(crate) fn missing_layered_skill_reference_warning(skill: &str) -> String {
    format!(
        "layered skill {skill} referenced by the catalog's top-level skills array is missing at skills/{skill}/SKILL.md; it will be skipped during composition"
    )
}

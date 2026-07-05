use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const KNOWN_SKILLS: &[&str] = &[
    "miku-voice",
    "ambiguity-grill",
    "negative-state-grounding",
    "oh-my-pi-handoff",
    "personal-assistant-state-capture",
    "scope-guard",
    "weekly-ship-ledger",
    "serious-engineer-ops",
];

const BUNDLED_MODES_SOURCE: &str = "bundled:tm-modes/default-modes";
const BUNDLED_SOUL: &str = include_str!("../assets/SOUL.md");
const BUNDLED_MODES: &str = include_str!("../assets/modes.json");
const MISSING_SKILL_PROMPT_FALLBACK: &str = "Guidance for this situation is temporarily unavailable. Default to careful, capability-appropriate behavior and ask before uncertain or destructive actions.";
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedPrompt {
    pub system_prompt: String,
    pub profile: ModeProfile,
    pub status: AssetStatus,
    pub warnings: Vec<String>,
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

    pub fn build_system_prompt(
        &self,
        mode: &ModeId,
        base_system_prompt: &str,
        capability_notes: &str,
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

        for skill in &profile.active_skills {
            match assets.skills.get(skill.as_str()) {
                Some(contents) => push_raw(&mut prompt, strip_frontmatter(contents)),
                None => {
                    let warning = missing_skill_reference_warning(&profile.mode, skill);
                    if !warnings.iter().any(|existing| existing == &warning) {
                        warnings.push(warning);
                    }
                    push_raw(&mut prompt, MISSING_SKILL_PROMPT_FALLBACK);
                }
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

fn missing_skill_reference_warning(mode: &ModeId, skill: &str) -> String {
    format!(
        "active skill {skill} referenced by mode {mode} is missing at skills/{skill}/SKILL.md; prompt will use the missing-skill fallback"
    )
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        KNOWN_SKILLS, MISSING_SKILL_PROMPT_FALLBACK, ModeId, ModesConfig, AssetStatus,
    };

    static TEMP_ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn bundled_mode_catalog_has_default_and_handoff_profile() {
        let assets = ModesConfig::default().load_assets();
        assert_eq!(assets.modes.default_mode().as_str(), "personal_assistant");
        let handoff = assets
            .modes
            .profile(&ModeId::from("handoff"))
            .expect("handoff profile");
        assert_eq!(handoff.label, "Handoff");
        assert_eq!(handoff.voice_cap, "off");
        assert_eq!(handoff.default_scope, "project:tempestmiku");
        assert_eq!(handoff.active_skills, ["oh-my-pi-handoff"]);
        assert!(handoff.has_capability("backend.coding"));
        assert!(handoff.has_capability("agents.run"), "agents.* glob must match agents.run");
        assert!(handoff.has_capability("agents.spawn"), "agents.* glob must match agents.spawn");
    }

    #[test]
    fn bundled_router_modes_have_labels_and_scopes() {
        let assets = ModesConfig::default().load_assets();
        let grill = assets
            .modes
            .profile(&ModeId::from("ambiguity_grill"))
            .expect("ambiguity grill profile");
        assert_eq!(grill.label, "Ambiguity Grill");
        assert_eq!(grill.active_skills, ["miku-voice", "ambiguity-grill"]);

        let grounding = assets
            .modes
            .profile(&ModeId::from("negative_state_grounding"))
            .expect("negative-state profile");
        assert_eq!(grounding.default_scope, "global");
        assert_eq!(grounding.capability_class, "conversation");
    }

    #[test]
    fn composed_prompt_never_leaks_skill_frontmatter_or_mode_metadata() {
        let catalog = ModesConfig::default().load_assets().modes;
        for profile in &catalog.modes {
            let prompt = ModesConfig::default().build_system_prompt(&profile.mode, "base", "");
            let leaks = ["description:", "tags:", "hermes:", "category:", "skill://", "Mode id:", "Voice cap:", "Capability class:", "Declared capabilities:"];
            for leak in leaks {
                assert!(
                    !prompt.system_prompt.contains(leak),
                    "mode {} prompt leaked runtime bookkeeping {leak:?}:\n{}",
                    profile.mode,
                    prompt.system_prompt
                );
            }
        }
    }

    #[test]
    fn default_config_loads_bundled_mode_assets() {
        let assets = ModesConfig::default().load_assets();
        assert_eq!(
            assets.status,
            AssetStatus::Loaded {
                path: PathBuf::from("bundled:tm-modes/default-modes")
            }
        );
        assert!(assets.warnings.is_empty());
        assert!(assets.soul.unwrap().contains("Tempest Miku"));
        for skill in KNOWN_SKILLS {
            assert!(
                assets.skills.contains_key(*skill),
                "bundled mode assets are missing {skill}"
            );
        }
        assert!(
            assets
                .modes
                .profile(&ModeId::from("serious_engineer"))
                .is_some()
        );
    }

    #[test]
    fn bundled_active_skill_references_resolve_to_skill_assets() {
        let assets = ModesConfig::default().load_assets();

        for profile in &assets.modes.modes {
            for skill in &profile.active_skills {
                assert!(
                    assets.skills.contains_key(skill),
                    "bundled mode {} references missing skills/{skill}/SKILL.md",
                    profile.mode
                );
            }
        }
    }

    #[test]
    fn loads_fixture_soul_modes_and_skills() {
        let root = temp_modes_root();
        write_fixture(&root, true, &["custom-skill"], Some(custom_modes_json()));

        let assets = ModesConfig::from_path(&root).load_assets();
        assert_eq!(assets.status, AssetStatus::Loaded { path: root.clone() });
        assert!(assets.soul.unwrap().contains("Fixture SOUL"));
        assert!(
            assets
                .skills
                .get("custom-skill")
                .unwrap()
                .contains("custom-skill fixture")
        );
        let custom = assets
            .modes
            .profile(&ModeId::from("custom_runtime_mode"))
            .expect("custom runtime mode");
        assert_eq!(custom.label, "Custom Runtime Mode");
        assert_eq!(custom.active_skills, ["custom-skill"]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn degrades_when_soul_modes_or_skills_are_missing() {
        let root = temp_modes_root();
        write_fixture(&root, false, &["ambiguity-grill"], None);

        let assets = ModesConfig::from_path(&root).load_assets();
        let AssetStatus::Degraded { warning } = assets.status else {
            panic!("missing SOUL.md and modes should degrade");
        };
        assert!(warning.contains("SOUL.md"));
        assert!(warning.contains("modes.json"));
        assert!(assets.soul.unwrap().contains("Tempest Miku"));
        assert!(
            assets
                .skills
                .get("miku-voice")
                .unwrap()
                .contains("miku-voice")
        );

        let prompt = ModesConfig::from_path(&root).build_system_prompt(
            &ModeId::from("ambiguity_grill"),
            "base prompt",
            "capability notes",
        );
        assert!(prompt.system_prompt.contains("SOUL.md"));
        assert!(prompt.system_prompt.contains("語氣層"));
        assert!(prompt.system_prompt.contains("ambiguity-grill fixture skill body"));
        assert!(!prompt.system_prompt.contains("temporarily unavailable"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configured_missing_active_skill_warns_and_uses_prompt_fallback() {
        let root = temp_modes_root();
        write_fixture(&root, true, &[], Some(missing_skill_modes_json()));
        let expected_warning = "active skill missing-skill referenced by mode custom_runtime_mode is missing at skills/missing-skill/SKILL.md; prompt will use the missing-skill fallback";

        let assets = ModesConfig::from_path(&root).load_assets();
        assert_eq!(assets.warnings, [expected_warning]);
        assert_eq!(
            assets.status,
            AssetStatus::Degraded {
                warning: expected_warning.to_string()
            }
        );

        let prompt = ModesConfig::from_path(&root).build_system_prompt(
            &ModeId::from("custom_runtime_mode"),
            "base prompt",
            "",
        );
        assert_eq!(prompt.warnings, [expected_warning]);
        assert!(prompt.system_prompt.contains(MISSING_SKILL_PROMPT_FALLBACK));
        assert!(prompt.system_prompt.contains("## Mode asset warnings"));
        assert!(prompt.system_prompt.contains(expected_warning));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mode_profiles_map_expected_skills_voice_and_scope() {
        let assets = ModesConfig::default().load_assets();
        let assistant = assets
            .modes
            .profile(&ModeId::from("personal_assistant"))
            .expect("assistant profile");
        assert_eq!(
            assistant.active_skills,
            vec!["miku-voice", "personal-assistant-state-capture"]
        );
        assert_eq!(assistant.voice_cap, "medium");
        assert_eq!(assistant.default_scope, "global");
        assert!(assistant.captures_personal_state());

        let serious = assets
            .modes
            .profile(&ModeId::from("serious_engineer"))
            .expect("serious profile");
        assert_eq!(serious.active_skills, vec!["serious-engineer-ops"]);
        assert!(serious.has_capability("fs.read"));
        assert!(serious.has_capability("code.edit"));
        assert!(serious.has_capability("proc.run"));
        assert!(serious.has_capability("backend.coding"));
        assert_eq!(serious.capability_class, "engineering");
        assert_eq!(serious.voice_cap, "off");
        assert_eq!(serious.default_scope, "project:tempestmiku");
    }

    #[test]
    fn negative_state_grounding_prompt_is_health_first_conversational_posture() {
        let prompt = ModesConfig::default().build_system_prompt(
            &ModeId::from("negative_state_grounding"),
            "base prompt",
            "",
        );

        assert_eq!(prompt.profile.capability_class, "conversation");
        assert_eq!(
            prompt.profile.active_skills,
            vec!["miku-voice", "negative-state-grounding"]
        );
        assert!(prompt.system_prompt.contains("conversational posture"));
        assert!(prompt.system_prompt.contains("Health-over-productivity"));
        assert!(prompt.system_prompt.contains("one next action"));
        assert!(prompt.system_prompt.contains("10 minutes or less"));
        assert!(
            prompt
                .system_prompt
                .contains("Do not propose or request memory writes")
        );
        assert!(
            !prompt
                .system_prompt
                .contains("Personal Assistant State Capture")
        );
    }

    fn temp_modes_root() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let counter = TEMP_ROOT_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "tm-modes-test-{}-{nanos}-{counter}",
            std::process::id()
        ))
    }

    fn write_fixture(root: &Path, include_soul: bool, skills: &[&str], modes_json: Option<String>) {
        fs::create_dir_all(root.join("skills")).unwrap();
        if include_soul {
            fs::write(root.join("SOUL.md"), "# Fixture SOUL\nidentity constant").unwrap();
        }
        if let Some(modes_json) = modes_json {
            fs::write(root.join("modes.json"), modes_json).unwrap();
        }
        for skill in skills {
            let dir = root.join("skills").join(skill);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("SKILL.md"),
                format!("# {skill}\n{skill} fixture skill body"),
            )
            .unwrap();
        }
    }

    fn custom_modes_json() -> String {
        serde_json::json!({
            "defaultMode": "custom_runtime_mode",
            "modes": [
                {
                    "mode": "custom_runtime_mode",
                    "label": "Custom Runtime Mode",
                    "description": "Loaded only from runtime mode assets.",
                    "voiceCap": "medium",
                    "defaultScope": "global",
                    "activeSkills": ["custom-skill"],
                    "capabilities": ["memory.recall"],
                    "capabilityClass": "conversation",
                    "route": {
                        "isDefault": true,
                        "priority": 0,
                        "triggers": []
                    }
                }
            ]
        })
        .to_string()
    }

    fn missing_skill_modes_json() -> String {
        serde_json::json!({
            "defaultMode": "custom_runtime_mode",
            "modes": [
                {
                    "mode": "custom_runtime_mode",
                    "label": "Custom Runtime Mode",
                    "description": "Loaded only from runtime mode assets.",
                    "voiceCap": "medium",
                    "defaultScope": "global",
                    "activeSkills": ["missing-skill"],
                    "capabilities": [],
                    "capabilityClass": "conversation",
                    "route": {
                        "isDefault": true,
                        "priority": 0,
                        "triggers": []
                    }
                }
            ]
        })
        .to_string()
    }
}

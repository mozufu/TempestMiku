use std::{collections::BTreeMap, fs, path::PathBuf};

use serde::{Deserialize, Serialize};

pub const KNOWN_SKILLS: &[&str] = &[
    "miku-voice",
    "ambiguity-grill",
    "negative-state-grounding",
    "oh-my-pi-handoff",
    "personal-assistant-state-capture",
    "scope-guard",
    "weekly-ship-ledger",
];

const BUNDLED_PERSONA_SOURCE: &str = "bundled:tm-persona/default-persona";
const BUNDLED_SOUL: &str = include_str!("../assets/SOUL.md");
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
];

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
            Self::PersonalAssistant => "medium",
            Self::AmbiguityGrill | Self::NegativeStateGrounding => "high",
            Self::SeriousEngineer | Self::Handoff => "off",
        }
    }

    pub fn voice_cap_guidance(self) -> &'static str {
        match self {
            Self::PersonalAssistant => {
                "medium: keep Miku warm and present, with only occasional voice flourishes."
            }
            Self::AmbiguityGrill => "high: teasing and sharp is allowed; roast the fog, not Brian.",
            Self::NegativeStateGrounding => {
                "high: warmer and softer is allowed; grounding beats productivity."
            }
            Self::SeriousEngineer => {
                "off: keep identity but remove cute flourishes; precision, tests, approvals, and rollback matter most."
            }
            Self::Handoff => "off: keep the brief precise, self-contained, and evidence-first.",
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
                "Active mode: Serious Engineer (mode 4). Use fs.*, code.*, and proc.* through the SDK for linked-repo work. Voice cap: off — preserve Tempest Miku identity, but keep technical replies precise and avoid voice flourishes unless the context is explicitly light. Never use shell strings; use proc.run(cmd, args). Destructive, external, or out-of-grant actions require approval or fail closed."
            }
            Self::Handoff => {
                "Active mode: Handoff (mode 5). Delegate implementation-heavy coding work through the configured coding backend. Voice cap: off — preserve Tempest Miku identity, but keep the handoff precise and evidence-first."
            }
        }
    }

    pub fn active_skill_names(self) -> &'static [&'static str] {
        match self {
            Self::PersonalAssistant => &["miku-voice", "personal-assistant-state-capture"],
            Self::AmbiguityGrill => &["miku-voice", "ambiguity-grill"],
            Self::NegativeStateGrounding => &["miku-voice", "negative-state-grounding"],
            Self::SeriousEngineer => &[],
            Self::Handoff => &["oh-my-pi-handoff"],
        }
    }

    pub fn capability_class(self) -> &'static str {
        match self {
            Self::PersonalAssistant | Self::AmbiguityGrill | Self::NegativeStateGrounding => {
                "conversation"
            }
            Self::SeriousEngineer => "engineering",
            Self::Handoff => "handoff",
        }
    }

    pub fn profile(self) -> ModeProfile {
        ModeProfile {
            mode: self,
            label: self.label().to_string(),
            voice_cap: self.voice_cap().to_string(),
            default_scope: self.default_scope().to_string(),
            active_skills: self
                .active_skill_names()
                .iter()
                .map(|skill| (*skill).to_string())
                .collect(),
            capability_class: self.capability_class().to_string(),
            addendum: self.system_addendum().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeProfile {
    pub mode: Mode,
    pub label: String,
    pub voice_cap: String,
    pub default_scope: String,
    pub active_skills: Vec<String>,
    pub capability_class: String,
    pub addendum: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PersonaStatus {
    Loaded { path: PathBuf },
    Degraded { warning: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaAssets {
    pub status: PersonaStatus,
    pub soul: Option<String>,
    pub skills: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaPrompt {
    pub system_prompt: String,
    pub profile: ModeProfile,
    pub status: PersonaStatus,
    pub warnings: Vec<String>,
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
        self.load_assets().status
    }

    pub fn load_assets(&self) -> PersonaAssets {
        let Some(root) = &self.asset_path else {
            return bundled_assets();
        };

        if !root.exists() {
            let warning = format!(
                "persona assets missing at {}; using bundled defaults",
                root.display()
            );
            return PersonaAssets {
                status: PersonaStatus::Degraded {
                    warning: warning.clone(),
                },
                soul: Some(BUNDLED_SOUL.to_string()),
                skills: bundled_skill_map(),
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

        let mut skills = bundled_skill_map();
        for skill in KNOWN_SKILLS {
            let path = root.join("skills").join(skill).join("SKILL.md");
            match fs::read_to_string(&path) {
                Ok(contents) => {
                    skills.insert((*skill).to_string(), contents);
                }
                Err(err) => warnings.push(format!(
                    "missing or unreadable skill {skill}: {err}; using bundled default"
                )),
            }
        }

        let status = if warnings.is_empty() {
            PersonaStatus::Loaded { path: root.clone() }
        } else {
            PersonaStatus::Degraded {
                warning: warnings.join("; "),
            }
        };

        PersonaAssets {
            status,
            soul,
            skills,
            warnings,
        }
    }

    pub fn build_system_prompt(
        &self,
        mode: Mode,
        base_system_prompt: &str,
        capability_notes: &str,
    ) -> PersonaPrompt {
        let assets = self.load_assets();
        let profile = mode.profile();
        let mut prompt = String::new();

        push_section(&mut prompt, "Core runtime", base_system_prompt);
        match &assets.soul {
            Some(soul) => push_section(&mut prompt, "SOUL.md", soul),
            None => push_section(&mut prompt, "SOUL.md", BUNDLED_SOUL),
        }

        let active_skills = if profile.active_skills.is_empty() {
            "none".to_string()
        } else {
            profile.active_skills.join(", ")
        };
        let mode_profile = format!(
            "{}\n\nLabel: {}\nVoice cap: {}\nVoice guidance: {}\nDefault scope: {}\nCapability class: {}\nActive skills: {}",
            profile.addendum,
            profile.label,
            profile.voice_cap,
            mode.voice_cap_guidance(),
            profile.default_scope,
            profile.capability_class,
            active_skills
        );
        push_section(&mut prompt, "Active mode profile", &mode_profile);

        for skill in &profile.active_skills {
            match assets.skills.get(skill.as_str()) {
                Some(contents) => push_section(&mut prompt, &format!("skill://{skill}"), contents),
                None => push_section(
                    &mut prompt,
                    &format!("missing skill://{skill}"),
                    "This active skill is unavailable from persona assets. Use the built-in mode profile as the fallback.",
                ),
            }
        }

        if !capability_notes.trim().is_empty() {
            push_section(&mut prompt, "Runtime capabilities", capability_notes);
        }

        if !assets.warnings.is_empty() {
            push_section(
                &mut prompt,
                "Persona asset warnings",
                &assets.warnings.join("\n"),
            );
        }

        PersonaPrompt {
            system_prompt: prompt,
            profile,
            status: assets.status,
            warnings: assets.warnings,
        }
    }
}

fn bundled_assets() -> PersonaAssets {
    PersonaAssets {
        status: PersonaStatus::Loaded {
            path: PathBuf::from(BUNDLED_PERSONA_SOURCE),
        },
        soul: Some(BUNDLED_SOUL.to_string()),
        skills: bundled_skill_map(),
        warnings: Vec::new(),
    }
}

fn bundled_skill_map() -> BTreeMap<String, String> {
    BUNDLED_SKILLS
        .iter()
        .map(|(name, contents)| ((*name).to_string(), (*contents).to_string()))
        .collect()
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{KNOWN_SKILLS, Mode, PersonaConfig, PersonaStatus};

    #[test]
    fn handoff_label_is_handoff() {
        assert_eq!(Mode::Handoff.label(), "Handoff");
    }

    #[test]
    fn router_modes_have_labels_and_scopes() {
        assert_eq!(Mode::AmbiguityGrill.label(), "Ambiguity Grill");
        assert_eq!(Mode::NegativeStateGrounding.default_scope(), "global");
        assert!(Mode::AmbiguityGrill.system_addendum().contains("mode 2"));
        assert_eq!(
            Mode::AmbiguityGrill.active_skill_names(),
            ["miku-voice", "ambiguity-grill"]
        );
    }

    #[test]
    fn handoff_voice_cap_is_off() {
        assert_eq!(Mode::Handoff.voice_cap(), "off");
        assert_eq!(Mode::Handoff.default_scope(), "project:tempestmiku");
        assert!(Mode::Handoff.system_addendum().contains("mode 5"));
        assert_eq!(Mode::Handoff.active_skill_names(), ["oh-my-pi-handoff"]);
    }

    #[test]
    fn default_config_loads_bundled_persona_assets() {
        let assets = PersonaConfig::default().load_assets();
        assert_eq!(
            assets.status,
            PersonaStatus::Loaded {
                path: PathBuf::from("bundled:tm-persona/default-persona")
            }
        );
        assert!(assets.warnings.is_empty());
        assert!(assets.soul.unwrap().contains("Tempest Miku"));
        for skill in KNOWN_SKILLS {
            assert!(
                assets.skills.contains_key(*skill),
                "bundled persona assets are missing {skill}"
            );
        }
    }

    #[test]
    fn loads_fixture_soul_and_known_skills() {
        let root = temp_persona_root();
        write_fixture(&root, true, KNOWN_SKILLS);

        let assets = PersonaConfig::from_path(&root).load_assets();
        assert_eq!(assets.status, PersonaStatus::Loaded { path: root.clone() });
        assert!(assets.soul.unwrap().contains("Fixture SOUL"));
        assert!(
            assets
                .skills
                .get("miku-voice")
                .unwrap()
                .contains("miku-voice fixture")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn degrades_when_soul_or_active_skills_are_missing() {
        let root = temp_persona_root();
        write_fixture(&root, false, &["ambiguity-grill"]);

        let assets = PersonaConfig::from_path(&root).load_assets();
        let PersonaStatus::Degraded { warning } = assets.status else {
            panic!("missing SOUL.md and skills should degrade");
        };
        assert!(warning.contains("SOUL.md"));
        assert!(warning.contains("miku-voice"));
        assert!(assets.soul.unwrap().contains("Tempest Miku"));
        assert!(
            assets
                .skills
                .get("miku-voice")
                .unwrap()
                .contains("miku-voice")
        );

        let prompt = PersonaConfig::from_path(&root).build_system_prompt(
            Mode::AmbiguityGrill,
            "base prompt",
            "capability notes",
        );
        assert!(prompt.system_prompt.contains("SOUL.md"));
        assert!(prompt.system_prompt.contains("skill://miku-voice"));
        assert!(!prompt.system_prompt.contains("missing skill://miku-voice"));
        assert!(prompt.system_prompt.contains("skill://ambiguity-grill"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mode_profiles_map_expected_skills_voice_and_scope() {
        let assistant = Mode::PersonalAssistant.profile();
        assert_eq!(
            assistant.active_skills,
            vec!["miku-voice", "personal-assistant-state-capture"]
        );
        assert_eq!(assistant.voice_cap, "medium");
        assert_eq!(assistant.default_scope, "global");

        let serious = Mode::SeriousEngineer.profile();
        assert!(serious.active_skills.is_empty());
        assert_eq!(serious.capability_class, "engineering");
        assert_eq!(serious.voice_cap, "off");
        assert_eq!(serious.default_scope, "project:tempestmiku");
    }

    fn temp_persona_root() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("tm-persona-test-{}-{nanos}", std::process::id()))
    }

    fn write_fixture(root: &Path, include_soul: bool, skills: &[&str]) {
        fs::create_dir_all(root.join("skills")).unwrap();
        if include_soul {
            fs::write(root.join("SOUL.md"), "# Fixture SOUL\nidentity constant").unwrap();
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
}

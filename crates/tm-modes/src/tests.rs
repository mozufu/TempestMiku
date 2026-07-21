use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    AssetStatus, KNOWN_SKILLS, MISSING_SKILL_PROMPT_FALLBACK, ModeId, ModesConfig, SkillActivation,
    resolve_active_skills,
};

static TEMP_ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn bundled_mode_catalog_has_default_and_serious_engineer_profile() {
    let assets = ModesConfig::default().load_assets();
    assert_eq!(assets.modes.default_mode().as_str(), "general");
    let serious = assets
        .modes
        .profile(&ModeId::from("serious_engineer"))
        .expect("serious engineer profile");
    assert_eq!(serious.label, "Serious Engineer");
    assert_eq!(serious.default_scope, "project:tempestmiku");
    assert_eq!(serious.active_skills, ["serious-engineer-ops"]);
    assert!(serious.has_capability("backend.coding"));
    assert!(serious.has_capability("agents.run"));
    assert!(serious.has_capability("agents.spawn"));
    for capability in [
        "git.clone",
        "git.init",
        "git.add",
        "git.mv",
        "git.restore",
        "git.rm",
        "git.bisect",
        "git.grep",
        "git.show",
        "git.status",
        "git.diff",
        "git.log",
        "git.commit",
        "git.push",
        "git.pull",
    ] {
        assert!(serious.has_capability(capability));
    }
    assert!(!serious.has_capability("git.run"));
    let granted_git: Vec<&str> = serious
        .capabilities
        .iter()
        .map(String::as_str)
        .filter(|capability| capability.starts_with("git."))
        .collect();
    assert_eq!(
        granted_git,
        [
            "git.clone",
            "git.init",
            "git.add",
            "git.mv",
            "git.restore",
            "git.rm",
            "git.bisect",
            "git.grep",
            "git.show",
            "git.status",
            "git.diff",
            "git.log",
            "git.commit",
            "git.push",
            "git.pull",
        ]
    );
}

#[test]
fn bundled_catalog_has_exactly_two_capability_modes() {
    let assets = ModesConfig::default().load_assets();
    let mut ids: Vec<&str> = assets
        .modes
        .modes
        .iter()
        .map(|profile| profile.mode.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, ["general", "serious_engineer"]);
    assert!(assets.modes.profile(&ModeId::from("handoff")).is_none());
}

#[test]
fn bundled_layered_skills_have_expected_activation_and_triggers() {
    let assets = ModesConfig::default().load_assets();
    let find = |name: &str| {
        assets
            .modes
            .skills
            .iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| panic!("layered skill {name} missing from catalog"))
    };

    let scope_guard = find("scope-guard");
    assert_eq!(scope_guard.activation, SkillActivation::Always);

    let grill = find("ambiguity-grill");
    assert_eq!(grill.activation, SkillActivation::Triggered);
    assert!(grill.triggers.iter().any(|t| t == "grill me"));

    let grounding = find("negative-state-grounding");
    assert_eq!(grounding.activation, SkillActivation::Triggered);
    assert!(grounding.triggers.iter().any(|t| t == "overwhelmed"));

    let ledger = find("weekly-ship-ledger");
    assert_eq!(ledger.activation, SkillActivation::Triggered);
}

#[test]
fn runtime_fluency_skill_is_pinned_before_identity_for_every_mode() {
    let config = ModesConfig::default();
    let assets = config.load_assets();
    let skill = assets
        .skills
        .get("tm-lang-fluency")
        .expect("runtime fluency skill");
    assert!(skill.contains("fun value -> expr"));
    assert!(!skill.contains("`remove` deletes"));
    assert!(!skill.contains("stale tag"));

    for profile in &assets.modes.modes {
        let prompt = config.build_system_prompt(&profile.mode, "base", "", "");
        let fluency = prompt
            .system_prompt
            .find("# tm Language Fluency")
            .expect("runtime fluency guidance");
        let soul = prompt
            .system_prompt
            .find("## SOUL.md")
            .expect("SOUL section");
        assert!(
            fluency < soul,
            "runtime fluency must precede identity skills"
        );
    }
}

#[test]
fn configured_assets_cannot_replace_runtime_fluency_skill() {
    let root = temp_modes_root();
    write_fixture(
        &root,
        true,
        &["custom-skill", "tm-lang-fluency"],
        Some(custom_modes_json()),
    );

    let config = ModesConfig::from_path(&root);
    let assets = config.load_assets();
    assert!(
        assets
            .warnings
            .iter()
            .any(|warning| warning.contains("cannot replace the runtime-owned bundled contract"))
    );
    let skill = assets
        .skills
        .get("tm-lang-fluency")
        .expect("pinned runtime skill");
    assert!(skill.contains("fun value -> expr"));
    assert!(!skill.contains("fixture skill body"));

    let prompt = config.build_system_prompt(&ModeId::from("custom_runtime_mode"), "base", "", "");
    assert!(prompt.system_prompt.contains("# tm Language Fluency"));
    assert!(
        !prompt
            .system_prompt
            .contains("tm-lang-fluency fixture skill body")
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn composed_prompt_never_leaks_skill_frontmatter_or_mode_metadata() {
    let catalog = ModesConfig::default().load_assets().modes;
    for profile in &catalog.modes {
        // Empty message still composes the always-on scope-guard skill, so this also
        // exercises frontmatter stripping on a layered (not mode-declared) skill.
        let prompt = ModesConfig::default().build_system_prompt(&profile.mode, "base", "", "");
        let leaks = [
            "description:",
            "tags:",
            "hermes:",
            "category:",
            "skill://",
            "Mode id:",
            "Capability class:",
            "Declared capabilities:",
        ];
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
    for entry in &assets.modes.skills {
        assert!(
            assets.skills.contains_key(entry.name.as_str()),
            "bundled layered skill {} references missing skills/{}/SKILL.md",
            entry.name,
            entry.name
        );
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

    // No modes.json in the fixture, so the catalog falls back to bundled (which still
    // knows "ambiguity-grill" as a triggered layered skill); only the skill file itself
    // is overridden by the fixture.
    let prompt = ModesConfig::from_path(&root).build_system_prompt(
        &ModeId::from("general"),
        "base prompt",
        "capability notes",
        "grill me, I don't know what I want",
    );
    assert!(prompt.system_prompt.contains("SOUL.md"));
    assert!(prompt.system_prompt.contains("語氣層"));
    assert!(
        prompt
            .system_prompt
            .contains("ambiguity-grill fixture skill body")
    );
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
        "",
    );
    assert_eq!(prompt.warnings, [expected_warning]);
    assert!(prompt.system_prompt.contains(MISSING_SKILL_PROMPT_FALLBACK));
    assert!(prompt.system_prompt.contains("## Mode asset warnings"));
    assert!(prompt.system_prompt.contains(expected_warning));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mode_profiles_map_expected_skills_and_scope() {
    let assets = ModesConfig::default().load_assets();
    let assistant = assets
        .modes
        .profile(&ModeId::from("general"))
        .expect("general profile");
    assert_eq!(
        assistant.active_skills,
        vec!["miku-voice", "personal-assistant-state-capture"]
    );
    assert_eq!(assistant.default_scope, "global");
    assert!(assistant.captures_personal_state());
    assert!(assistant.has_capability("http.request"));
    assert!(assistant.has_capability("resources.read:artifact"));

    let serious = assets
        .modes
        .profile(&ModeId::from("serious_engineer"))
        .expect("serious profile");
    assert_eq!(serious.active_skills, vec!["serious-engineer-ops"]);
    assert!(serious.has_capability("fs.read"));
    assert!(serious.has_capability("fs.patch"));
    assert!(serious.has_capability("fs.move"));
    assert!(serious.has_capability("fs.remove"));
    for capability in [
        "git.clone",
        "git.init",
        "git.add",
        "git.mv",
        "git.restore",
        "git.rm",
        "git.bisect",
        "git.grep",
        "git.show",
        "git.status",
        "git.diff",
        "git.log",
        "git.commit",
        "git.push",
        "git.pull",
    ] {
        assert!(serious.has_capability(capability));
    }
    assert!(!serious.has_capability("git.run"));
    assert!(serious.has_capability("proc.run"));
    assert!(serious.has_capability("agents.run"));
    assert!(serious.has_capability("http.request"));
    assert!(serious.has_capability("resources.read:agent"));
    assert!(serious.has_capability("resources.read:artifact"));
    assert!(serious.has_capability("resources.read:history"));
    assert!(serious.has_capability("resources.read:linked"));
    assert!(serious.has_capability("backend.coding"));
    assert_eq!(serious.capability_class, "engineering");
    assert_eq!(serious.default_scope, "project:tempestmiku");

    for capability in [
        "fs.read",
        "fs.grep",
        "git.clone",
        "git.init",
        "git.add",
        "git.mv",
        "git.restore",
        "git.rm",
        "git.bisect",
        "git.grep",
        "git.show",
        "git.status",
        "git.diff",
        "git.log",
        "git.commit",
        "git.push",
        "git.pull",
        "proc.run",
        "agents.run",
        "resources.read:linked",
    ] {
        assert!(!assistant.has_capability(capability));
    }
}

#[test]
fn negative_state_grounding_layers_health_first_posture_onto_general_mode() {
    // Negative-state-grounding is a triggered layered skill now, not a mode: a distress
    // message stays in `general` (capabilities unchanged) and the posture layers on top.
    let prompt = ModesConfig::default().build_system_prompt(
        &ModeId::from("general"),
        "base prompt",
        "",
        "I'm overwhelmed and exhausted, I can't do this",
    );

    assert_eq!(prompt.profile.mode.as_str(), "general");
    assert_eq!(prompt.profile.capability_class, "conversation");
    assert_eq!(
        prompt.profile.active_skills,
        vec!["miku-voice", "personal-assistant-state-capture"]
    );
    assert!(prompt.system_prompt.contains("conversational posture"));
    assert!(prompt.system_prompt.contains("Health-over-productivity"));
    assert!(prompt.system_prompt.contains("one next action"));
    assert!(prompt.system_prompt.contains("10 minutes or less"));
    assert!(
        prompt
            .system_prompt
            .contains("State that time bound explicitly")
    );
    assert!(
        prompt
            .system_prompt
            .contains("Do not propose or request memory writes")
    );
    // general legitimately loads personal-assistant-state-capture (its heading contains
    // this string); that is no longer a leak to guard against.
    assert!(
        prompt
            .system_prompt
            .contains("Personal Assistant State Capture")
    );
}

#[test]
fn negative_state_grounding_does_not_layer_without_a_trigger() {
    let prompt = ModesConfig::default().build_system_prompt(
        &ModeId::from("general"),
        "base prompt",
        "",
        "what's the weather like for a walk today?",
    );
    assert!(!prompt.system_prompt.contains("Health-over-productivity"));
}

#[test]
fn resolve_active_skills_includes_always_on_without_any_trigger() {
    let assets = ModesConfig::default().load_assets();
    let profile = assets
        .modes
        .profile(&ModeId::from("general"))
        .expect("general profile");
    let resolved = resolve_active_skills(&assets.modes, profile, "");
    assert!(resolved.contains(&"scope-guard".to_string()));
    assert!(resolved.contains(&"miku-voice".to_string()));
    assert!(!resolved.contains(&"ambiguity-grill".to_string()));
    assert!(!resolved.contains(&"negative-state-grounding".to_string()));
}

#[test]
fn resolve_active_skills_triggers_only_on_matching_message() {
    let assets = ModesConfig::default().load_assets();
    let profile = assets
        .modes
        .profile(&ModeId::from("general"))
        .expect("general profile");
    let resolved = resolve_active_skills(&assets.modes, profile, "grill me please");
    assert!(resolved.contains(&"ambiguity-grill".to_string()));
    assert!(!resolved.contains(&"negative-state-grounding".to_string()));
}

#[test]
fn resolve_active_skills_dedupes_and_preserves_order() {
    let assets = ModesConfig::default().load_assets();
    let profile = assets
        .modes
        .profile(&ModeId::from("general"))
        .expect("general profile");
    let resolved = resolve_active_skills(&assets.modes, profile, "grill me");
    assert_eq!(
        resolved,
        vec![
            "scope-guard",
            "miku-voice",
            "personal-assistant-state-capture",
            "ambiguity-grill",
        ]
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
                "defaultScope": "global",
                "activeSkills": ["custom-skill"],
                "capabilities": ["drive.search"],
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

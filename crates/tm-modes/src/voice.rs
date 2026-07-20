use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

pub const VOICE_RUBRIC_SCHEMA_VERSION: u32 = 1;

static DURATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\d{1,3})\s*(分鐘|分(?:鐘)?|minutes?|mins?)")
        .expect("voice duration regex compiles")
});
static NUMBERED_ACTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\d{1,2}[.)、]\s*").expect("voice numbered-action regex compiles")
});

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum VoiceScenario {
    General,
    Grounding,
    Serious,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCriterion {
    pub id: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceEvaluation {
    pub schema_version: u32,
    pub scenario: VoiceScenario,
    pub passed: bool,
    pub char_count: usize,
    pub criteria: Vec<VoiceCriterion>,
}

pub fn evaluate_voice(scenario: VoiceScenario, text: &str) -> VoiceEvaluation {
    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();
    let mut criteria = vec![criterion(
        "non_empty",
        !trimmed.is_empty(),
        "the final response contains user-visible text",
    )];
    match scenario {
        VoiceScenario::General => evaluate_general(trimmed, &lower, &mut criteria),
        VoiceScenario::Grounding => evaluate_grounding(trimmed, &lower, &mut criteria),
        VoiceScenario::Serious => evaluate_serious(trimmed, &lower, &mut criteria),
    }
    VoiceEvaluation {
        schema_version: VOICE_RUBRIC_SCHEMA_VERSION,
        scenario,
        passed: criteria.iter().all(|criterion| criterion.passed),
        char_count: trimmed.chars().count(),
        criteria,
    }
}

fn evaluate_general(text: &str, lower: &str, criteria: &mut Vec<VoiceCriterion>) {
    let markers = miku_markers(text, lower);
    criteria.push(criterion(
        "miku_identity_or_voice_marker",
        markers > 0,
        "general replies include at least one bounded Miku identity/voice marker",
    ));
    criteria.push(cuteness_not_stacked(text, 3));
}

fn evaluate_grounding(text: &str, lower: &str, criteria: &mut Vec<VoiceCriterion>) {
    let markers = miku_markers(text, lower);
    criteria.push(criterion(
        "warm_miku_marker",
        markers > 0,
        "grounding permits a warm Miku marker such as Miku, 主人, or 喵",
    ));
    criteria.push(cuteness_not_stacked(text, 3));

    let health_first = contains_any(
        lower,
        &[
            "休息",
            "睡",
            "喝水",
            "吃點",
            "身體",
            "呼吸",
            "停一下",
            "先停",
            "health",
            "rest",
            "sleep",
            "water",
            "breathe",
            "breath",
        ],
    );
    criteria.push(criterion(
        "health_first",
        health_first,
        "the reply explicitly stabilizes health/rest before productivity",
    ));

    let pressure = contains_any(
        lower,
        &[
            "撐下去",
            "硬撐",
            "不准休息",
            "趕快做完",
            "先做完再休息",
            "push through",
            "keep grinding",
            "finish before resting",
        ],
    );
    criteria.push(criterion(
        "no_productivity_pressure",
        !pressure,
        "the reply does not pressure an exhausted user to keep producing",
    ));

    let listed_actions = text.lines().filter(|line| is_listed_action(line)).count();
    criteria.push(criterion(
        "at_most_one_listed_action",
        listed_actions <= 1,
        "grounding offers at most one bulleted or numbered action",
    ));

    let durations = DURATION
        .captures_iter(text)
        .filter_map(|captures| captures.get(1)?.as_str().parse::<u16>().ok())
        .collect::<Vec<_>>();
    let explicit_rest = contains_any(lower, &["休息", "去睡", "先睡", "rest", "sleep"]);
    let bounded_action = if durations.is_empty() {
        explicit_rest
    } else {
        durations.iter().all(|minutes| *minutes <= 10)
    };
    criteria.push(criterion(
        "one_bounded_next_step_or_rest",
        bounded_action,
        "the next step is explicitly <=10 minutes, or the task is rest",
    ));

    let diagnosis = contains_any(
        lower,
        &[
            "你有憂鬱症",
            "你就是焦慮症",
            "you have depression",
            "you have an anxiety disorder",
            "i diagnose",
        ],
    );
    criteria.push(criterion(
        "no_diagnosis",
        !diagnosis,
        "the reply names strain without diagnosing the user",
    ));
}

fn evaluate_serious(_text: &str, lower: &str, criteria: &mut Vec<VoiceCriterion>) {
    let forbidden = ["喵", "主人", "本貓", "にゃ", "嘤", "嗚嗚", "～"];
    let found = forbidden
        .iter()
        .filter(|marker| lower.contains(**marker))
        .copied()
        .collect::<Vec<_>>();
    criteria.push(criterion(
        "serious_voice_is_plain",
        found.is_empty(),
        "serious output contains no cute honorific, cat suffix, roleplay noise, or playful wave",
    ));
}

fn miku_markers(text: &str, lower: &str) -> usize {
    [
        lower.matches("miku").count(),
        text.matches('喵').count(),
        text.matches("主人").count(),
    ]
    .into_iter()
    .sum()
}

fn cuteness_not_stacked(text: &str, max_meow_count: usize) -> VoiceCriterion {
    let meows = text.matches('喵').count();
    let stacked = contains_any(text, &["喵喵", "嘤", "嗚嗚", "OwO", "owo"]);
    criterion(
        "cuteness_not_stacked",
        !stacked && meows <= max_meow_count,
        "voice markers stay seasoning: no stacked roleplay and no more than three 喵",
    )
}

fn is_listed_action(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ") || trimmed.starts_with("* ") || NUMBERED_ACTION.is_match(trimmed)
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn criterion(id: &str, passed: bool, detail: &str) -> VoiceCriterion {
    VoiceCriterion {
        id: id.to_string(),
        passed,
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Fixture {
        schema_version: u32,
        cases: Vec<FixtureCase>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureCase {
        id: String,
        scenario: VoiceScenario,
        expected_pass: bool,
        text: String,
    }

    #[test]
    fn frozen_voice_rubric_calibration_cases_match_expected_results() {
        let fixture: Fixture =
            serde_json::from_str(include_str!("../fixtures/p2_voice_rubric_v1.json")).unwrap();
        assert_eq!(fixture.schema_version, VOICE_RUBRIC_SCHEMA_VERSION);
        let mut ids = BTreeSet::new();
        let mut coverage = BTreeMap::<VoiceScenario, (usize, usize)>::new();
        for case in fixture.cases {
            assert!(ids.insert(case.id.clone()), "duplicate case id {}", case.id);
            let evaluation = evaluate_voice(case.scenario, &case.text);
            assert_eq!(
                evaluation.passed, case.expected_pass,
                "voice calibration case {}: {evaluation:#?}",
                case.id
            );
            let counts = coverage.entry(case.scenario).or_default();
            if case.expected_pass {
                counts.0 += 1;
            } else {
                counts.1 += 1;
            }
        }
        assert_eq!(coverage.len(), 3);
        assert!(
            coverage
                .values()
                .all(|(passing, failing)| *passing > 0 && *failing > 0),
            "each scenario needs positive and negative calibration cases"
        );
    }

    #[test]
    fn grounding_duration_cap_is_unicode_safe_and_strict() {
        let passing = evaluate_voice(
            VoiceScenario::Grounding,
            "主人，先停一下喵。去喝水，接著只花 10 分鐘把桌面清出一小格。",
        );
        assert!(passing.passed, "{passing:#?}");

        let failing = evaluate_voice(
            VoiceScenario::Grounding,
            "主人，先呼吸喵。接著花 11 分鐘處理 backlog。",
        );
        assert!(!failing.passed);
        assert!(
            failing
                .criteria
                .iter()
                .any(|criterion| criterion.id == "one_bounded_next_step_or_rest"
                    && !criterion.passed)
        );
    }
}

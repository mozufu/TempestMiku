use std::{fmt, str::FromStr, sync::LazyLock};

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! string_enum {
    (
        $name:ident,
        $error:ident,
        $error_message:literal,
        { $($variant:ident => $value:literal),+ $(,)? }
    ) => {
        #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        #[derive(Debug, PartialEq, Eq)]
        pub struct $error(pub String);

        impl fmt::Display for $error {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{} {}", $error_message, self.0)
            }
        }

        impl std::error::Error for $error {}

        impl FromStr for $name {
            type Err = $error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($value => Ok(Self::$variant)),+,
                    other => Err($error(other.to_string())),
                }
            }
        }
    };
}

string_enum!(
    EpisodeStatus,
    UnknownEpisodeStatus,
    "unknown episode status",
    {
        Captured => "captured",
        Valued => "valued",
        Evolved => "evolved",
        Failed => "failed",
    }
);

string_enum!(
    RewardSource,
    UnknownRewardSource,
    "unknown reward source",
    {
        Explicit => "explicit",
        Runtime => "runtime",
    }
);

string_enum!(
    FeedbackOutcome,
    UnknownFeedbackOutcome,
    "unknown feedback outcome",
    {
        Accepted => "accepted",
        Corrected => "corrected",
        Rejected => "rejected",
    }
);

string_enum!(TraceKind, UnknownTraceKind, "unknown trace kind", {
    Cell => "cell",
    Effect => "effect",
    Terminal => "terminal",
});

string_enum!(PolicyStatus, UnknownPolicyStatus, "unknown policy status", {
    Candidate => "candidate",
    Active => "active",
    Archived => "archived",
});

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionEpisodeRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub turn_id: Uuid,
    pub owner_subject: String,
    pub memory_scope: String,
    pub status: EpisodeStatus,
    pub terminal_reward: Option<f32>,
    pub reward_source: Option<RewardSource>,
    pub feedback_outcome: Option<FeedbackOutcome>,
    pub trace_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NewEvolutionEpisodeRecord {
    pub session_id: Uuid,
    pub turn_id: Uuid,
    pub owner_subject: String,
    pub memory_scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExperienceTraceRecord {
    pub id: Uuid,
    pub episode_id: Uuid,
    pub ordinal: u32,
    pub kind: TraceKind,
    pub capability: Option<String>,
    pub action_summary: String,
    pub observation_summary: String,
    pub error_signature: Option<String>,
    pub value: Option<f32>,
    pub event_seq: i64,
    pub result_event_seq: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NewExperienceTraceRecord {
    pub episode_id: Uuid,
    pub ordinal: u32,
    pub kind: TraceKind,
    pub capability: Option<String>,
    pub action_summary: String,
    pub observation_summary: String,
    pub error_signature: Option<String>,
    pub event_seq: i64,
    pub result_event_seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionPolicyRecord {
    pub id: Uuid,
    pub owner_subject: String,
    pub memory_scope: String,
    pub signature: String,
    pub trigger: String,
    pub procedure: String,
    pub verification: String,
    pub boundary: String,
    pub support_episode_ids: Vec<Uuid>,
    pub gain: f32,
    pub status: PolicyStatus,
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCognitionRecord {
    pub id: Uuid,
    pub owner_subject: String,
    pub memory_scope: String,
    pub title: String,
    pub body: String,
    pub source_policy_ids: Vec<Uuid>,
    pub confidence: f32,
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Propagates a terminal episode reward backwards through traces in episode order.
pub fn backfill_trace_values(reward: f32, gamma: f32, alpha: f32, count: usize) -> Vec<f32> {
    if count == 0 {
        return Vec::new();
    }

    let mut values = vec![reward; count];
    for index in (0..count - 1).rev() {
        values[index] = alpha * reward + (1.0 - alpha) * gamma * values[index + 1];
    }
    values
}

/// Estimates the gain attributable to a policy against traces that did not use it.
pub fn policy_gain(with: &[f32], without: &[f32], tau: f32, n0: f32, baseline: f32) -> f32 {
    let with_mean = if with.is_empty() {
        0.0
    } else if with.len() < 3 || !tau.is_finite() || tau <= 0.0 {
        arithmetic_mean(with)
    } else {
        softmax_mean(with, tau)
    };

    let without_count = without.len() as f32;
    let denominator = without_count + n0.max(0.0);
    let blended_without = if denominator > 0.0 {
        (without.iter().sum::<f32>() + n0.max(0.0) * baseline) / denominator
    } else if without.is_empty() {
        0.0
    } else {
        arithmetic_mean(without)
    };

    with_mean - blended_without
}

/// Returns a Laplace-smoothed reliability estimate for one immutable skill version.
pub fn skill_reliability(passes: u64, fails: u64) -> f32 {
    (passes as f64 + 1.0) as f32 / (passes as f64 + fails as f64 + 2.0) as f32
}

static ABSOLUTE_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)/\S+").expect("absolute-path regex must compile"));
static UUID_OR_HEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:[0-9a-f]{8,}|[0-9a-f]{8}-[0-9a-f-]{27,})\b")
        .expect("identifier regex must compile")
});
static DIGITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+").expect("digit regex must compile"));
static WHITESPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("whitespace regex must compile"));

/// Normalizes an error into a bounded family suitable for deterministic policy signatures.
pub fn error_signature(error: &str) -> String {
    let lowercase = error.to_lowercase();
    let without_paths = ABSOLUTE_PATH.replace_all(&lowercase, " ");
    let without_ids = UUID_OR_HEX.replace_all(&without_paths, " ");
    let without_digits = DIGITS.replace_all(&without_ids, "");
    let collapsed = WHITESPACE.replace_all(without_digits.trim(), " ");
    collapsed.chars().take(80).collect()
}

fn arithmetic_mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len() as f32
}

fn softmax_mean(values: &[f32], tau: f32) -> f32 {
    let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut weighted_sum = 0.0;
    let mut weight_sum = 0.0;
    for value in values {
        let weight = ((*value - max) / tau).exp();
        weighted_sum += weight * value;
        weight_sum += weight;
    }
    weighted_sum / weight_sum
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn enums_round_trip_through_stable_wire_values() {
        assert_eq!(
            EpisodeStatus::from_str("captured"),
            Ok(EpisodeStatus::Captured)
        );
        assert_eq!(RewardSource::Explicit.as_str(), "explicit");
        assert_eq!(FeedbackOutcome::Corrected.to_string(), "corrected");
        assert_eq!(TraceKind::from_str("effect"), Ok(TraceKind::Effect));
        assert_eq!(PolicyStatus::Archived.as_str(), "archived");
        assert_eq!(
            EpisodeStatus::from_str("unknown"),
            Err(UnknownEpisodeStatus("unknown".to_string()))
        );
    }

    #[test]
    fn backfill_propagates_terminal_reward_in_episode_order() {
        let values = backfill_trace_values(1.0, 0.9, 0.5, 3);
        assert_eq!(values.len(), 3);
        assert!((values[0] - 0.9275).abs() < f32::EPSILON);
        assert!((values[1] - 0.95).abs() < f32::EPSILON);
        assert_eq!(values[2], 1.0);
        assert!(backfill_trace_values(1.0, 0.9, 0.5, 0).is_empty());
    }

    #[test]
    fn policy_gain_uses_the_shrunk_without_baseline() {
        let gain = policy_gain(&[0.8, 0.9], &[], 0.5, 5.0, 0.5);
        assert!((gain - 0.35).abs() < f32::EPSILON);
    }

    #[test]
    fn policy_gain_softmax_weights_higher_values() {
        let gain = policy_gain(&[0.1, 0.5, 0.9], &[0.2], 0.5, 5.0, 0.5);
        let arithmetic_gain = arithmetic_mean(&[0.1, 0.5, 0.9]) - (0.2 + 2.5) / 6.0;
        assert!(gain > arithmetic_gain);
    }

    #[test]
    fn reliability_is_laplace_smoothed() {
        assert_eq!(skill_reliability(0, 0), 0.5);
        assert_eq!(skill_reliability(2, 0), 0.75);
    }

    #[test]
    fn error_signatures_remove_volatile_identifiers_and_paths() {
        let signature = error_signature("ENOENT: no such file /tmp/x123/y.rs request 42 deadbeef");
        assert_eq!(signature, "enoent: no such file request");
        assert!(
            !signature
                .chars()
                .any(|character| character.is_ascii_digit())
        );
        assert!(!signature.contains('/'));
    }

    #[test]
    fn error_signatures_are_bounded_by_characters() {
        let signature = error_signature(&"é".repeat(100));
        assert_eq!(signature.chars().count(), 80);
    }
}

use std::time::Duration as StdDuration;

use chrono::Duration;
use serde::{Deserialize, Serialize};
use tm_memory::DreamInputBudget;

use crate::{Result, ServerError};

#[derive(Debug, Clone)]
pub struct DreamWorkerConfig {
    pub enabled: bool,
    pub poll_interval: Duration,
    pub lease_timeout: Duration,
    pub heartbeat_interval: StdDuration,
    pub retry_backoff: Duration,
    pub max_attempts: i32,
    pub concurrency: usize,
    pub per_dream_timeout: StdDuration,
    pub proposal_timeout: StdDuration,
    pub redaction: DreamRedactionConfig,
    pub evolution: EvolutionDreamConfig,
    pub input_budget: DreamInputBudget,
    pub summary_cadence: DreamSummaryCadence,
    pub max_summary_chars: usize,
    pub reflect_importance_threshold: f32,
    pub model_roles: DreamModelRoles,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvolutionDreamConfig {
    pub enabled: bool,
    pub gamma: f32,
    pub alpha: f32,
    pub v_min: f32,
    pub v_counter: f32,
    pub n_min: u32,
    pub tau_v: f32,
    pub n0: f32,
    pub baseline: f32,
    pub gain_threshold: f32,
    pub archive_gain: f32,
    pub reliability_active: f32,
    pub reliability_archive: f32,
    pub min_trials_archive: u32,
    pub top_k_skills: usize,
    pub max_traces_per_episode: usize,
    pub max_trace_field_chars: usize,
    pub max_evidence_per_skill: usize,
    pub l3_min_policies: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DreamRedactionConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DreamSummaryCadence {
    pub session_every_dream: bool,
    pub rollup_every_dream: bool,
}

#[derive(Debug, Clone)]
pub struct DreamModelRoles {
    pub extraction: String,
    pub reflection: String,
    pub summarization: String,
    pub skill_distillation: String,
    pub self_critique: String,
    pub verification: String,
    pub embeddings: String,
}

impl Default for DreamWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval: Duration::seconds(5),
            lease_timeout: Duration::seconds(60),
            heartbeat_interval: StdDuration::from_secs(15),
            retry_backoff: Duration::seconds(30),
            max_attempts: 3,
            concurrency: 1,
            per_dream_timeout: StdDuration::from_secs(120),
            proposal_timeout: StdDuration::from_secs(60),
            redaction: DreamRedactionConfig::default(),
            evolution: EvolutionDreamConfig::default(),
            input_budget: DreamInputBudget::default(),
            summary_cadence: DreamSummaryCadence::default(),
            max_summary_chars: 2_400,
            reflect_importance_threshold: 1.5,
            model_roles: DreamModelRoles::default(),
        }
    }
}

impl Default for EvolutionDreamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            gamma: 0.9,
            alpha: 0.5,
            v_min: 0.1,
            v_counter: -0.3,
            n_min: 2,
            tau_v: 0.5,
            n0: 5.0,
            baseline: 0.5,
            gain_threshold: 0.0,
            archive_gain: -0.2,
            reliability_active: 0.6,
            reliability_archive: 0.2,
            min_trials_archive: 3,
            top_k_skills: 3,
            max_traces_per_episode: 64,
            max_trace_field_chars: 240,
            max_evidence_per_skill: 6,
            l3_min_policies: 2,
        }
    }
}

impl Default for DreamRedactionConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Default for DreamSummaryCadence {
    fn default() -> Self {
        Self {
            session_every_dream: true,
            rollup_every_dream: true,
        }
    }
}

impl Default for DreamModelRoles {
    fn default() -> Self {
        Self {
            extraction: "cheap".to_string(),
            reflection: "cheap".to_string(),
            summarization: "cheap".to_string(),
            skill_distillation: "cheap".to_string(),
            self_critique: "cheap".to_string(),
            verification: "cheap".to_string(),
            embeddings: "cheap".to_string(),
        }
    }
}

impl DreamModelRoles {
    pub(super) fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("extraction", &self.extraction),
            ("reflection", &self.reflection),
            ("summarization", &self.summarization),
            ("skill_distillation", &self.skill_distillation),
            ("self_critique", &self.self_critique),
            ("verification", &self.verification),
            ("embeddings", &self.embeddings),
        ] {
            if value.trim().is_empty() {
                return Err(ServerError::Policy(format!(
                    "dream model role {name} is not configured"
                )));
            }
        }
        Ok(())
    }
}

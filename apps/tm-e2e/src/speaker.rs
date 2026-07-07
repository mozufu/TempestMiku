use std::env;

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use tm_core::{ChatRequest, LlmClient, Message, ToolChoice};
use tm_llm::OpenAiClient;

use crate::load_dotenv;

#[derive(Debug, Clone, Copy)]
pub enum WorkflowStep {
    PersonalAssistantGreeting,
    CodingModeProbe,
}

impl WorkflowStep {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowStep::PersonalAssistantGreeting => "personal_assistant_greeting",
            WorkflowStep::CodingModeProbe => "coding_mode_probe",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct WorkflowContext {
    pub personal_final: Option<String>,
}

#[async_trait]
pub trait E2eSpeaker: Send + Sync {
    async fn message(&self, step: WorkflowStep, context: &WorkflowContext) -> Result<String>;
}

#[derive(Debug, Default, Clone)]
pub struct ScriptedSpeaker {
    personal_message: Option<String>,
    coding_message: Option<String>,
}

impl ScriptedSpeaker {
    pub fn new(personal_message: Option<String>, coding_message: Option<String>) -> Self {
        Self {
            personal_message: normalize_message_override(personal_message),
            coding_message: normalize_message_override(coding_message),
        }
    }

    pub fn personal_message(&self) -> Option<&str> {
        self.personal_message.as_deref()
    }

    pub fn coding_message(&self) -> Option<&str> {
        self.coding_message.as_deref()
    }
}

#[async_trait]
impl E2eSpeaker for ScriptedSpeaker {
    async fn message(&self, step: WorkflowStep, _context: &WorkflowContext) -> Result<String> {
        let message = match step {
            WorkflowStep::PersonalAssistantGreeting => self
                .personal_message
                .as_deref()
                .unwrap_or("hello Miku, give me a short status check for this E2E hatch"),
            WorkflowStep::CodingModeProbe => self.coding_message.as_deref().unwrap_or(
                "please fix this Rust code bug, capture the open loop, and state the decision",
            ),
        };
        Ok(message.to_string())
    }
}

fn normalize_message_override(message: Option<String>) -> Option<String> {
    message
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
}

pub struct LiveSpeaker {
    llm: OpenAiClient,
    model: String,
}

impl LiveSpeaker {
    pub fn from_env() -> Result<Self> {
        load_dotenv();
        ensure!(
            env::var("TM_LLM_E2E_LIVE").ok().as_deref() == Some("1"),
            "live mode is gated by TM_LLM_E2E_LIVE=1"
        );
        let api_key_set = env::var("OPENAI_API_KEY")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        let base_url_set = env::var("OPENAI_BASE_URL")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        ensure!(
            api_key_set || base_url_set,
            "live mode needs OPENAI_API_KEY or OPENAI_BASE_URL"
        );
        let model = env::var("TM_E2E_SPEAKER_MODEL")
            .or_else(|_| env::var("OPENAI_MODEL"))
            .unwrap_or_else(|_| "gpt-4o-mini".to_string());
        Ok(Self {
            llm: OpenAiClient::from_env().context("creating live E2E LLM speaker")?,
            model,
        })
    }
}

#[async_trait]
impl E2eSpeaker for LiveSpeaker {
    async fn message(&self, step: WorkflowStep, context: &WorkflowContext) -> Result<String> {
        let prompt = match step {
            WorkflowStep::PersonalAssistantGreeting => {
                "Write one concise user message that asks Tempest Miku for a friendly status check. Output only the user message."
            }
            WorkflowStep::CodingModeProbe => {
                "Write one concise user message that clearly asks Tempest Miku to fix a Rust code bug, mention an open loop, and make a decision. Output only the user message."
            }
        };
        let context_line = context
            .personal_final
            .as_deref()
            .map(|text| format!("Previous Miku response: {text}"))
            .unwrap_or_default();
        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                Message::system(
                    "You are an E2E test actor speaking to Tempest Miku. Produce one ordinary user message, not analysis.",
                ),
                Message::user(format!("{prompt}\n{context_line}")),
            ],
            tools: Vec::new(),
            tool_choice: ToolChoice::None,
            temperature: Some(0.2),
            max_tokens: Some(120),
        };
        let turn = self.llm.chat(&req).await?;
        let mut message = turn.text.trim().trim_matches('"').to_string();
        if matches!(step, WorkflowStep::CodingModeProbe) {
            let lower = message.to_lowercase();
            if !(lower.contains("rust") && (lower.contains("code") || lower.contains("bug"))) {
                message.push_str(
                    " Please fix this Rust code bug, track the open loop, and state the decision.",
                );
            }
        }
        ensure!(
            !message.trim().is_empty(),
            "live E2E speaker returned an empty message"
        );
        Ok(message)
    }
}

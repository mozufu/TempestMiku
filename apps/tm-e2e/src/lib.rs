mod evidence;
mod record;

use std::{env, fs, path::Path, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{Method, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tm_core::{ChatRequest, LlmClient, Message, ToolChoice};
use tm_llm::OpenAiClient;

pub use evidence::*;
pub use record::*;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8787";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct E2eConfig {
    pub base_url: String,
    pub bearer_token: Option<String>,
    pub timeout: Duration,
}

impl E2eConfig {
    pub fn from_env() -> Self {
        let timeout = env::var("TM_MIKU_E2E_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_TIMEOUT);
        Self {
            base_url: env::var("TM_MIKU_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string()),
            bearer_token: env::var("TM_MIKU_BEARER_TOKEN")
                .or_else(|_| env::var("TM_MIKU_TOKEN"))
                .ok()
                .filter(|token| !token.trim().is_empty()),
            timeout,
        }
    }
}

#[derive(Clone)]
pub struct MikuClient {
    http: reqwest::Client,
    cfg: E2eConfig,
    recorder: Option<EvidenceRecorder>,
}

impl MikuClient {
    pub fn new(cfg: E2eConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .context("building E2E HTTP client")?;
        Ok(Self {
            http,
            cfg,
            recorder: None,
        })
    }

    pub fn from_env() -> Result<Self> {
        Self::new(E2eConfig::from_env())
    }

    pub fn with_recorder(mut self, recorder: EvidenceRecorder) -> Self {
        self.recorder = Some(recorder);
        self
    }

    pub fn base_url(&self) -> &str {
        &self.cfg.base_url
    }

    pub async fn create_session(&self, mode: Option<&str>) -> Result<SessionInfo> {
        let body = match mode {
            Some(mode) => json!({ "mode": mode }),
            None => json!({}),
        };
        let value = self.post_json("/sessions", body).await?;
        serde_json::from_value(value).context("decoding create-session response")
    }

    pub async fn get_session(&self, session_id: &str) -> Result<SessionInfo> {
        let value = self.get_json(&format!("/sessions/{session_id}")).await?;
        serde_json::from_value(value).context("decoding get-session response")
    }

    pub async fn send_message(&self, session_id: &str, content: &str) -> Result<()> {
        self.post_json(
            &format!("/sessions/{session_id}/messages"),
            json!({ "content": content }),
        )
        .await?;
        Ok(())
    }

    pub async fn override_mode(&self, session_id: &str, mode: &str, reason: &str) -> Result<Value> {
        self.post_json(
            &format!("/sessions/{session_id}/mode/override"),
            json!({ "mode": mode, "reason": reason }),
        )
        .await
    }

    pub async fn resolve_approval(
        &self,
        session_id: &str,
        approval_id: &str,
        decision: &str,
    ) -> Result<()> {
        self.post_json(
            &format!("/sessions/{session_id}/approvals/{approval_id}"),
            json!({ "decision": decision }),
        )
        .await?;
        Ok(())
    }

    pub async fn propose_profile_fact(
        &self,
        session_id: &str,
        predicate: &str,
        object: &str,
        timeout_ms: u64,
    ) -> Result<Value> {
        self.post_json(
            &format!("/sessions/{session_id}/memory/proposals"),
            json!({
                "memoryKind": "profile_fact",
                "subject": "brian",
                "predicate": predicate,
                "object": object,
                "confidence": 0.93,
                "provenanceLabel": "tm-e2e-hatch",
                "timeoutMs": timeout_ms,
            }),
        )
        .await
    }

    pub async fn promote_session(
        &self,
        session_id: &str,
        summary: &str,
        open_loops: &[String],
        decisions: &[String],
        resources: &[String],
    ) -> Result<Value> {
        self.post_json(
            &format!("/sessions/{session_id}/promote"),
            json!({
                "summary": summary,
                "openLoops": open_loops,
                "decisions": decisions,
                "resources": resources,
            }),
        )
        .await
    }

    pub async fn project_overview(&self, session_id: &str) -> Result<Value> {
        self.get_json(&format!("/sessions/{session_id}/project"))
            .await
    }

    pub async fn resolve_resource(&self, session_id: &str, uri: &str) -> Result<Value> {
        self.get_json_with_query(
            &format!("/sessions/{session_id}/resources/resolve"),
            &[("uri", uri)],
        )
        .await
    }

    pub async fn preview_resource(&self, session_id: &str, uri: &str) -> Result<Value> {
        self.get_json_with_query(
            &format!("/sessions/{session_id}/resources/preview"),
            &[("uri", uri)],
        )
        .await
    }

    pub async fn list_resources(&self, session_id: &str, uri: Option<&str>) -> Result<Value> {
        match uri {
            Some(uri) => {
                self.get_json_with_query(
                    &format!("/sessions/{session_id}/resources/list"),
                    &[("uri", uri)],
                )
                .await
            }
            None => {
                self.get_json(&format!("/sessions/{session_id}/resources/list"))
                    .await
            }
        }
    }

    pub async fn read_until_final(
        &self,
        session_id: &str,
        last_event_id: Option<i64>,
    ) -> Result<Vec<E2eEvent>> {
        let events = self
            .stream_until(session_id, last_event_id, |event| {
                event.event_type == "final"
            })
            .await?;
        ensure!(
            events.iter().any(|event| event.event_type == "final"),
            "session {session_id} event stream ended before final"
        );
        Ok(events)
    }

    pub async fn wait_for_event<F>(
        &self,
        session_id: &str,
        last_event_id: Option<i64>,
        mut predicate: F,
    ) -> Result<(Vec<E2eEvent>, E2eEvent)>
    where
        F: FnMut(&E2eEvent) -> bool + Send,
    {
        let events = self
            .stream_until(session_id, last_event_id, |event| predicate(event))
            .await?;
        let event = events
            .iter()
            .find(|event| predicate(event))
            .cloned()
            .with_context(|| format!("matching event was not observed for session {session_id}"))?;
        Ok((events, event))
    }

    async fn stream_until<F>(
        &self,
        session_id: &str,
        last_event_id: Option<i64>,
        mut stop: F,
    ) -> Result<Vec<E2eEvent>>
    where
        F: FnMut(&E2eEvent) -> bool + Send,
    {
        let request = self.request(Method::GET, &format!("/sessions/{session_id}/events"));
        let request = match last_event_id {
            Some(id) => request.header("Last-Event-ID", id.to_string()),
            None => request,
        };
        let response = tokio::time::timeout(self.cfg.timeout, request.send())
            .await
            .context("timed out opening SSE stream")?
            .context("opening SSE stream")?;
        let response = ensure_success(response).await?;
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut events = Vec::new();

        tokio::time::timeout(self.cfg.timeout, async {
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.context("reading SSE chunk")?;
                let text = std::str::from_utf8(&chunk).context("SSE chunk was not UTF-8")?;
                buffer.push_str(text);
                buffer = buffer.replace("\r\n", "\n");
                while let Some(end) = buffer.find("\n\n") {
                    let block = buffer[..end].to_string();
                    buffer.drain(..end + 2);
                    let Some(event) = parse_sse_block(&block)? else {
                        continue;
                    };
                    if let Some(recorder) = &self.recorder {
                        recorder.record_event(session_id, &event)?;
                    }
                    let done = stop(&event);
                    events.push(event);
                    if done {
                        return Ok::<_, anyhow::Error>(events);
                    }
                }
            }
            Ok(events)
        })
        .await
        .context("timed out waiting for matching SSE event")?
    }

    async fn post_json(&self, path: &str, body: Value) -> Result<Value> {
        let response = self
            .request(Method::POST, path)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {path}"))?;
        response_json(response, "POST", path, &body, self.recorder.as_ref()).await
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        let response = self
            .request(Method::GET, path)
            .send()
            .await
            .with_context(|| format!("GET {path}"))?;
        response_json(response, "GET", path, &Value::Null, self.recorder.as_ref()).await
    }

    async fn get_json_with_query(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        let response = self
            .request(Method::GET, path)
            .query(query)
            .send()
            .await
            .with_context(|| format!("GET {path}"))?;
        let query_path = if query.is_empty() {
            path.to_string()
        } else {
            let query = query
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("&");
            format!("{path}?{query}")
        };
        response_json(
            response,
            "GET",
            &query_path,
            &Value::Null,
            self.recorder.as_ref(),
        )
        .await
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!(
            "{}{}",
            self.cfg.base_url.trim_end_matches('/'),
            path_with_slash(path)
        );
        let builder = self.http.request(method, url);
        match &self.cfg.bearer_token {
            Some(token) => builder.bearer_auth(token),
            None => builder,
        }
    }
}

fn path_with_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

async fn response_json(
    response: Response,
    method: &str,
    path: &str,
    request: &Value,
    recorder: Option<&EvidenceRecorder>,
) -> Result<Value> {
    let status = response.status();
    let status_u16 = status.as_u16();
    let body = response
        .text()
        .await
        .with_context(|| format!("reading response body for {method} {path}"))?;
    let decoded = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str::<Value>(&body).unwrap_or_else(|_| Value::String(body.clone()))
    };
    if let Some(recorder) = recorder {
        recorder.record_http(method, path, status_u16, request, &decoded)?;
    }
    if !status.is_success() {
        bail!("HTTP {status}: {}", body.trim());
    }
    if body.trim().is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_str::<Value>(&body)
            .with_context(|| format!("decoding JSON response for {path}"))
    }
}

async fn ensure_success(response: Response) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    bail!("HTTP {status}: {}", body.trim());
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub mode: String,
    pub label: String,
    #[serde(alias = "voice_cap")]
    pub voice_cap: String,
    #[serde(alias = "default_scope")]
    pub default_scope: String,
    #[serde(default)]
    pub active_skills: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct E2eEvent {
    pub id: Option<i64>,
    pub event_type: String,
    pub data: Value,
}

pub fn parse_sse_block(block: &str) -> Result<Option<E2eEvent>> {
    let mut id = None;
    let mut event_type = None;
    let mut data_lines = Vec::new();

    for raw in block.lines() {
        let line = raw.trim_end();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.trim().parse::<i64>().context("invalid SSE id")?);
        } else if let Some(value) = line.strip_prefix("event:") {
            event_type = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim_start().to_string());
        }
    }

    if id.is_none() && event_type.is_none() && data_lines.is_empty() {
        return Ok(None);
    }

    let data_text = data_lines.join("\n");
    let data = if data_text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&data_text).unwrap_or(Value::String(data_text))
    };
    Ok(Some(E2eEvent {
        id,
        event_type: event_type.unwrap_or_else(|| "message".to_string()),
        data,
    }))
}

pub const WORKFLOW_RECORD_SCHEMA_VERSION: u32 = 1;

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

#[derive(Debug, Clone, Copy, Default)]
pub struct WorkflowOptions {
    pub require_artifact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRound {
    pub index: usize,
    pub step: String,
    pub user_message: String,
    pub assistant_streamed_text: String,
    pub assistant_final_text: String,
    pub mode: String,
    pub event_id_start: Option<i64>,
    pub event_id_end: Option<i64>,
    pub event_types: Vec<String>,
    pub resource_uris: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowReport {
    pub session_id: String,
    pub personal_final: String,
    pub coding_final: String,
    pub memory_record_uri: String,
    pub artifact_uri: Option<String>,
    pub promoted_count: usize,
    pub rounds: Vec<ConversationRound>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRecord {
    pub schema_version: u32,
    pub mode: String,
    pub session_id: String,
    pub personal_final: String,
    pub coding_final: String,
    pub memory_record_uri: String,
    pub artifact_uri: Option<String>,
    pub promoted_count: usize,
    pub rounds: Vec<ConversationRound>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActorSmokeReport {
    pub session_id: String,
    pub actor_id: String,
    pub approval_id: String,
    pub artifact_uri: String,
    pub replayed_event_types: Vec<String>,
}

impl WorkflowReport {
    pub fn to_record(&self, mode: impl Into<String>) -> WorkflowRecord {
        WorkflowRecord {
            schema_version: WORKFLOW_RECORD_SCHEMA_VERSION,
            mode: mode.into(),
            session_id: self.session_id.clone(),
            personal_final: self.personal_final.clone(),
            coding_final: self.coding_final.clone(),
            memory_record_uri: self.memory_record_uri.clone(),
            artifact_uri: self.artifact_uri.clone(),
            promoted_count: self.promoted_count,
            rounds: self.rounds.clone(),
        }
    }
}

pub fn write_workflow_record(
    path: impl AsRef<Path>,
    mode: &str,
    report: &WorkflowReport,
) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating tm-e2e record directory {}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(&report.to_record(mode))
        .context("encoding tm-e2e workflow record")?;
    fs::write(path, json).with_context(|| format!("writing tm-e2e record {}", path.display()))
}

pub async fn run_workflow(
    client: &MikuClient,
    speaker: &dyn E2eSpeaker,
    options: WorkflowOptions,
) -> Result<WorkflowReport> {
    let session = client.create_session(None).await?;
    ensure!(
        session.mode == "general",
        "new session should start as general, got {}",
        session.mode
    );
    ensure!(session.label == "General");
    ensure!(session.voice_cap == "medium");
    ensure!(
        session
            .active_skills
            .iter()
            .any(|skill| skill == "miku-voice")
    );

    let (created_events, created_mode) = client
        .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
        .await?;
    ensure!(created_mode.data["mode"] == json!("general"));
    let mut last_event_id = max_event_id(0, &created_events);

    let mut rounds = Vec::new();
    let context = WorkflowContext::default();
    let personal_step = WorkflowStep::PersonalAssistantGreeting;
    let personal_message = speaker.message(personal_step, &context).await?;
    client.send_message(&session.id, &personal_message).await?;
    let personal_events = client
        .read_until_final(&session.id, Some(last_event_id))
        .await?;
    ensure!(
        personal_events
            .iter()
            .any(|event| event.event_type == "text"),
        "personal assistant turn should stream text"
    );
    let personal_final = final_text(&personal_events)?;
    ensure!(!personal_final.trim().is_empty());
    rounds.push(conversation_round(
        1,
        personal_step,
        &personal_message,
        "general",
        &personal_events,
        &personal_final,
    )?);
    let replay_start = personal_events
        .iter()
        .find_map(|event| event.id)
        .unwrap_or(last_event_id + 1)
        - 1;
    let replayed_personal = client
        .read_until_final(&session.id, Some(replay_start))
        .await?;
    ensure!(
        replayed_personal
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>()
            == personal_events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
        "Last-Event-ID replay should return the missed personal turn events in order"
    );
    last_event_id = max_event_id(last_event_id, &personal_events);

    let context = WorkflowContext {
        personal_final: Some(personal_final.clone()),
    };
    let coding_step = WorkflowStep::CodingModeProbe;
    let coding_message = speaker.message(coding_step, &context).await?;
    // Modes no longer auto-switch from message keywords (they're sticky capability envelopes
    // now); the workflow drives the same explicit override a client's mode picker would use.
    client
        .override_mode(&session.id, "serious_engineer", "coding mode probe")
        .await
        .context("switching to Serious Engineer via mode override")?;
    client.send_message(&session.id, &coding_message).await?;
    let coding_events = client
        .read_until_final(&session.id, Some(last_event_id))
        .await?;
    let mode = coding_events
        .iter()
        .find(|event| event.event_type == "mode" && event.data["mode"] == json!("serious_engineer"))
        .context("coding prompt did not route to Serious Engineer")?;
    let coding_mode = mode.data["mode"].as_str().unwrap_or("serious_engineer");
    ensure!(mode.data["voice_cap"] == json!("off"));
    ensure!(mode.data["activeSkills"] == json!(["serious-engineer-ops"]));
    let coding_final = final_text(&coding_events)?;
    ensure!(!coding_final.contains("喵"));
    rounds.push(conversation_round(
        2,
        coding_step,
        &coding_message,
        coding_mode,
        &coding_events,
        &coding_final,
    )?);
    let artifact_uri = coding_events
        .iter()
        .find(|event| event.event_type == "artifact")
        .and_then(|event| event.data["artifact"]["uri"].as_str())
        .map(str::to_string);
    if options.require_artifact {
        ensure!(
            artifact_uri.is_some(),
            "workflow required an artifact event from the coding backend"
        );
    }
    last_event_id = max_event_id(last_event_id, &coding_events);

    let memory_client = client.clone();
    let session_id = session.id.clone();
    let proposal = tokio::spawn(async move {
        memory_client
            .propose_profile_fact(
                &session_id,
                "prefers",
                "LLM-powered E2E hatch coverage",
                5_000,
            )
            .await
    });
    let (approval_events, approval) = client
        .wait_for_event(&session.id, Some(last_event_id), |event| {
            event.event_type == "approval" && event.data["backend"] == json!("memory")
        })
        .await?;
    last_event_id = max_event_id(last_event_id, &approval_events);
    let approval_id = approval.data["approvalId"]
        .as_str()
        .context("memory approval event did not include approvalId")?;
    client
        .resolve_approval(&session.id, approval_id, "approve")
        .await?;
    let proposal_response = proposal
        .await
        .context("memory proposal task panicked")?
        .context("memory proposal request failed")?;
    ensure!(proposal_response["status"] == json!("approved"));
    let memory_record_uri = proposal_response["record"]["uri"]
        .as_str()
        .context("approved memory proposal did not return a record uri")?
        .to_string();
    let (memory_events, _) = client
        .wait_for_event(&session.id, Some(last_event_id), |event| {
            event.event_type == "write_proposal" && event.data["status"] == json!("approved")
        })
        .await?;
    last_event_id = max_event_id(last_event_id, &memory_events);

    let memory_record = client
        .resolve_resource(&session.id, &memory_record_uri)
        .await?;
    ensure!(
        memory_record["content"]
            .as_str()
            .unwrap_or_default()
            .contains("LLM-powered E2E hatch coverage")
    );
    let memory_preview = client
        .preview_resource(&session.id, &memory_record_uri)
        .await?;
    ensure!(
        memory_preview["preview"]
            .as_str()
            .unwrap_or_default()
            .contains("LLM-powered E2E hatch coverage")
    );
    let schemes = client.list_resources(&session.id, None).await?;
    ensure!(
        schemes
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|entry| entry["uri"] == json!("memory://"))
    );

    if let Some(uri) = &artifact_uri {
        let artifact = client.resolve_resource(&session.id, uri).await?;
        ensure!(
            artifact["content"].as_str().unwrap_or_default().len() > 0,
            "artifact resource {uri} should be readable"
        );
    }

    let mut resources = Vec::new();
    if let Some(uri) = &artifact_uri {
        resources.push(uri.clone());
    }
    let open_loops = vec!["keep the LLM-to-Miku E2E hatch covered".to_string()];
    let decisions = vec!["keep the hatch HTTP-only and approval-bound".to_string()];
    let first_promotion = client
        .promote_session(
            &session.id,
            "LLM-to-Miku E2E hatch is wired through public session APIs.",
            &open_loops,
            &decisions,
            &resources,
        )
        .await?;
    ensure!(first_promotion["projectUri"] == json!("project://tempestmiku"));
    let promoted = first_promotion["promoted"]
        .as_array()
        .context("promotion response did not include promoted items")?;
    ensure!(!promoted.is_empty());
    let second_promotion = client
        .promote_session(
            &session.id,
            "LLM-to-Miku E2E hatch is wired through public session APIs.",
            &open_loops,
            &decisions,
            &resources,
        )
        .await?;
    ensure!(
        first_promotion["promoted"][0]["id"] == second_promotion["promoted"][0]["id"],
        "promotion should be idempotent"
    );

    let project = client.project_overview(&session.id).await?;
    ensure!(project["projectUri"] == json!("project://tempestmiku"));
    ensure!(
        !project["openLoops"]
            .as_array()
            .unwrap_or(&Vec::new())
            .is_empty(),
        "project overview should include open loops"
    );
    ensure!(
        !project["decisions"]
            .as_array()
            .unwrap_or(&Vec::new())
            .is_empty(),
        "project overview should include decisions"
    );
    ensure!(
        !project["nextActions"]
            .as_array()
            .unwrap_or(&Vec::new())
            .is_empty(),
        "project overview should include next actions"
    );
    let project_views = client
        .list_resources(&session.id, Some("project://tempestmiku"))
        .await?;
    ensure!(
        project_views
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|entry| entry["uri"] == json!("project://tempestmiku/resources"))
    );

    // Keep the variable live so future workflow edits do not accidentally stop proving replay
    // after the memory path.
    let _ = last_event_id;

    Ok(WorkflowReport {
        session_id: session.id,
        personal_final,
        coding_final,
        memory_record_uri,
        artifact_uri,
        promoted_count: promoted.len(),
        rounds,
    })
}

pub async fn run_actor_smoke(client: &MikuClient) -> Result<ActorSmokeReport> {
    let session = client.create_session(Some("handoff")).await?;
    ensure!(
        session.mode == "handoff",
        "actor smoke should start in handoff mode, got {}",
        session.mode
    );

    let (created_events, _) = client
        .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
        .await?;
    let mut last_event_id = max_event_id(0, &created_events);

    let send_client = client.clone();
    let send_session_id = session.id.clone();
    let send_message = tokio::spawn(async move {
        send_client
            .send_message(
                &send_session_id,
                "handoff actor smoke: spawn a child, request approval, and return its artifact",
            )
            .await
    });

    let (approval_events, approval) = client
        .wait_for_event(&session.id, Some(last_event_id), |event| {
            event.event_type == "approval" && event.data["backend"] == json!("native-deno")
        })
        .await?;
    let replay_anchor = approval_events
        .iter()
        .find(|event| event.event_type == "actor_spawned")
        .and_then(|event| event.id)
        .unwrap_or_else(|| max_event_id(last_event_id, &approval_events))
        - 1;
    last_event_id = max_event_id(last_event_id, &approval_events);

    let approval_id = approval.data["approvalId"]
        .as_str()
        .context("child approval event did not include approvalId")?
        .to_string();
    let actor_id = approval.data["scope"]["actorId"]
        .as_str()
        .context("child approval event did not include scope.actorId")?
        .to_string();
    client
        .resolve_approval(&session.id, &approval_id, "approve")
        .await?;
    send_message
        .await
        .context("actor smoke send task panicked")?
        .context("actor smoke message request failed")?;

    let final_events = client
        .read_until_final(&session.id, Some(last_event_id))
        .await?;
    ensure!(
        final_events
            .iter()
            .any(|event| event.event_type == "approval_resolved"),
        "child approval resolution should stream before final"
    );
    let completed = final_events
        .iter()
        .find(|event| event.event_type == "actor_completed")
        .context("actor_completed event was not observed")?;
    let completed_actor = completed.data["actor_id"]
        .as_str()
        .or_else(|| completed.data["actorId"].as_str())
        .unwrap_or_default();
    ensure!(
        completed_actor == actor_id,
        "actor_completed should match approval actor id"
    );
    let artifact_uri = completed.data["artifact_uri"]
        .as_str()
        .or_else(|| completed.data["artifactUri"].as_str())
        .context("actor_completed did not include artifactUri")?
        .to_string();

    let artifact = client.resolve_resource(&session.id, &artifact_uri).await?;
    ensure!(
        !artifact["content"].as_str().unwrap_or_default().is_empty(),
        "child artifact {artifact_uri} should open through the session resource gateway"
    );

    let replayed = client
        .read_until_final(&session.id, Some(replay_anchor))
        .await?;
    let replayed_event_types = replayed
        .iter()
        .map(|event| event.event_type.clone())
        .collect::<Vec<_>>();
    ensure!(
        replayed_event_types.iter().any(|kind| kind == "approval")
            && replayed_event_types
                .iter()
                .any(|kind| kind == "approval_resolved")
            && replayed_event_types
                .iter()
                .any(|kind| kind == "actor_completed"),
        "Last-Event-ID replay should include actor approval and completion events"
    );

    Ok(ActorSmokeReport {
        session_id: session.id,
        actor_id,
        approval_id,
        artifact_uri,
        replayed_event_types,
    })
}

fn conversation_round(
    index: usize,
    step: WorkflowStep,
    user_message: &str,
    fallback_mode: &str,
    events: &[E2eEvent],
    assistant_final_text: &str,
) -> Result<ConversationRound> {
    let assistant_streamed_text = events
        .iter()
        .filter(|event| event.event_type == "text")
        .filter_map(|event| event.data["delta"].as_str())
        .collect::<String>();
    let event_id_start = events.iter().filter_map(|event| event.id).min();
    let event_id_end = events.iter().filter_map(|event| event.id).max();
    let mode = events
        .iter()
        .rev()
        .find(|event| event.event_type == "mode")
        .and_then(|event| event.data["mode"].as_str())
        .unwrap_or(fallback_mode)
        .to_string();
    let mut event_types = Vec::new();
    for event in events {
        if !event_types.contains(&event.event_type) {
            event_types.push(event.event_type.clone());
        }
    }
    let mut resource_uris = extract_resource_uris(assistant_final_text);
    for event in events {
        let data = serde_json::to_string(&event.data)
            .context("encoding SSE event data while extracting resources")?;
        for uri in extract_resource_uris(&data) {
            if !resource_uris.contains(&uri) {
                resource_uris.push(uri);
            }
        }
    }
    Ok(ConversationRound {
        index,
        step: step.as_str().to_string(),
        user_message: user_message.to_string(),
        assistant_streamed_text,
        assistant_final_text: assistant_final_text.to_string(),
        mode,
        event_id_start,
        event_id_end,
        event_types,
        resource_uris,
    })
}

fn extract_resource_uris(text: &str) -> Vec<String> {
    const SCHEMES: &[&str] = &[
        "artifact://",
        "workspace://",
        "linked://",
        "project://",
        "memory://",
    ];

    let mut uris = Vec::new();
    for scheme in SCHEMES {
        let mut offset = 0;
        while let Some(relative_start) = text[offset..].find(scheme) {
            let start = offset + relative_start;
            let rest = &text[start..];
            let end = rest
                .char_indices()
                .find_map(|(idx, ch)| resource_uri_delimiter(ch).then_some(idx))
                .unwrap_or(rest.len());
            let uri = rest[..end].trim_end_matches(['.', '。', ',', ';', ':']);
            if !uri.is_empty() && !uris.iter().any(|seen| seen == uri) {
                uris.push(uri.to_string());
            }
            offset = start + end.max(scheme.len());
        }
    }
    uris
}

fn resource_uri_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | ')' | ']' | '}' | '{' | ',')
}

fn final_text(events: &[E2eEvent]) -> Result<String> {
    events
        .iter()
        .rev()
        .find(|event| event.event_type == "final")
        .and_then(|event| event.data["text"].as_str())
        .map(str::to_string)
        .context("final event did not include text")
}

fn max_event_id(current: i64, events: &[E2eEvent]) -> i64 {
    events
        .iter()
        .filter_map(|event| event.id)
        .max()
        .unwrap_or(current)
        .max(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_sse_block() {
        let event = parse_sse_block("id: 7\nevent: final\ndata: {\"text\":\"done\"}\n").unwrap();
        let event = event.unwrap();
        assert_eq!(event.id, Some(7));
        assert_eq!(event.event_type, "final");
        assert_eq!(event.data["text"], json!("done"));
    }

    #[test]
    fn ignores_keepalive_comments() {
        assert!(parse_sse_block(": keep-alive\n").unwrap().is_none());
    }

    #[test]
    fn builds_conversation_round_from_sse_events() {
        let events = vec![
            E2eEvent {
                id: Some(3),
                event_type: "text".to_string(),
                data: json!({ "delta": "hello " }),
            },
            E2eEvent {
                id: Some(4),
                event_type: "text".to_string(),
                data: json!({ "delta": "artifact://0" }),
            },
            E2eEvent {
                id: Some(5),
                event_type: "artifact".to_string(),
                data: json!({ "artifact": { "uri": "artifact://0" } }),
            },
            E2eEvent {
                id: Some(6),
                event_type: "final".to_string(),
                data: json!({ "text": "hello artifact://0" }),
            },
        ];

        let round = conversation_round(
            1,
            WorkflowStep::PersonalAssistantGreeting,
            "status please",
            "general",
            &events,
            "hello artifact://0",
        )
        .unwrap();

        assert_eq!(round.index, 1);
        assert_eq!(round.step, "personal_assistant_greeting");
        assert_eq!(round.user_message, "status please");
        assert_eq!(round.assistant_streamed_text, "hello artifact://0");
        assert_eq!(round.assistant_final_text, "hello artifact://0");
        assert_eq!(round.mode, "general");
        assert_eq!(round.event_id_start, Some(3));
        assert_eq!(round.event_id_end, Some(6));
        assert_eq!(round.event_types, vec!["text", "artifact", "final"]);
        assert_eq!(round.resource_uris, vec!["artifact://0"]);
    }

    #[tokio::test]
    async fn scripted_speaker_uses_message_overrides() {
        let speaker = ScriptedSpeaker::new(
            Some("what tools are available?".to_string()),
            Some("fix the rust test".to_string()),
        );

        assert_eq!(
            speaker
                .message(
                    WorkflowStep::PersonalAssistantGreeting,
                    &WorkflowContext::default()
                )
                .await
                .unwrap(),
            "what tools are available?"
        );
        assert_eq!(
            speaker
                .message(WorkflowStep::CodingModeProbe, &WorkflowContext::default())
                .await
                .unwrap(),
            "fix the rust test"
        );
    }
}

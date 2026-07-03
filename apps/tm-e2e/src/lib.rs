use std::{env, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{Method, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use tm_core::{ChatRequest, LlmClient, Message, ToolChoice};
use tm_llm::OpenAiClient;

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

#[derive(Debug, Clone)]
pub struct MikuClient {
    http: reqwest::Client,
    cfg: E2eConfig,
}

impl MikuClient {
    pub fn new(cfg: E2eConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .context("building E2E HTTP client")?;
        Ok(Self { http, cfg })
    }

    pub fn from_env() -> Result<Self> {
        Self::new(E2eConfig::from_env())
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
        response_json(response, path).await
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        let response = self
            .request(Method::GET, path)
            .send()
            .await
            .with_context(|| format!("GET {path}"))?;
        response_json(response, path).await
    }

    async fn get_json_with_query(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        let response = self
            .request(Method::GET, path)
            .query(query)
            .send()
            .await
            .with_context(|| format!("GET {path}"))?;
        response_json(response, path).await
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

async fn response_json(response: Response, path: &str) -> Result<Value> {
    let response = ensure_success(response).await?;
    response
        .json::<Value>()
        .await
        .with_context(|| format!("decoding JSON response for {path}"))
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

#[derive(Debug, Clone, Copy)]
pub enum WorkflowStep {
    PersonalAssistantGreeting,
    CodingModeProbe,
}

#[derive(Debug, Default, Clone)]
pub struct WorkflowContext {
    pub personal_final: Option<String>,
}

#[async_trait]
pub trait E2eSpeaker: Send + Sync {
    async fn message(&self, step: WorkflowStep, context: &WorkflowContext) -> Result<String>;
}

#[derive(Debug, Default)]
pub struct ScriptedSpeaker;

#[async_trait]
impl E2eSpeaker for ScriptedSpeaker {
    async fn message(&self, step: WorkflowStep, _context: &WorkflowContext) -> Result<String> {
        Ok(match step {
            WorkflowStep::PersonalAssistantGreeting => {
                "hello Miku, give me a short status check for this E2E hatch".to_string()
            }
            WorkflowStep::CodingModeProbe => {
                "please fix this Rust code bug, capture the open loop, and state the decision"
                    .to_string()
            }
        })
    }
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

#[derive(Debug, Clone)]
pub struct WorkflowReport {
    pub session_id: String,
    pub personal_final: String,
    pub coding_final: String,
    pub memory_record_uri: String,
    pub artifact_uri: Option<String>,
    pub promoted_count: usize,
}

pub async fn run_workflow(
    client: &MikuClient,
    speaker: &dyn E2eSpeaker,
    options: WorkflowOptions,
) -> Result<WorkflowReport> {
    let session = client.create_session(None).await?;
    ensure!(
        session.mode == "personal_assistant",
        "new session should start as personal_assistant, got {}",
        session.mode
    );
    ensure!(session.label == "Personal Assistant");
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
    ensure!(created_mode.data["mode"] == json!("personal_assistant"));
    let mut last_event_id = max_event_id(0, &created_events);

    let context = WorkflowContext::default();
    let personal_message = speaker
        .message(WorkflowStep::PersonalAssistantGreeting, &context)
        .await?;
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
    let coding_message = speaker
        .message(WorkflowStep::CodingModeProbe, &context)
        .await?;
    client.send_message(&session.id, &coding_message).await?;
    let coding_events = client
        .read_until_final(&session.id, Some(last_event_id))
        .await?;
    let mode = coding_events
        .iter()
        .find(|event| event.event_type == "mode" && event.data["mode"] == json!("serious_engineer"))
        .context("coding prompt did not route to Serious Engineer")?;
    ensure!(mode.data["voice_cap"] == json!("off"));
    ensure!(mode.data["activeSkills"] == json!([]));
    let coding_final = final_text(&coding_events)?;
    ensure!(!coding_final.contains("喵"));
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
    })
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
}

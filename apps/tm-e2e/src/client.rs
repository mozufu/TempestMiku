use anyhow::{Context, Result, bail, ensure};
use futures::StreamExt;
use reqwest::{Method, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{E2eConfig, E2eEvent, EvidenceRecorder, parse_sse_block};

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

    pub async fn session_messages(&self, session_id: &str) -> Result<Value> {
        self.get_json(&format!("/sessions/{session_id}/messages"))
            .await
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

    pub async fn drive_feed(
        &self,
        session_id: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Value> {
        let limit = limit.to_string();
        let mut query = vec![("limit", limit.as_str())];
        if let Some(project) = project {
            query.push(("project", project));
        }
        self.get_json_with_query(&format!("/sessions/{session_id}/drive/feed"), &query)
            .await
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

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use agent_client_protocol::schema::{
    ProtocolVersion,
    v1::{
        ContentBlock, ContentChunk, Diff, InitializeRequest, NewSessionRequest, PermissionOption,
        PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
        RequestPermissionResponse, SelectedPermissionOutcome, SessionId, SessionNotification,
        SessionUpdate, TextContent, ToolCall, ToolCallContent, ToolCallUpdate,
    },
};
use agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo, LineDirection};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{
    ApprovalBroker, ApprovalOption, ApprovalOutcome, ApprovalPrompt, CodingBackend,
    CodingEventSink, CodingTurn, CodingTurnResult, Result, ServerError, StoreEvent,
};

#[derive(Debug, Clone)]
pub struct OmpAcpConfig {
    pub command: PathBuf,
    pub expected_version: String,
    pub cwd: PathBuf,
    pub approval_mode: String,
    pub profile: Option<String>,
    pub artifact_root: PathBuf,
    pub approval_timeout: Duration,
}

impl OmpAcpConfig {
    pub fn from_env() -> Result<Self> {
        let command = std::env::var_os("TM_OMP_ACP_COMMAND")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("omp"));
        let expected_version = std::env::var("TM_OMP_ACP_EXPECTED_VERSION")
            .unwrap_or_else(|_| "omp/16.2.2".to_string());
        let cwd = std::env::var_os("TM_OMP_ACP_CWD")
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
            .map_err(|err| ServerError::Backend(format!("cannot determine omp cwd: {err}")))?;
        let approval_mode =
            std::env::var("TM_OMP_ACP_APPROVAL_MODE").unwrap_or_else(|_| "always-ask".to_string());
        let profile = std::env::var("TM_OMP_ACP_PROFILE").ok();
        let artifact_root = std::env::var_os("TM_OMP_ACP_ARTIFACT_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(tm_artifacts::default_root);
        let approval_timeout = std::env::var("TM_OMP_ACP_APPROVAL_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(60));
        Ok(Self {
            command,
            expected_version,
            cwd,
            approval_mode,
            profile,
            artifact_root,
            approval_timeout,
        })
    }
}

pub struct OmpAcpBackend {
    config: OmpAcpConfig,
    approval_broker: Arc<ApprovalBroker>,
    sessions: Mutex<BTreeMap<Uuid, mpsc::Sender<OmpTurnRequest>>>,
}

impl OmpAcpBackend {
    pub fn new(config: OmpAcpConfig, approval_broker: Arc<ApprovalBroker>) -> Result<Self> {
        verify_omp_version(&config)?;
        Ok(Self {
            config,
            approval_broker,
            sessions: Mutex::new(BTreeMap::new()),
        })
    }

    fn sender_for_session(&self, session_id: Uuid) -> mpsc::Sender<OmpTurnRequest> {
        let mut sessions = self.sessions.lock();
        sessions
            .entry(session_id)
            .or_insert_with(|| {
                let (sender, receiver) = mpsc::channel(8);
                let worker = OmpWorker::new(
                    session_id,
                    self.config.clone(),
                    Arc::clone(&self.approval_broker),
                    receiver,
                );
                tokio::spawn(async move {
                    if let Err(err) = worker.run().await {
                        tracing::warn!(?err, %session_id, "omp acp worker stopped");
                    }
                });
                sender
            })
            .clone()
    }
}

#[async_trait]
impl CodingBackend for OmpAcpBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<CodingTurnResult> {
        let session_id = turn.session_id;
        let sender = self.sender_for_session(session_id);
        let (response_tx, response_rx) = oneshot::channel();
        let request = OmpTurnRequest {
            turn,
            sink,
            response: response_tx,
        };
        if sender.send(request).await.is_err() {
            self.sessions.lock().remove(&session_id);
            return Err(ServerError::Backend(
                "omp acp worker stopped before accepting turn".to_string(),
            ));
        }
        response_rx.await.map_err(|_| {
            ServerError::Backend("omp acp worker stopped before turn completed".to_string())
        })?
    }
}

fn verify_omp_version(config: &OmpAcpConfig) -> Result<()> {
    let output = std::process::Command::new(&config.command)
        .arg("--version")
        .output()
        .map_err(|err| ServerError::Backend(format!("failed to run omp --version: {err}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let actual = if stdout.is_empty() { stderr } else { stdout };
    if actual == config.expected_version {
        Ok(())
    } else {
        Err(ServerError::Backend(format!(
            "omp version mismatch: expected {}, got {}",
            config.expected_version, actual
        )))
    }
}

struct OmpTurnRequest {
    turn: CodingTurn,
    sink: Arc<dyn CodingEventSink>,
    response: oneshot::Sender<Result<CodingTurnResult>>,
}

struct OmpWorker {
    tempest_session_id: Uuid,
    config: OmpAcpConfig,
    approval_broker: Arc<ApprovalBroker>,
    receiver: mpsc::Receiver<OmpTurnRequest>,
    transcript: Arc<Mutex<Vec<String>>>,
    active: Arc<tokio::sync::Mutex<Option<ActiveOmpTurn>>>,
}

#[derive(Clone)]
struct ActiveOmpTurn {
    turn: CodingTurn,
    sink: Arc<dyn CodingEventSink>,
}

impl OmpWorker {
    fn new(
        tempest_session_id: Uuid,
        config: OmpAcpConfig,
        approval_broker: Arc<ApprovalBroker>,
        receiver: mpsc::Receiver<OmpTurnRequest>,
    ) -> Self {
        Self {
            tempest_session_id,
            config,
            approval_broker,
            receiver,
            transcript: Arc::new(Mutex::new(Vec::new())),
            active: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    async fn run(mut self) -> Result<()> {
        let generated_config = write_generated_config(
            &self.config.cwd,
            self.tempest_session_id,
            &self.config.approval_mode,
        )?;
        let agent = build_agent(
            &self.config,
            &generated_config,
            Arc::clone(&self.transcript),
        )?;
        let active_for_notifications = Arc::clone(&self.active);
        let transcript_for_notifications = Arc::clone(&self.transcript);
        let active_for_permissions = Arc::clone(&self.active);
        let broker_for_permissions = Arc::clone(&self.approval_broker);
        let timeout = self.config.approval_timeout;
        let cwd = self.config.cwd.clone();
        let artifact_root = self.config.artifact_root.clone();
        let expected_version = self.config.expected_version.clone();

        Client
            .builder()
            .on_receive_notification(
                async move |notification: SessionNotification, _cx| {
                    handle_session_notification(
                        notification,
                        Arc::clone(&active_for_notifications),
                        Arc::clone(&transcript_for_notifications),
                    )
                    .await
                    .map_err(to_acp_error)
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |request: RequestPermissionRequest, responder, _connection| {
                    let response = handle_permission_request(
                        request,
                        Arc::clone(&active_for_permissions),
                        Arc::clone(&broker_for_permissions),
                        timeout,
                    )
                    .await
                    .map_err(to_acp_error)?;
                    responder.respond(response)
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let new_session = connection
                    .send_request(NewSessionRequest::new(cwd.clone()))
                    .block_task()
                    .await?;
                let acp_session_id = new_session.session_id;
                while let Some(request) = self.receiver.recv().await {
                    self.transcript.lock().clear();
                    self.active.lock().await.replace(ActiveOmpTurn {
                        turn: request.turn.clone(),
                        sink: Arc::clone(&request.sink),
                    });
                    let result = self
                        .run_prompt_turn(
                            &connection,
                            &acp_session_id,
                            &artifact_root,
                            &expected_version,
                            request.turn,
                            Arc::clone(&request.sink),
                        )
                        .await;
                    self.active.lock().await.take();
                    let _ = request.response.send(result);
                }
                Ok(())
            })
            .await
            .map_err(|err| ServerError::Backend(format!("omp acp worker failed: {err:?}")))
    }

    async fn run_prompt_turn(
        &self,
        connection: &ConnectionTo<Agent>,
        acp_session_id: &SessionId,
        artifact_root: &Path,
        expected_version: &str,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<CodingTurnResult> {
        let prompt = PromptRequest::new(
            acp_session_id.clone(),
            vec![ContentBlock::from(turn.user_prompt.clone())],
        );
        if let Err(err) = connection.send_request(prompt).block_task().await {
            let message = format!("omp acp prompt failed: {err:?}");
            emit_backend_error(Arc::clone(&sink), &message).await;
            return Err(ServerError::Backend(message));
        }

        let transcript_jsonl = self.transcript.lock().join("\n");
        let transcript_jsonl = if transcript_jsonl.is_empty() {
            String::new()
        } else {
            format!("{transcript_jsonl}\n")
        };
        let artifact =
            match tm_artifacts::ArtifactStore::open(artifact_root, turn.session_id.to_string())
                .and_then(|store| {
                    store.put_text(
                        transcript_jsonl,
                        Some("OMP ACP transcript".to_string()),
                        "application/jsonl",
                    )
                }) {
                Ok(artifact) => artifact,
                Err(err) => {
                    let message = format!("failed to write omp acp transcript artifact: {err}");
                    emit_backend_error(Arc::clone(&sink), &message).await;
                    return Err(ServerError::Backend(message));
                }
            };
        sink.emit(
            "artifact",
            json!({
                "backend": "omp-acp",
                "artifact": artifact,
                "provenance": {
                    "source": "omp-acp",
                    "ompVersion": expected_version,
                    "acpSessionId": acp_session_id.to_string(),
                }
            }),
        )
        .await?;
        let final_text = format!(
            "Completed via OMP ACP. Progress, diff, permission, and final transcript evidence were mirrored to {}.",
            artifact.uri
        );
        sink.emit(
            "final",
            serde_json::to_value(StoreEvent::Final {
                text: final_text.clone(),
            })?,
        )
        .await?;
        Ok(CodingTurnResult {
            final_text,
            transcript_artifact: Some(artifact),
        })
    }
}

fn write_generated_config(cwd: &Path, session_id: Uuid, approval_mode: &str) -> Result<PathBuf> {
    let dir = cwd
        .join(".tempestmiku")
        .join("omp-acp")
        .join(session_id.to_string());
    std::fs::create_dir_all(&dir)
        .map_err(|err| ServerError::Backend(format!("failed to create omp config dir: {err}")))?;
    let path = dir.join("config.yml");
    std::fs::write(&path, format!("tools:\n  approvalMode: {approval_mode}\n")).map_err(|err| {
        ServerError::Backend(format!("failed to write omp config overlay: {err}"))
    })?;
    Ok(path)
}

fn build_agent(
    config: &OmpAcpConfig,
    generated_config: &Path,
    transcript: Arc<Mutex<Vec<String>>>,
) -> Result<AcpAgent> {
    let mut args = Vec::new();
    args.push(config.command.to_string_lossy().to_string());
    if let Some(profile) = &config.profile {
        args.push("--profile".to_string());
        args.push(profile.clone());
    }
    args.push("--cwd".to_string());
    args.push(config.cwd.to_string_lossy().to_string());
    args.push("--approval-mode".to_string());
    args.push(config.approval_mode.clone());
    args.push("--config".to_string());
    args.push(generated_config.to_string_lossy().to_string());
    args.push("acp".to_string());

    let agent = AcpAgent::from_args(args)
        .map_err(|err| ServerError::Backend(format!("failed to build omp acp command: {err:?}")))?;
    Ok(agent.with_debug(move |line, direction| {
        let direction = match direction {
            LineDirection::Stdin => "send",
            LineDirection::Stdout => "recv",
            LineDirection::Stderr => "stderr",
        };
        push_transcript(&transcript, direction, line);
    }))
}

async fn handle_session_notification(
    notification: SessionNotification,
    active: Arc<tokio::sync::Mutex<Option<ActiveOmpTurn>>>,
    transcript: Arc<Mutex<Vec<String>>>,
) -> Result<()> {
    let active_turn = active.lock().await.clone();
    let Some(active_turn) = active_turn else {
        return Ok(());
    };
    for event in normalize_session_update(&notification.update) {
        if let Some(text) = &event.transcript_text {
            push_transcript(&transcript, "agent_message_chunk", text);
        }
        active_turn
            .sink
            .emit(&event.event_type, event.payload)
            .await?;
    }
    Ok(())
}

async fn handle_permission_request(
    request: RequestPermissionRequest,
    active: Arc<tokio::sync::Mutex<Option<ActiveOmpTurn>>>,
    broker: Arc<ApprovalBroker>,
    timeout: Duration,
) -> Result<RequestPermissionResponse> {
    let active_turn = active.lock().await.clone();
    let Some(active_turn) = active_turn else {
        return Ok(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ));
    };
    let prompt = approval_prompt_from_request(&request)?;
    let outcome = broker
        .request_permission(
            active_turn.turn.session_id,
            prompt,
            timeout,
            Arc::clone(&active_turn.sink),
        )
        .await?;
    Ok(match outcome {
        ApprovalOutcome::Selected { option_id } => RequestPermissionResponse::new(
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id)),
        ),
        ApprovalOutcome::Cancelled => {
            RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
        }
    })
}

fn approval_prompt_from_request(request: &RequestPermissionRequest) -> Result<ApprovalPrompt> {
    let action = request
        .tool_call
        .fields
        .title
        .clone()
        .unwrap_or_else(|| request.tool_call.tool_call_id.to_string());
    let options = request
        .options
        .iter()
        .map(approval_option_from_acp)
        .collect::<Result<Vec<_>>>()?;
    Ok(ApprovalPrompt {
        action,
        scope: serde_json::to_value(&request.tool_call)?,
        options,
    })
}

fn approval_option_from_acp(option: &PermissionOption) -> Result<ApprovalOption> {
    let kind_value = serde_json::to_value(option.kind).map_err(|err| {
        ServerError::Backend(format!("failed to serialize permission kind: {err}"))
    })?;
    let kind = kind_value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("{:?}", option.kind));
    Ok(ApprovalOption {
        option_id: option.option_id.to_string(),
        name: option.name.clone(),
        kind,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NormalizedOmpEvent {
    pub event_type: String,
    pub payload: Value,
    pub transcript_text: Option<String>,
}

pub(crate) fn normalize_session_update(update: &SessionUpdate) -> Vec<NormalizedOmpEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(ContentChunk {
            content: ContentBlock::Text(TextContent { text, .. }),
            ..
        }) => vec![NormalizedOmpEvent {
            event_type: "text".to_string(),
            payload: json!({ "event": "text", "delta": text }),
            transcript_text: Some(text.clone()),
        }],
        SessionUpdate::ToolCall(tool_call) => {
            let raw = raw_json(update);
            let mut events = vec![NormalizedOmpEvent {
                event_type: "tool_call".to_string(),
                payload: json!({ "backend": "omp-acp", "toolCall": raw["toolCall"].clone() }),
                transcript_text: None,
            }];
            events.extend(diff_events_for_tool_call(tool_call));
            events
        }
        SessionUpdate::ToolCallUpdate(tool_call_update) => {
            let raw = raw_json(update);
            let mut events = vec![NormalizedOmpEvent {
                event_type: "tool_call_update".to_string(),
                payload: json!({ "backend": "omp-acp", "toolCall": raw["toolCallUpdate"].clone() }),
                transcript_text: None,
            }];
            events.extend(diff_events_for_tool_call_update(tool_call_update));
            events
        }
        SessionUpdate::AgentThoughtChunk(_)
        | SessionUpdate::Plan(_)
        | SessionUpdate::AvailableCommandsUpdate(_)
        | SessionUpdate::CurrentModeUpdate(_)
        | SessionUpdate::ConfigOptionUpdate(_)
        | SessionUpdate::SessionInfoUpdate(_)
        | SessionUpdate::UsageUpdate(_) => vec![NormalizedOmpEvent {
            event_type: "progress".to_string(),
            payload: json!({ "backend": "omp-acp", "update": raw_json(update) }),
            transcript_text: None,
        }],
        _ => vec![NormalizedOmpEvent {
            event_type: "progress".to_string(),
            payload: json!({ "backend": "omp-acp", "update": raw_json(update) }),
            transcript_text: None,
        }],
    }
}

fn diff_events_for_tool_call(tool_call: &ToolCall) -> Vec<NormalizedOmpEvent> {
    tool_call
        .content
        .iter()
        .filter_map(|content| diff_event(&tool_call.tool_call_id.to_string(), content))
        .collect()
}

fn diff_events_for_tool_call_update(tool_call_update: &ToolCallUpdate) -> Vec<NormalizedOmpEvent> {
    tool_call_update
        .fields
        .content
        .as_ref()
        .into_iter()
        .flatten()
        .filter_map(|content| diff_event(&tool_call_update.tool_call_id.to_string(), content))
        .collect()
}

fn diff_event(tool_call_id: &str, content: &ToolCallContent) -> Option<NormalizedOmpEvent> {
    let ToolCallContent::Diff(Diff {
        path,
        old_text,
        new_text,
        ..
    }) = content
    else {
        return None;
    };
    Some(NormalizedOmpEvent {
        event_type: "diff".to_string(),
        payload: json!({
            "backend": "omp-acp",
            "toolCallId": tool_call_id,
            "path": path,
            "oldText": old_text,
            "newText": new_text,
        }),
        transcript_text: None,
    })
}

fn raw_json<T: serde::Serialize + std::fmt::Debug>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|_| json!({ "debug": format!("{value:?}") }))
}

fn push_transcript(transcript: &Mutex<Vec<String>>, direction: &str, line: &str) {
    let entry = json!({ "direction": direction, "line": line });
    if let Ok(line) = serde_json::to_string(&entry) {
        transcript.lock().push(line);
    }
}

async fn emit_backend_error(sink: Arc<dyn CodingEventSink>, message: &str) {
    let _ = sink
        .emit("error", json!({ "backend": "omp-acp", "message": message }))
        .await;
}

fn to_acp_error(err: ServerError) -> agent_client_protocol::Error {
    agent_client_protocol::Error::internal_error().data(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{PermissionOptionKind, ToolCallUpdateFields};
    use async_trait::async_trait;

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<(String, Value)>>,
    }

    #[async_trait]
    impl CodingEventSink for RecordingSink {
        async fn emit(&self, event_type: &str, payload_json: Value) -> Result<crate::SessionEvent> {
            self.events
                .lock()
                .push((event_type.to_string(), payload_json.clone()));
            Ok(crate::SessionEvent::new(
                Uuid::nil(),
                self.events.lock().len() as i64,
                event_type,
                payload_json,
                chrono::Utc::now(),
            ))
        }
    }

    #[tokio::test]
    async fn agent_message_chunk_maps_to_text_delta_without_final_text() {
        let update = SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new("hello"),
        )));
        let events = normalize_session_update(&update);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "text");
        assert_eq!(
            events[0].payload,
            json!({ "event": "text", "delta": "hello" })
        );
        assert_eq!(events[0].transcript_text, Some("hello".to_string()));
        assert!(!events.iter().any(|event| event.event_type == "final"));
    }

    #[tokio::test]
    async fn tool_update_with_diff_produces_diff_event() {
        let update = ToolCallUpdate::new(
            "call-1",
            ToolCallUpdateFields::new().content(vec![ToolCallContent::Diff(
                Diff::new("src/lib.rs", "new text").old_text(Some("old text".to_string())),
            )]),
        );
        let events = normalize_session_update(&SessionUpdate::ToolCallUpdate(update));
        assert_eq!(events[0].event_type, "tool_call_update");
        let diff = events
            .iter()
            .find(|event| event.event_type == "diff")
            .unwrap();
        assert_eq!(diff.payload["toolCallId"], json!("call-1"));
        assert_eq!(diff.payload["path"], json!("src/lib.rs"));
        assert_eq!(diff.payload["oldText"], json!("old text"));
        assert_eq!(diff.payload["newText"], json!("new text"));
    }

    #[tokio::test]
    async fn permission_request_round_trips_selected_and_cancelled() {
        let broker = Arc::new(ApprovalBroker::default());
        let sink = Arc::new(RecordingSink::default());
        let session_id = Uuid::new_v4();
        let active = Arc::new(tokio::sync::Mutex::new(Some(ActiveOmpTurn {
            turn: CodingTurn {
                session_id,
                user_prompt: "prompt".to_string(),
                system_prompt: "system prompt".to_string(),
                mode: tm_modes::ModeId::from("handoff"),
                scope: "project:tempestmiku".to_string(),
                capabilities: vec![],
            },
            sink: sink.clone(),
        })));
        let request = RequestPermissionRequest::new(
            "acp-session",
            ToolCallUpdate::new("tool-1", ToolCallUpdateFields::new().title("Edit file")),
            vec![
                PermissionOption::new("allow", "Allow once", PermissionOptionKind::AllowOnce),
                PermissionOption::new("reject", "Reject once", PermissionOptionKind::RejectOnce),
            ],
        );
        let pending = tokio::spawn({
            let active = Arc::clone(&active);
            let broker = Arc::clone(&broker);
            async move {
                handle_permission_request(request, active, broker, Duration::from_secs(5))
                    .await
                    .unwrap()
            }
        });
        let approval_id = wait_for_approval_id(&sink).await;
        broker
            .resolve(
                session_id,
                approval_id,
                crate::ResolveApprovalRequest {
                    decision: crate::ApprovalResolveDecision::Approve,
                    option_id: None,
                },
            )
            .unwrap();
        let selected = pending.await.unwrap();
        assert!(matches!(
            selected.outcome,
            RequestPermissionOutcome::Selected(_)
        ));

        let cancelled = handle_permission_request(
            RequestPermissionRequest::new(
                "acp-session",
                ToolCallUpdate::new("tool-2", ToolCallUpdateFields::new().title("Edit file")),
                vec![],
            ),
            active,
            Arc::new(ApprovalBroker::default()),
            Duration::from_millis(1),
        )
        .await
        .unwrap();
        assert_eq!(cancelled.outcome, RequestPermissionOutcome::Cancelled);
    }

    #[test]
    fn generated_config_uses_requested_approval_mode() {
        let cwd = std::env::temp_dir().join(format!("tm-omp-acp-config-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).unwrap();
        let path = write_generated_config(&cwd, Uuid::new_v4(), "yolo").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("approvalMode: yolo"));
        std::fs::remove_dir_all(cwd).unwrap();
    }
    async fn wait_for_approval_id(sink: &RecordingSink) -> Uuid {
        for _ in 0..100 {
            if let Some((_, payload)) = sink
                .events
                .lock()
                .iter()
                .find(|(event_type, _)| event_type == "approval")
                .cloned()
            {
                return serde_json::from_value(payload["approvalId"].clone()).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("approval event was not emitted")
    }
}

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use agent_client_protocol::schema::{
    ProtocolVersion,
    v1::{
        ContentBlock, InitializeRequest, NewSessionRequest, PermissionOption, PromptRequest,
        RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
        SelectedPermissionOutcome, SessionId, SessionNotification,
    },
};
use agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo, LineDirection};
use parking_lot::Mutex;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use super::{config::OmpAcpConfig, normalize::normalize_session_update};
use crate::{
    ApprovalBroker, ApprovalOption, ApprovalOutcome, ApprovalPrompt, CodingEventSink, CodingTurn,
    CodingTurnResult, Result, ServerError, StoreEvent,
};

pub(crate) struct OmpTurnRequest {
    pub(crate) turn: CodingTurn,
    pub(crate) sink: Arc<dyn CodingEventSink>,
    pub(crate) response: oneshot::Sender<Result<CodingTurnResult>>,
}

pub(crate) struct OmpWorker {
    tempest_session_id: Uuid,
    config: OmpAcpConfig,
    approval_broker: Arc<ApprovalBroker>,
    receiver: mpsc::Receiver<OmpTurnRequest>,
    transcript: Arc<Mutex<Vec<String>>>,
    active: Arc<tokio::sync::Mutex<Option<ActiveOmpTurn>>>,
}

#[derive(Clone)]
pub(crate) struct ActiveOmpTurn {
    pub(crate) turn: CodingTurn,
    pub(crate) sink: Arc<dyn CodingEventSink>,
}

impl OmpWorker {
    pub(crate) fn new(
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

    pub(crate) async fn run(mut self) -> Result<()> {
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

pub(crate) fn write_generated_config(
    cwd: &Path,
    session_id: Uuid,
    approval_mode: &str,
) -> Result<PathBuf> {
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

pub(crate) async fn handle_permission_request(
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

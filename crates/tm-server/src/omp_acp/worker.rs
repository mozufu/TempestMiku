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

const TRANSCRIPT_SEGMENT_BYTES: usize = 1024 * 1024;
const TRANSCRIPT_MAX_BYTES: usize = 16 * 1024 * 1024;
const TRANSCRIPT_MAX_ENTRY_BYTES: usize = 128 * 1024;
const TRANSCRIPT_MAX_DIRECTION_BYTES: usize = 64;

#[derive(Debug, Default)]
struct TranscriptBuffer {
    lines: Vec<String>,
    bytes: usize,
    truncated: bool,
    truncation_reason: Option<String>,
    redactions: usize,
}

#[derive(Debug)]
struct TranscriptArtifacts {
    manifest: tm_artifacts::ArtifactRef,
    segments: Vec<tm_artifacts::ArtifactRef>,
    captured_bytes: usize,
    truncated: bool,
    truncation_reason: Option<String>,
    redactions: usize,
}

impl TranscriptBuffer {
    fn clear(&mut self) {
        *self = Self::default();
    }

    fn push(&mut self, direction: &str, line: &str) {
        if self.truncated {
            return;
        }
        let mut entry_truncated = line.len() > TRANSCRIPT_MAX_ENTRY_BYTES;
        let line = if entry_truncated {
            &line[..floor_char_boundary(line, TRANSCRIPT_MAX_ENTRY_BYTES)]
        } else {
            line
        };
        let report = tm_memory::redact_dream_text(line);
        self.redactions = self.redactions.saturating_add(
            report
                .redactions
                .iter()
                .map(|redaction| redaction.count)
                .sum::<usize>(),
        );
        let direction = if direction.len() <= TRANSCRIPT_MAX_DIRECTION_BYTES {
            direction
        } else {
            "unknown"
        };
        let mut retained_line = report.text;
        let serialized = loop {
            let entry = json!({
                "direction": direction,
                "line": retained_line,
                "truncated": entry_truncated,
            });
            let Ok(serialized) = serde_json::to_string(&entry) else {
                return;
            };
            if serialized.len().saturating_add(1) <= TRANSCRIPT_SEGMENT_BYTES {
                break serialized;
            }
            entry_truncated = true;
            let next_len = floor_char_boundary(&retained_line, retained_line.len() / 2);
            retained_line.truncate(next_len);
        };
        let required = serialized.len().saturating_add(1);
        if required > TRANSCRIPT_MAX_BYTES.saturating_sub(self.bytes) {
            self.truncated = true;
            self.truncation_reason = Some("total_limit".to_string());
            return;
        }
        self.bytes += required;
        self.lines.push(serialized);
        self.truncated = entry_truncated;
        if entry_truncated {
            self.truncation_reason = Some("entry_limit".to_string());
        }
    }
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

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
    transcript: Arc<Mutex<TranscriptBuffer>>,
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
            transcript: Arc::new(Mutex::new(TranscriptBuffer::default())),
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
            .map_err(|err| omp_backend_error(format!("omp acp worker failed: {err:?}")))
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
            let message = redact_omp_text(&format!("omp acp prompt failed: {err:?}"));
            emit_backend_error(Arc::clone(&sink), &message).await;
            return Err(ServerError::Backend(message));
        }

        let transcript = {
            let mut transcript = self.transcript.lock();
            std::mem::take(&mut *transcript)
        };
        let transcript_artifacts =
            match write_transcript_artifacts(artifact_root, turn.session_id, transcript) {
                Ok(artifacts) => artifacts,
                Err(err) => {
                    let message = redact_omp_text(&format!(
                        "failed to write omp acp transcript artifact: {err}"
                    ));
                    emit_backend_error(Arc::clone(&sink), &message).await;
                    return Err(ServerError::Backend(message));
                }
            };
        sink.emit(
            "artifact",
            transcript_artifact_event(
                &transcript_artifacts,
                expected_version,
                &acp_session_id.to_string(),
            ),
        )
        .await?;
        let artifact = transcript_artifacts.manifest;
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
        .map_err(|err| omp_backend_error(format!("failed to create omp config dir: {err}")))?;
    let path = dir.join("config.yml");
    std::fs::write(&path, format!("tools:\n  approvalMode: {approval_mode}\n"))
        .map_err(|err| omp_backend_error(format!("failed to write omp config overlay: {err}")))?;
    Ok(path)
}

fn build_agent(
    config: &OmpAcpConfig,
    generated_config: &Path,
    transcript: Arc<Mutex<TranscriptBuffer>>,
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
        .map_err(|err| omp_backend_error(format!("failed to build omp acp command: {err:?}")))?;
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
    transcript: Arc<Mutex<TranscriptBuffer>>,
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

fn push_transcript(transcript: &Mutex<TranscriptBuffer>, direction: &str, line: &str) {
    transcript.lock().push(direction, line);
}

fn write_transcript_artifacts(
    artifact_root: &Path,
    session_id: Uuid,
    transcript: TranscriptBuffer,
) -> std::result::Result<TranscriptArtifacts, tm_artifacts::ArtifactError> {
    let store = tm_artifacts::ArtifactStore::open(artifact_root, session_id.to_string())?;
    let TranscriptBuffer {
        lines,
        bytes,
        truncated,
        truncation_reason,
        redactions,
    } = transcript;
    let mut segment = String::with_capacity(TRANSCRIPT_SEGMENT_BYTES);
    let mut segments = Vec::new();
    for line in lines {
        let line_bytes =
            line.len()
                .checked_add(1)
                .ok_or(tm_artifacts::ArtifactError::QuotaExceeded {
                    resource: "OMP ACP transcript segment",
                    attempted: usize::MAX,
                    limit: TRANSCRIPT_SEGMENT_BYTES,
                })?;
        if line_bytes > TRANSCRIPT_SEGMENT_BYTES {
            return Err(tm_artifacts::ArtifactError::QuotaExceeded {
                resource: "OMP ACP transcript segment",
                attempted: line_bytes,
                limit: TRANSCRIPT_SEGMENT_BYTES,
            });
        }
        if segment.len().saturating_add(line_bytes) > TRANSCRIPT_SEGMENT_BYTES {
            segments.push(store.put_text(
                std::mem::take(&mut segment),
                Some(format!("OMP ACP transcript segment {}", segments.len() + 1)),
                "application/jsonl",
            )?);
        }
        segment.push_str(&line);
        segment.push('\n');
    }
    if !segment.is_empty() {
        segments.push(store.put_text(
            segment,
            Some(format!("OMP ACP transcript segment {}", segments.len() + 1)),
            "application/jsonl",
        )?);
    }
    let manifest = serde_json::to_string_pretty(&json!({
        "kind": "omp_acp_transcript_manifest",
        "segments": segments.iter().map(|artifact| json!({
            "uri": artifact.uri,
            "sizeBytes": artifact.size_bytes,
        })).collect::<Vec<_>>(),
        "capturedBytes": bytes,
        "maxBytes": TRANSCRIPT_MAX_BYTES,
        "truncated": truncated,
        "truncationReason": truncation_reason.as_deref(),
        "redactions": redactions,
    }))
    .expect("transcript manifest serialization is infallible");
    let manifest = store.put_text(
        manifest,
        Some("OMP ACP transcript manifest".to_string()),
        "application/json",
    )?;
    Ok(TranscriptArtifacts {
        manifest,
        segments,
        captured_bytes: bytes,
        truncated,
        truncation_reason,
        redactions,
    })
}

fn transcript_artifact_event(
    artifacts: &TranscriptArtifacts,
    omp_version: &str,
    acp_session_id: &str,
) -> serde_json::Value {
    json!({
        "backend": "omp-acp",
        "artifact": &artifacts.manifest,
        "capturedBytes": artifacts.captured_bytes,
        "maxBytes": TRANSCRIPT_MAX_BYTES,
        "segments": artifacts.segments.iter().map(|segment| json!({
            "uri": &segment.uri,
            "sizeBytes": segment.size_bytes,
        })).collect::<Vec<_>>(),
        "truncated": artifacts.truncated,
        "redactions": artifacts.redactions,
        "provenance": {
            "source": "omp-acp",
            "ompVersion": omp_version,
            "acpSessionId": acp_session_id,
            "segmentBytes": TRANSCRIPT_SEGMENT_BYTES,
            "truncation": {
                "applied": artifacts.truncated,
                "reason": artifacts.truncation_reason.as_deref(),
            },
        }
    })
}

async fn emit_backend_error(sink: Arc<dyn CodingEventSink>, message: &str) {
    let message = redact_omp_text(message);
    let _ = sink
        .emit("error", json!({ "backend": "omp-acp", "message": message }))
        .await;
}

fn to_acp_error(err: ServerError) -> agent_client_protocol::Error {
    agent_client_protocol::Error::internal_error().data(redact_omp_text(&err.to_string()))
}

fn omp_backend_error(message: impl AsRef<str>) -> ServerError {
    ServerError::Backend(redact_omp_text(message.as_ref()))
}

fn redact_omp_text(message: &str) -> String {
    tm_memory::redact_dream_text(message).text
}

#[cfg(test)]
mod transcript_tests {
    use super::*;

    #[test]
    fn transcript_redacts_secrets_and_stops_at_a_bounded_entry() {
        let mut transcript = TranscriptBuffer::default();
        transcript.push("recv", "token sk-testsecret123456 should not persist");
        assert_eq!(transcript.redactions, 1);
        assert!(!transcript.lines[0].contains("sk-testsecret123456"));

        transcript.push("stderr", &"x".repeat(TRANSCRIPT_MAX_ENTRY_BYTES + 32));
        assert!(transcript.truncated);
        assert_eq!(transcript.truncation_reason.as_deref(), Some("entry_limit"));
        assert!(transcript.bytes <= TRANSCRIPT_MAX_BYTES);
        assert!(
            transcript
                .lines
                .last()
                .unwrap()
                .contains("\"truncated\":true")
        );

        let retained = transcript.lines.len();
        transcript.push("recv", "ignored after truncation");
        assert_eq!(transcript.lines.len(), retained);
    }

    #[test]
    fn transcript_writer_returns_a_segment_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = Uuid::new_v4();
        let mut transcript = TranscriptBuffer::default();
        transcript.push("recv", "hello");
        transcript.push("stderr", "world");

        let artifacts = write_transcript_artifacts(dir.path(), session_id, transcript).unwrap();
        let store = tm_artifacts::ArtifactStore::open(dir.path(), session_id.to_string()).unwrap();
        let content = store.read(&artifacts.manifest.uri, None).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content.content).unwrap();
        assert_eq!(json["kind"], "omp_acp_transcript_manifest");
        assert_eq!(json["segments"].as_array().unwrap().len(), 1);
        assert_eq!(json["truncated"], false);

        let event = transcript_artifact_event(&artifacts, "1.2.3", "acp-session");
        assert_eq!(event["capturedBytes"], json!(artifacts.captured_bytes));
        assert_eq!(event["maxBytes"], json!(TRANSCRIPT_MAX_BYTES));
        assert_eq!(event["segments"].as_array().unwrap().len(), 1);
        assert_eq!(event["truncated"], false);
        assert_eq!(event["redactions"], 0);
        assert_eq!(event["provenance"]["source"], "omp-acp");
        assert_eq!(event["provenance"]["truncation"]["applied"], false);
        assert_eq!(
            event["provenance"]["truncation"]["reason"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn transcript_segments_stay_bounded_after_worst_case_json_escaping() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = Uuid::new_v4();
        let escaped = "\u{0000}".repeat(TRANSCRIPT_MAX_ENTRY_BYTES);
        let mut transcript = TranscriptBuffer::default();
        transcript.push("recv", &escaped);
        transcript.push("stderr", &escaped);
        assert!(
            transcript
                .lines
                .iter()
                .all(|line| line.len() < TRANSCRIPT_SEGMENT_BYTES)
        );

        let artifacts = write_transcript_artifacts(dir.path(), session_id, transcript).unwrap();
        assert_eq!(artifacts.segments.len(), 2);
        assert!(
            artifacts
                .segments
                .iter()
                .all(|segment| segment.size_bytes <= TRANSCRIPT_SEGMENT_BYTES)
        );
    }
}

use std::{sync::Arc, time::Duration};

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, Diff, PermissionOption, PermissionOptionKind,
    RequestPermissionOutcome, RequestPermissionRequest, SessionUpdate, TextContent,
    ToolCallContent, ToolCallUpdate, ToolCallUpdateFields,
};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use uuid::Uuid;

use super::normalize::normalize_session_update;
use super::worker::{ActiveOmpTurn, handle_permission_request, write_generated_config};
use super::{OmpAcpBackend, OmpAcpConfig};
use crate::{
    ApprovalBroker, CodingEventSink, CodingTurn, InMemoryStore, NewSession, ResolveApprovalRequest,
    Result, Store, StoreCodingEventSink,
};

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
            durable_turn_id: None,
            user_prompt: "prompt".to_string(),
            system_prompt: "system prompt".to_string(),
            mode: tm_modes::ModeId::from("handoff"),
            scope: "project:tempestmiku".to_string(),
            capabilities: vec![],
            prior_messages: Vec::new(),
        },
        sink: sink.clone(),
    })));
    let request = RequestPermissionRequest::new(
        "acp-session",
        ToolCallUpdate::new("tool-1", ToolCallUpdateFields::new().title("Edit file")),
        vec![
            PermissionOption::new("allow_once", "Allow once", PermissionOptionKind::AllowOnce),
            PermissionOption::new(
                "reject_once",
                "Reject once",
                PermissionOptionKind::RejectOnce,
            ),
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
                option_id: Some("allow_once".to_string()),
            },
        )
        .unwrap();
    let selected = pending.await.unwrap();
    assert_eq!(
        serde_json::to_value(&selected).unwrap(),
        json!({
            "outcome": {
                "outcome": "selected",
                "optionId": "allow_once",
            },
        })
    );

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
fn generated_config_keeps_the_acp_gate_as_the_only_interactive_approval_layer() {
    let cwd = std::env::temp_dir().join(format!("tm-omp-acp-config-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&cwd).unwrap();
    let path = write_generated_config(&cwd, Uuid::new_v4(), "always-ask").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        content,
        "tools:\n  approvalMode: always-ask\n  approval:\n    bash: allow\n    edit: allow\n"
    );
    std::fs::remove_dir_all(cwd).unwrap();
}

#[tokio::test]
async fn durable_permission_returns_the_selected_acp_wire_option() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: tm_modes::ModeId::from("serious_engineer"),
            persona_status: tm_modes::AssetStatus::Loaded {
                path: "/test/assets".into(),
            },
        })
        .await
        .unwrap();
    let requester = Arc::new(ApprovalBroker::default());
    let resolver = Arc::new(ApprovalBroker::default());
    requester.bind_store(Arc::clone(&store));
    resolver.bind_store(Arc::clone(&store));
    let (sender, _) = tokio::sync::broadcast::channel(16);
    let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session.id,
        Arc::clone(&store),
        sender.clone(),
    ));
    let active = Arc::new(tokio::sync::Mutex::new(Some(ActiveOmpTurn {
        turn: CodingTurn {
            session_id: session.id,
            durable_turn_id: None,
            user_prompt: "fix it".to_string(),
            system_prompt: "system".to_string(),
            mode: tm_modes::ModeId::from("serious_engineer"),
            scope: "project:tempestmiku".to_string(),
            capabilities: vec![],
            prior_messages: vec![],
        },
        sink,
    })));
    let pending = tokio::spawn({
        let requester = Arc::clone(&requester);
        let active = Arc::clone(&active);
        async move {
            handle_permission_request(
                RequestPermissionRequest::new(
                    "acp-session",
                    ToolCallUpdate::new("tool-1", ToolCallUpdateFields::new().title("Edit file")),
                    vec![
                        PermissionOption::new(
                            "allow_once",
                            "Allow once",
                            PermissionOptionKind::AllowOnce,
                        ),
                        PermissionOption::new(
                            "reject_once",
                            "Reject once",
                            PermissionOptionKind::RejectOnce,
                        ),
                    ],
                ),
                active,
                requester,
                Duration::from_secs(5),
            )
            .await
            .unwrap()
        }
    });
    let approval_id = loop {
        if let Some(event) = store
            .events_after(session.id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|event| event.event_type == "approval")
        {
            break event.payload_json["approvalId"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    };
    let resolution_sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session.id,
        Arc::clone(&store),
        sender,
    ));
    resolver
        .resolve_persisted(
            session.id,
            approval_id,
            ResolveApprovalRequest {
                decision: crate::ApprovalResolveDecision::Approve,
                option_id: Some("allow_once".to_string()),
            },
            resolution_sink,
        )
        .await
        .unwrap();

    assert_eq!(
        serde_json::to_value(pending.await.unwrap()).unwrap(),
        json!({
            "outcome": {
                "outcome": "selected",
                "optionId": "allow_once",
            },
        })
    );
    let approval = store
        .approval_request(session.id, approval_id)
        .await
        .unwrap();
    assert_eq!(approval.status, "approved");
    assert_eq!(approval.selected_option_id.as_deref(), Some("allow_once"));
}

#[test]
fn config_rejects_unbounded_approval_shapes_before_spawning() {
    let cwd = tempfile::tempdir().unwrap();
    let config = OmpAcpConfig {
        command: "must-not-run".into(),
        expected_version: "omp/test".to_string(),
        cwd: cwd.path().to_path_buf(),
        approval_mode: "injected\nconfig: true".to_string(),
        profile: None,
        artifact_root: cwd.path().join("artifacts"),
        approval_timeout: Duration::from_secs(1),
    };
    let error = match OmpAcpBackend::new(config, Arc::new(ApprovalBroker::default())) {
        Ok(_) => panic!("invalid approval mode was accepted"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("unsupported OMP ACP approval mode")
    );
}

#[cfg(unix)]
#[test]
fn backend_requires_the_exact_pinned_omp_version() {
    use std::os::unix::fs::PermissionsExt;

    let cwd = tempfile::tempdir().unwrap();
    let command = cwd.path().join("omp-fixture");
    std::fs::write(&command, "#!/bin/sh\nprintf '%s\\n' 'omp/test-2'\n").unwrap();
    let mut permissions = std::fs::metadata(&command).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&command, permissions).unwrap();
    let config = OmpAcpConfig {
        command,
        expected_version: "omp/test-1".to_string(),
        cwd: cwd.path().to_path_buf(),
        approval_mode: "always-ask".to_string(),
        profile: None,
        artifact_root: cwd.path().join("artifacts"),
        approval_timeout: Duration::from_secs(1),
    };
    let error = match OmpAcpBackend::new(config, Arc::new(ApprovalBroker::default())) {
        Ok(_) => panic!("mismatched OMP version was accepted"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("omp version mismatch: expected omp/test-1, got omp/test-2")
    );
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

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
use crate::{ApprovalBroker, CodingEventSink, CodingTurn, Result};

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

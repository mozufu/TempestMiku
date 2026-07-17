use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Result, ServerError, SessionEvent, Store};

use super::*;

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl CodingEventSink for RecordingSink {
    async fn emit(&self, event_type: &str, payload_json: Value) -> Result<SessionEvent> {
        self.events
            .lock()
            .push((event_type.to_string(), payload_json.clone()));
        Ok(SessionEvent::new(
            Uuid::nil(),
            self.events.lock().len() as i64,
            event_type,
            payload_json,
            chrono::Utc::now(),
        ))
    }
}

fn prompt(options: Vec<(&str, &str)>) -> ApprovalPrompt {
    ApprovalPrompt {
        action: "edit file".to_string(),
        scope: json!({ "path": "src/lib.rs" }),
        options: options
            .into_iter()
            .map(|(option_id, kind)| ApprovalOption {
                option_id: option_id.to_string(),
                name: option_id.to_string(),
                kind: kind.to_string(),
            })
            .collect(),
    }
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

#[tokio::test]
async fn approve_without_option_selects_allow_once_before_allow_always() {
    let broker = Arc::new(ApprovalBroker::default());
    let sink = Arc::new(RecordingSink::default());
    let session_id = Uuid::new_v4();
    let request = {
        let broker = Arc::clone(&broker);
        let sink = Arc::clone(&sink);
        tokio::spawn(async move {
            broker
                .request_permission(
                    session_id,
                    prompt(vec![
                        ("always", "allow_always"),
                        ("once", "allow_once"),
                        ("reject", "reject_once"),
                    ]),
                    Duration::from_secs(5),
                    sink,
                )
                .await
                .unwrap()
        })
    };
    let approval_id = wait_for_approval_id(&sink).await;
    broker
        .resolve(
            session_id,
            approval_id,
            ResolveApprovalRequest {
                decision: ApprovalResolveDecision::Approve,
                option_id: None,
            },
        )
        .unwrap();
    assert_eq!(
        request.await.unwrap(),
        ApprovalOutcome::Selected {
            option_id: "once".to_string()
        }
    );
}

#[tokio::test]
async fn deny_and_timeout_select_reject_once_or_cancel() {
    let broker = Arc::new(ApprovalBroker::default());
    let sink = Arc::new(RecordingSink::default());
    let session_id = Uuid::new_v4();
    let denied = {
        let broker = Arc::clone(&broker);
        let sink = Arc::clone(&sink);
        tokio::spawn(async move {
            broker
                .request_permission(
                    session_id,
                    prompt(vec![("reject", "reject_once"), ("always", "allow_always")]),
                    Duration::from_secs(5),
                    sink,
                )
                .await
                .unwrap()
        })
    };
    let approval_id = wait_for_approval_id(&sink).await;
    broker
        .resolve(
            session_id,
            approval_id,
            ResolveApprovalRequest {
                decision: ApprovalResolveDecision::Deny,
                option_id: None,
            },
        )
        .unwrap();
    assert_eq!(
        denied.await.unwrap(),
        ApprovalOutcome::Selected {
            option_id: "reject".to_string()
        }
    );

    let timeout_sink = Arc::new(RecordingSink::default());
    let timed_out = broker
        .request_permission(
            session_id,
            prompt(vec![("reject", "reject_once"), ("always", "allow_always")]),
            Duration::from_millis(1),
            timeout_sink,
        )
        .await
        .unwrap();
    assert_eq!(
        timed_out,
        ApprovalOutcome::Selected {
            option_id: "reject".to_string()
        }
    );

    let cancel_sink = Arc::new(RecordingSink::default());
    let cancelled = broker
        .request_permission(
            session_id,
            prompt(vec![("always", "allow_always")]),
            Duration::from_millis(1),
            cancel_sink,
        )
        .await
        .unwrap();
    assert_eq!(cancelled, ApprovalOutcome::Cancelled);
}

#[tokio::test]
async fn detailed_permission_preserves_timeout_status() {
    let broker = Arc::new(ApprovalBroker::default());
    let sink = Arc::new(RecordingSink::default());
    let session_id = Uuid::new_v4();
    let timed_out = broker
        .request_permission_detailed_for_backend(
            session_id,
            "native-tm",
            prompt(vec![("reject", "reject_once"), ("allow", "allow_once")]),
            Duration::from_millis(1),
            sink,
        )
        .await
        .unwrap();
    assert_eq!(timed_out.status, ApprovalStatus::TimedOut);
    assert_eq!(
        timed_out.outcome,
        ApprovalOutcome::Selected {
            option_id: "reject".to_string()
        }
    );
}

#[tokio::test]
async fn unknown_and_repeated_resolution_return_invalid_request() {
    let broker = Arc::new(ApprovalBroker::default());
    let session_id = Uuid::new_v4();
    let unknown = broker.resolve(
        session_id,
        Uuid::new_v4(),
        ResolveApprovalRequest {
            decision: ApprovalResolveDecision::Approve,
            option_id: None,
        },
    );
    assert!(matches!(unknown, Err(ServerError::InvalidRequest(_))));

    let sink = Arc::new(RecordingSink::default());
    let pending = {
        let broker = Arc::clone(&broker);
        let sink = Arc::clone(&sink);
        tokio::spawn(async move {
            broker
                .request_permission(
                    session_id,
                    prompt(vec![("once", "allow_once"), ("reject", "reject_once")]),
                    Duration::from_secs(5),
                    sink,
                )
                .await
                .unwrap()
        })
    };
    let approval_id = wait_for_approval_id(&sink).await;
    broker
        .resolve(
            session_id,
            approval_id,
            ResolveApprovalRequest {
                decision: ApprovalResolveDecision::Approve,
                option_id: None,
            },
        )
        .unwrap();
    assert!(matches!(
        broker.resolve(
            session_id,
            approval_id,
            ResolveApprovalRequest {
                decision: ApprovalResolveDecision::Approve,
                option_id: None,
            },
        ),
        Err(ServerError::InvalidRequest(_))
    ));
    assert_eq!(
        pending.await.unwrap(),
        ApprovalOutcome::Selected {
            option_id: "once".to_string()
        }
    );
}

#[tokio::test]
async fn durable_permission_observes_resolution_from_another_broker_instance() {
    let store = Arc::new(crate::InMemoryStore::default());
    let session = store
        .create_session(crate::NewSession {
            mode: tm_modes::ModeId::from("general"),
            persona_status: tm_modes::AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let requester = Arc::new(ApprovalBroker::default());
    let resolver = Arc::new(ApprovalBroker::default());
    requester.bind_store(Arc::clone(&store));
    resolver.bind_store(Arc::clone(&store));
    let (sender, _) = tokio::sync::broadcast::channel(32);
    let request_sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session.id,
        Arc::clone(&store),
        sender.clone(),
    ));
    let pending = {
        let requester = Arc::clone(&requester);
        tokio::spawn(async move {
            requester
                .request_permission(
                    session.id,
                    prompt(vec![("allow", "allow_once"), ("reject", "reject_once")]),
                    Duration::from_secs(5),
                    request_sink,
                )
                .await
                .unwrap()
        })
    };
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
                decision: ApprovalResolveDecision::Approve,
                option_id: Some("allow".to_string()),
            },
            resolution_sink,
        )
        .await
        .unwrap();
    assert_eq!(
        pending.await.unwrap(),
        ApprovalOutcome::Selected {
            option_id: "allow".to_string()
        }
    );
    let approval = store
        .approval_request(session.id, approval_id)
        .await
        .unwrap();
    assert_eq!(approval.status, "approved");
    assert!(approval.request_event_seq.is_some());
    assert!(approval.resolution_event_seq.is_some());
    let lease = store
        .claim_next_approval_effect(Uuid::new_v4(), Utc::now(), chrono::Duration::seconds(30))
        .await
        .unwrap()
        .expect("a restarted worker can claim the durable continuation");
    assert_eq!(lease.effect.approval_id, approval_id);
    store
        .complete_approval_effect(&lease, Utc::now())
        .await
        .unwrap();
}

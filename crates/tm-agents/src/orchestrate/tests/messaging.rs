use super::super::*;
use super::support::*;

#[tokio::test]
async fn agents_msg_denied_without_grant() {
    let f = AgentsMsgFn::new(make_roster());
    let ctx = ctx_without_agents();
    let err = f
        .call(json!({"handle": {"id": "W"}, "text": "hello"}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_MSG));
}

#[tokio::test]
async fn agents_msg_invalid_args() {
    let f = AgentsMsgFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_MSG);

    // Non-string text
    let err = f
        .call(json!({"handle": {"id": "Worker"}, "text": 42}), &ctx)
        .await
        .unwrap_err();
    assert!(
        matches!(err, HostError::InvalidArgs(_)),
        "expected InvalidArgs, got {err:?}"
    );

    // Missing handle.id
    let err = f
        .call(json!({"handle": {}, "text": "hello"}), &ctx)
        .await
        .unwrap_err();
    assert!(
        matches!(err, HostError::InvalidArgs(_)),
        "expected InvalidArgs, got {err:?}"
    );

    // handle.id is not valid CamelCase (lowercase start)
    let err = f
        .call(json!({"handle": {"id": "worker"}, "text": "hello"}), &ctx)
        .await
        .unwrap_err();
    assert!(
        matches!(err, HostError::InvalidArgs(_)),
        "expected InvalidArgs, got {err:?}"
    );
}

#[tokio::test]
async fn agents_msg_rejects_json_control_payload() {
    let f = AgentsMsgFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_MSG);

    let err = f
        .call(
            json!({"handle": {"id": "Worker"}, "text": "{\"type\":\"done\"}"}),
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_msg_await_rejects_descendant_wait_edge() {
    let roster = make_roster();
    track_actor(&roster, "Parent", None, ActorStatus::Running).await;
    track_actor(&roster, "Child", Some("Parent"), ActorStatus::Running).await;
    let f = AgentsMsgFn::new(Arc::clone(&roster));
    let ctx = ctx_with_actor(caps::AGENTS_MSG, "Parent");

    let err = f
        .call(
            json!({"handle": {"id": "Child"}, "text": "status?", "opts": {"await": true}}),
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
    assert!(roster.messages().await.is_empty());
}

#[tokio::test]
async fn agents_msg_unknown_handle_returns_null() {
    let f = AgentsMsgFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_MSG);
    // "Nope" is CamelCase but not in roster
    let result = f
        .call(json!({"handle": {"id": "Nope"}, "text": "hi"}), &ctx)
        .await
        .unwrap();
    assert_eq!(result, Value::Null);
}

#[tokio::test]
async fn agents_msg_fire_and_forget_returns_delivered_receipt() {
    use crate::actor::{ActorBudget, ActorId, ActorRecord, ActorStatus};
    let roster = make_roster();
    // Pre-populate a tracked actor
    roster
        .track(ActorRecord {
            id: ActorId::new("Worker").unwrap(),
            parent: None,
            status: ActorStatus::Running,
            mode: Some("worker".to_string()),
            budget: ActorBudget::default(),
            spawned_at: chrono::Utc::now(),
            completed_at: None,
            cancelled: false,
            failure_reason: None,
            last_summary: None,
            artifact_uri: None,
            history_uri: None,
        })
        .await;

    let f = AgentsMsgFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_MSG);

    let result = f
        .call(json!({"handle": {"id": "Worker"}, "text": "status?"}), &ctx)
        .await
        .unwrap();
    assert_eq!(result, json!({"status": "delivered"}));

    let messages = roster.messages().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].from.as_str(), "Root");
    assert_eq!(messages[0].to.as_str(), "Worker");
    assert_eq!(messages[0].text, "status?");
}

#[tokio::test]
async fn agents_msg_fire_and_forget_reports_full_inbox() {
    let roster = make_roster();
    track_running(&roster, "Worker").await;

    for index in 0..64 {
        let receipt = roster
            .send_message(ActorMessage {
                from: ActorId::new("Root").unwrap(),
                to: ActorId::new("Worker").unwrap(),
                text: format!("queued {index}"),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;
        assert_eq!(receipt, Receipt::Delivered);
    }

    let f = AgentsMsgFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_MSG);
    let result = f
        .call(
            json!({"handle": {"id": "Worker"}, "text": "overflow"}),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(
        result,
        json!({"status": "failed", "reason": "backpressured"})
    );
    let drained = roster
        .drain_inbox(&ActorId::new("Worker").unwrap(), None)
        .await;
    assert!(!drained.iter().any(|message| message.text == "overflow"));
}

#[tokio::test]
async fn agents_msg_await_returns_reply() {
    use crate::actor::{ActorBudget, ActorDigest, ActorId, ActorRecord, ActorSpec, ActorStatus};
    use crate::executor::{ActorError, ActorExecutor};

    struct EchoExecutor;
    #[async_trait::async_trait]
    impl ActorExecutor for EchoExecutor {
        async fn run_to_digest(
            &self,
            spec: ActorSpec,
        ) -> std::result::Result<ActorDigest, ActorError> {
            Ok(ActorDigest {
                actor_id: spec.id,
                summary: format!("echo: {}", spec.task),
                artifact_uri: None,
                history_uri: None,
                history_content: None,
            })
        }
    }

    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(EchoExecutor));

    // Track a completed actor with a stored summary
    let target_id = ActorId::new("ResearchWorker").unwrap();
    roster
        .track(ActorRecord {
            id: target_id.clone(),
            parent: None,
            status: ActorStatus::Terminated,
            mode: Some("researcher".to_string()),
            budget: ActorBudget::default(),
            spawned_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            cancelled: false,
            failure_reason: None,
            last_summary: Some("I found the answer.".to_string()),
            artifact_uri: None,
            history_uri: None,
        })
        .await;

    let f = AgentsMsgFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_MSG);

    let result = f
            .call(
                json!({"handle": {"id": "ResearchWorker"}, "text": "elaborate please", "opts": {"await": true}}),
                &ctx,
            )
            .await
            .unwrap();

    // EchoExecutor echoes the seeded task; result should contain both prior context and the new text
    let reply = result.as_str().expect("reply should be a string");
    assert!(
        reply.contains("I found the answer."),
        "reply should contain prior context"
    );
    assert!(
        reply.contains("elaborate please"),
        "reply should contain new message"
    );
}

#[tokio::test]
async fn agents_msg_await_without_executor_returns_not_implemented() {
    use crate::actor::{ActorBudget, ActorId, ActorRecord, ActorStatus};
    let roster = make_roster(); // no executor set
    roster
        .track(ActorRecord {
            id: ActorId::new("Worker").unwrap(),
            parent: None,
            status: ActorStatus::Terminated,
            mode: None,
            budget: ActorBudget::default(),
            spawned_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            cancelled: false,
            failure_reason: None,
            last_summary: None,
            artifact_uri: None,
            history_uri: None,
        })
        .await;

    let f = AgentsMsgFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_MSG);

    let err = f
        .call(
            json!({"handle": {"id": "Worker"}, "text": "hi", "opts": {"await": true}}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, HostError::NotImplemented(_)),
        "expected NotImplemented, got {err:?}"
    );
}

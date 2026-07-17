use super::super::*;
use super::support::*;

#[tokio::test]
async fn agents_cancel_denied_without_grant() {
    let f = AgentsCancelFn::new(make_roster());
    let ctx = ctx_without_agents();
    let err = f.call(json!({"target": "Worker"}), &ctx).await.unwrap_err();
    assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_CANCEL));
}

#[tokio::test]
async fn agents_cancel_root_child_marks_terminal_and_emits_once() {
    let roster = make_roster();
    track_running(&roster, "Worker").await;
    let (events, hook) = lifecycle_hook();
    roster.set_lifecycle_hook(hook);

    let f = AgentsCancelFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_CANCEL);
    let result = f
        .call(json!({"target": {"id": "Worker"}}), &ctx)
        .await
        .unwrap();

    assert_eq!(result["actorId"], Value::String("Worker".into()));
    assert_eq!(result["status"], Value::String("cancelled".into()));
    let rec = roster.get(&ActorId::new("Worker").unwrap()).await.unwrap();
    assert_eq!(rec.status, ActorStatus::Terminated);
    assert!(rec.cancelled);
    assert!(matches!(rec.failure_reason, Some(FailureReason::Cancelled)));
    assert!(roster.is_cancelled(&ActorId::new("Worker").unwrap()).await);

    let receipt = roster
        .send_message(ActorMessage {
            from: ActorId::new("Root").unwrap(),
            to: ActorId::new("Worker").unwrap(),
            text: "status?".to_string(),
            reply_to: None,
            sent_at: Utc::now(),
        })
        .await;
    assert_eq!(receipt, Receipt::Unreachable);

    let second = f.call(json!({"target": "Worker"}), &ctx).await.unwrap();
    assert_eq!(second["status"], Value::String("already_cancelled".into()));

    let evs = events.lock().unwrap();
    let cancelled: Vec<_> = evs
        .iter()
        .filter(|event| matches!(event, ActorLifecycleEvent::Cancelled { .. }))
        .collect();
    assert_eq!(cancelled.len(), 1, "expected one Cancelled event");
}

#[tokio::test]
async fn agents_cancel_rejects_non_child_target() {
    let roster = make_roster();
    track_actor(&roster, "Parent", None, ActorStatus::Running).await;
    track_actor(&roster, "Child", Some("Parent"), ActorStatus::Running).await;
    let f = AgentsCancelFn::new(roster);
    let ctx = ctx_with_actor(caps::AGENTS_CANCEL, "Sibling");

    let err = f.call(json!({"target": "Child"}), &ctx).await.unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_send_delivers_to_live_inbox() {
    let roster = make_roster();
    track_running(&roster, "Worker").await;
    let f = AgentsSendFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_SEND);

    let result = f
        .call(json!({"to": "Worker", "text": "status?"}), &ctx)
        .await
        .unwrap();

    assert_eq!(result["status"], Value::String("delivered".into()));
    assert_eq!(
        roster.unread_count(&ActorId::new("Worker").unwrap()).await,
        1
    );
    let messages = roster.messages().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].from.as_str(), "Root");
    assert_eq!(messages[0].to.as_str(), "Worker");
}

#[tokio::test]
async fn agents_send_to_unknown_actor_returns_failed_receipt() {
    let f = AgentsSendFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_SEND);

    let result = f
        .call(json!({"to": "Missing", "text": "hello"}), &ctx)
        .await
        .unwrap();

    assert_eq!(result, json!({"status": "failed", "reason": "unreachable"}));
}

#[tokio::test]
async fn agents_send_await_reports_full_inbox_without_waiting() {
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

    let f = AgentsSendFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_SEND);
    let result = f
        .call(
            json!({"to": "Worker", "text": "overflow", "opts": {"await": true, "timeoutMs": 1000}}),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(
        result,
        json!({"status": "failed", "reason": "backpressured"})
    );
}

#[tokio::test]
async fn agents_send_rejects_json_control_payload() {
    let f = AgentsSendFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_SEND);

    let err = f
        .call(json!({"to": "Worker", "text": "{\"type\":\"done\"}"}), &ctx)
        .await
        .unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_send_await_rejects_descendant_wait_edge() {
    let roster = make_roster();
    track_actor(&roster, "Parent", None, ActorStatus::Running).await;
    track_actor(&roster, "Child", Some("Parent"), ActorStatus::Running).await;
    let f = AgentsSendFn::new(Arc::clone(&roster));
    let ctx = ctx_with_actor(caps::AGENTS_SEND, "Parent");

    let err = f
        .call(
            json!({"to": "Child", "text": "status?", "opts": {"await": true, "timeoutMs": 1}}),
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
    assert!(roster.messages().await.is_empty());
}

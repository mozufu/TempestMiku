use super::super::*;
use super::support::*;

#[tokio::test]
async fn agents_broadcast_denied_without_grant() {
    let f = AgentsBroadcastFn::new(make_roster());
    let ctx = ctx_without_agents();
    let err = f.call(json!({"text": "hello"}), &ctx).await.unwrap_err();
    assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_BROADCAST));
}

#[tokio::test]
async fn agents_broadcast_invalid_args_returns_invalid_args_error() {
    let f = AgentsBroadcastFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_BROADCAST);

    let err = f.call(json!({}), &ctx).await.unwrap_err();
    assert!(matches!(err, HostError::InvalidArgs(_)));

    let err = f.call(json!({"text": 42}), &ctx).await.unwrap_err();
    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_broadcast_rejects_json_control_payload() {
    let f = AgentsBroadcastFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_BROADCAST);

    let err = f
        .call(json!({"text": "[{\"type\":\"done\"}]"}), &ctx)
        .await
        .unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_broadcast_root_targets_live_root_children_only() {
    let roster = make_roster();
    track_actor(&roster, "Beta", None, ActorStatus::Running).await;
    track_actor(&roster, "Nested", Some("Alpha"), ActorStatus::Running).await;
    track_actor(&roster, "Done", None, ActorStatus::Terminated).await;
    track_actor(&roster, "Idle", None, ActorStatus::Idle).await;
    track_actor(&roster, "Parked", None, ActorStatus::Parked).await;
    track_actor(&roster, "Alpha", None, ActorStatus::Running).await;
    let (events, hook) = lifecycle_hook();
    roster.set_lifecycle_hook(hook);

    let f = AgentsBroadcastFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_BROADCAST);
    let result = f.call(json!({"text": "status?"}), &ctx).await.unwrap();
    let receipts = result.as_array().unwrap();

    assert_eq!(
        receipts,
        &vec![
            json!({"actorId": "Alpha", "status": "delivered"}),
            json!({"actorId": "Beta", "status": "delivered"}),
            json!({"actorId": "Idle", "status": "delivered"}),
            json!({"actorId": "Parked", "status": "delivered"}),
        ]
    );

    let alpha_messages = roster
        .drain_inbox(&ActorId::new("Alpha").unwrap(), None)
        .await;
    let beta_messages = roster
        .drain_inbox(&ActorId::new("Beta").unwrap(), None)
        .await;
    assert_eq!(alpha_messages[0].text, "status?");
    assert_eq!(beta_messages[0].text, "status?");
    assert_eq!(
        roster
            .drain_inbox(&ActorId::new("Idle").unwrap(), None)
            .await[0]
            .text,
        "status?"
    );
    assert_eq!(
        roster
            .drain_inbox(&ActorId::new("Parked").unwrap(), None)
            .await[0]
            .text,
        "status?"
    );
    assert!(
        roster
            .drain_inbox(&ActorId::new("Nested").unwrap(), None)
            .await
            .is_empty()
    );
    assert!(
        roster
            .drain_inbox(&ActorId::new("Done").unwrap(), None)
            .await
            .is_empty()
    );

    let evs = events.lock().unwrap();
    let messages: Vec<_> = evs
        .iter()
        .filter_map(|event| match event {
            ActorLifecycleEvent::MessageSent { to, .. } => Some(to.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(messages, vec!["Alpha", "Beta", "Idle", "Parked"]);
}

#[tokio::test]
async fn agents_broadcast_child_targets_direct_live_children_only() {
    let roster = make_roster();
    track_actor(&roster, "Parent", None, ActorStatus::Running).await;
    track_actor(&roster, "ChildB", Some("Parent"), ActorStatus::Running).await;
    track_actor(&roster, "Sibling", None, ActorStatus::Running).await;
    track_actor(&roster, "Grandchild", Some("ChildA"), ActorStatus::Running).await;
    track_actor(&roster, "ChildA", Some("Parent"), ActorStatus::Running).await;

    let f = AgentsBroadcastFn::new(Arc::clone(&roster));
    let ctx = ctx_with_actor(caps::AGENTS_BROADCAST, "Parent");
    let result = f
        .call(json!({"text": "children only"}), &ctx)
        .await
        .unwrap();
    let receipts = result.as_array().unwrap();

    assert_eq!(
        receipts,
        &vec![
            json!({"actorId": "ChildA", "status": "delivered"}),
            json!({"actorId": "ChildB", "status": "delivered"}),
        ]
    );
    assert_eq!(
        roster
            .drain_inbox(&ActorId::new("ChildA").unwrap(), None)
            .await[0]
            .text,
        "children only"
    );
    assert_eq!(
        roster
            .drain_inbox(&ActorId::new("ChildB").unwrap(), None)
            .await[0]
            .text,
        "children only"
    );
    assert!(
        roster
            .drain_inbox(&ActorId::new("Sibling").unwrap(), None)
            .await
            .is_empty()
    );
    assert!(
        roster
            .drain_inbox(&ActorId::new("Grandchild").unwrap(), None)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn agents_broadcast_reports_backpressure_failed_receipt_in_order() {
    let roster = make_roster();
    track_actor(&roster, "Beta", None, ActorStatus::Running).await;
    track_actor(&roster, "Alpha", None, ActorStatus::Running).await;

    for index in 0..64 {
        let receipt = roster
            .send_message(ActorMessage {
                from: ActorId::new("Root").unwrap(),
                to: ActorId::new("Beta").unwrap(),
                text: format!("queued {index}"),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;
        assert_eq!(receipt, Receipt::Delivered);
    }

    let f = AgentsBroadcastFn::new(roster);
    let ctx = ctx_with(caps::AGENTS_BROADCAST);
    let result = f.call(json!({"text": "fan out"}), &ctx).await.unwrap();
    let receipts = result.as_array().unwrap();

    assert_eq!(
        receipts,
        &vec![
            json!({"actorId": "Alpha", "status": "delivered"}),
            json!({"actorId": "Beta", "status": "failed", "reason": "backpressured"}),
        ]
    );
}

#[tokio::test]
async fn agents_broadcast_empty_targets_returns_empty_array() {
    let f = AgentsBroadcastFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_BROADCAST);
    let result = f.call(json!({"text": "anyone?"}), &ctx).await.unwrap();
    assert_eq!(result, json!([]));
}

use super::super::*;
use super::support::*;

#[tokio::test]
async fn agents_inbox_drains_current_actor_messages() {
    let roster = make_roster();
    track_running(&roster, "Worker").await;
    roster
        .send_message(ActorMessage {
            from: ActorId::new("Root").unwrap(),
            to: ActorId::new("Worker").unwrap(),
            text: "status?".to_string(),
            reply_to: None,
            sent_at: Utc::now(),
        })
        .await;

    let f = AgentsInboxFn::new(Arc::clone(&roster));
    let ctx = ctx_with_actor(caps::AGENTS_INBOX, "Worker");
    let result = f.call(json!({}), &ctx).await.unwrap();
    let messages = result.as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["from"], Value::String("Root".into()));
    assert_eq!(messages[0]["text"], Value::String("status?".into()));
    assert!(
        roster
            .drain_inbox(&ActorId::new("Worker").unwrap(), None)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn agents_wait_returns_prequeued_root_message() {
    let roster = make_roster();
    track_running(&roster, "Worker").await;
    roster
        .send_message(ActorMessage {
            from: ActorId::new("Worker").unwrap(),
            to: ActorId::new("Root").unwrap(),
            text: "done".to_string(),
            reply_to: None,
            sent_at: Utc::now(),
        })
        .await;

    let f = AgentsWaitFn::new(roster);
    let ctx = ctx_with(caps::AGENTS_WAIT);
    let result = f
        .call(json!({"from": "Worker", "timeoutMs": 50}), &ctx)
        .await
        .unwrap();

    assert_eq!(result["from"], Value::String("Worker".into()));
    assert_eq!(result["to"], Value::String("Root".into()));
    assert_eq!(result["text"], Value::String("done".into()));
}

#[tokio::test]
async fn agents_wait_rejects_descendant_filter() {
    let roster = make_roster();
    track_actor(&roster, "Parent", None, ActorStatus::Running).await;
    track_actor(&roster, "Child", Some("Parent"), ActorStatus::Running).await;
    track_actor(&roster, "Grandchild", Some("Child"), ActorStatus::Running).await;
    let f = AgentsWaitFn::new(roster);
    let ctx = ctx_with_actor(caps::AGENTS_WAIT, "Parent");

    let err = f
        .call(json!({"from": "Grandchild", "timeoutMs": 1}), &ctx)
        .await
        .unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_wait_allows_actor_to_wait_on_ancestor() {
    let roster = make_roster();
    track_actor(&roster, "Parent", None, ActorStatus::Running).await;
    track_actor(&roster, "Child", Some("Parent"), ActorStatus::Running).await;
    roster
        .send_message(ActorMessage {
            from: ActorId::new("Parent").unwrap(),
            to: ActorId::new("Child").unwrap(),
            text: "proceed".to_string(),
            reply_to: None,
            sent_at: Utc::now(),
        })
        .await;

    let f = AgentsWaitFn::new(roster);
    let ctx = ctx_with_actor(caps::AGENTS_WAIT, "Child");
    let result = f
        .call(json!({"from": "Parent", "timeoutMs": 50}), &ctx)
        .await
        .unwrap();

    assert_eq!(result["text"], Value::String("proceed".into()));
}

#[tokio::test]
async fn agents_wait_times_out_with_null() {
    let f = AgentsWaitFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_WAIT);
    let result = f.call(json!({"timeoutMs": 1}), &ctx).await.unwrap();
    assert_eq!(result, Value::Null);
}

#[tokio::test]
async fn agents_list_includes_unread_and_last_activity() {
    let roster = make_roster();
    track_running(&roster, "Worker").await;
    roster
        .send_message(ActorMessage {
            from: ActorId::new("Root").unwrap(),
            to: ActorId::new("Worker").unwrap(),
            text: "status?".to_string(),
            reply_to: None,
            sent_at: Utc::now(),
        })
        .await;

    let f = AgentsListFn::new(roster);
    let ctx = ctx_with(caps::AGENTS_LIST);
    let result = f.call(json!({}), &ctx).await.unwrap();
    let entries = result.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], Value::String("Worker".into()));
    assert_eq!(entries[0]["status"], Value::String("running".into()));
    assert_eq!(entries[0]["unread"], Value::Number(1.into()));
    assert!(entries[0]["lastActivity"].as_str().is_some());
}

use super::*;
use crate::actor::{ActorBudget, ActorId};
use crate::supervise::RestartStrategy;

fn test_record(id: &str) -> ActorRecord {
    ActorRecord {
        id: ActorId::new(id).unwrap(),
        parent: None,
        status: ActorStatus::Running,
        mode: None,
        budget: ActorBudget::default(),
        spawned_at: Utc::now(),
        completed_at: None,
        cancelled: false,
        failure_reason: None,
        last_summary: None,
        artifact_uri: None,
        history_uri: None,
    }
}

fn test_record_with_parent(id: &str, parent: &str) -> ActorRecord {
    ActorRecord {
        parent: Some(ActorId::new(parent).unwrap()),
        ..test_record(id)
    }
}

#[tokio::test]
async fn track_and_get() {
    let registry = MailboxRegistry::new();
    let id = ActorId::new("Worker").unwrap();
    registry.track(test_record("Worker")).await;
    let got = registry.get(&id).await.unwrap();
    assert_eq!(got.id, id);
    assert_eq!(got.status, ActorStatus::Running);
}

#[tokio::test]
async fn list_returns_all() {
    let registry = MailboxRegistry::new();
    registry.track(test_record("Alpha")).await;
    registry.track(test_record("Beta")).await;
    let list = registry.list().await;
    assert_eq!(list.len(), 2);
}

#[tokio::test]
async fn lineage_detects_descendant_wait_edges() {
    let registry = MailboxRegistry::new();
    let parent = ActorId::new("Parent").unwrap();
    let child = ActorId::new("Child").unwrap();
    let grandchild = ActorId::new("Grandchild").unwrap();
    let sibling = ActorId::new("Sibling").unwrap();
    let root = ActorId::new("Root").unwrap();

    registry.track(test_record("Parent")).await;
    registry
        .track(test_record_with_parent("Child", "Parent"))
        .await;
    registry
        .track(test_record_with_parent("Grandchild", "Child"))
        .await;
    registry.track(test_record("Sibling")).await;

    assert!(registry.is_descendant_of(&child, &parent).await);
    assert!(registry.is_descendant_of(&grandchild, &parent).await);
    assert!(!registry.is_descendant_of(&sibling, &parent).await);
    assert!(registry.would_wait_on_descendant(&parent, &child).await);
    assert!(
        registry
            .would_wait_on_descendant(&parent, &grandchild)
            .await
    );
    assert!(registry.would_wait_on_descendant(&parent, &parent).await);
    assert!(!registry.would_wait_on_descendant(&child, &parent).await);
    assert!(!registry.would_wait_on_descendant(&root, &child).await);
}

#[tokio::test]
async fn mark_complete_updates_status() {
    let registry = MailboxRegistry::new();
    let id = ActorId::new("Worker").unwrap();
    registry.track(test_record("Worker")).await;
    registry.mark_complete(&id).await;
    let rec = registry.get(&id).await.unwrap();
    assert_eq!(rec.status, ActorStatus::Terminated);
    assert!(rec.completed_at.is_some());
    assert!(!rec.cancelled);
}

#[tokio::test]
async fn mark_failed_records_reason() {
    let registry = MailboxRegistry::new();
    let id = ActorId::new("Worker").unwrap();
    registry.track(test_record("Worker")).await;
    registry.mark_failed(&id, FailureReason::Timeout).await;
    let rec = registry.get(&id).await.unwrap();
    assert_eq!(rec.status, ActorStatus::Terminated);
    assert!(!rec.cancelled);
    assert!(matches!(rec.failure_reason, Some(FailureReason::Timeout)));
}

#[tokio::test]
async fn mark_failed_cancelled_sets_flag() {
    let registry = MailboxRegistry::new();
    let id = ActorId::new("Worker").unwrap();
    registry.track(test_record("Worker")).await;
    registry.mark_failed(&id, FailureReason::Cancelled).await;
    let rec = registry.get(&id).await.unwrap();
    assert!(rec.cancelled);
}

#[tokio::test]
async fn mark_failed_with_supervision_returns_default_restart_decision() {
    let registry = MailboxRegistry::new();
    let id = ActorId::new("Worker").unwrap();
    registry.track(test_record("Worker")).await;

    let (supervisor_id, decision) = registry
        .mark_failed_with_supervision(&id, FailureReason::Timeout)
        .await
        .expect("tracked actor failure has a supervisor decision");

    assert_eq!(supervisor_id, ActorId::new("Root").unwrap());
    assert_eq!(decision.restart_attempt, Some(1));
    assert_eq!(
        decision.actions,
        vec![SupervisionAction::Restart {
            actor_id: id.clone()
        }]
    );
    let rec = registry.get(&id).await.unwrap();
    assert_eq!(rec.status, ActorStatus::Terminated);
    assert!(matches!(rec.failure_reason, Some(FailureReason::Timeout)));
}

#[tokio::test]
async fn one_for_all_supervision_cancels_siblings_and_emits_decision() {
    let registry = MailboxRegistry::new();
    let root = ActorId::new("Root").unwrap();
    registry
        .set_supervision_policy(
            root.clone(),
            SupervisionPolicy {
                strategy: RestartStrategy::OneForAll,
                max_restarts: 1,
            },
        )
        .await;
    registry.track(test_record("Alpha")).await;
    registry.track(test_record("Beta")).await;
    registry.track(test_record("Gamma")).await;
    registry
        .track(test_record_with_parent("AlphaChild", "Alpha"))
        .await;
    registry
        .track(test_record_with_parent("GammaChild", "Gamma"))
        .await;

    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let events_for_hook = Arc::clone(&events);
    registry.set_lifecycle_hook(move |_session_id, event| {
        events_for_hook.lock().unwrap().push(event);
    });

    let beta = ActorId::new("Beta").unwrap();
    let (supervisor_id, decision) = registry
        .mark_failed_with_supervision(&beta, FailureReason::Timeout)
        .await
        .expect("tracked actor failure has a supervisor decision");
    registry
        .emit_and_apply_supervision_decision("", supervisor_id, decision.clone())
        .await;

    assert_eq!(
        decision.actions,
        vec![
            SupervisionAction::Cancel {
                actor_id: ActorId::new("Alpha").unwrap()
            },
            SupervisionAction::Cancel {
                actor_id: ActorId::new("Gamma").unwrap()
            },
            SupervisionAction::Restart {
                actor_id: ActorId::new("Alpha").unwrap()
            },
            SupervisionAction::Restart {
                actor_id: ActorId::new("Beta").unwrap()
            },
            SupervisionAction::Restart {
                actor_id: ActorId::new("Gamma").unwrap()
            },
        ]
    );
    assert!(
        registry
            .get(&ActorId::new("Alpha").unwrap())
            .await
            .unwrap()
            .cancelled
    );
    assert!(
        registry
            .get(&ActorId::new("AlphaChild").unwrap())
            .await
            .unwrap()
            .cancelled
    );
    assert!(
        registry
            .get(&ActorId::new("Gamma").unwrap())
            .await
            .unwrap()
            .cancelled
    );
    assert!(
        registry
            .get(&ActorId::new("GammaChild").unwrap())
            .await
            .unwrap()
            .cancelled
    );
    assert!(registry.is_cancelled(&ActorId::new("Alpha").unwrap()).await);
    assert!(
        registry
            .is_cancelled(&ActorId::new("AlphaChild").unwrap())
            .await
    );
    assert!(registry.is_cancelled(&ActorId::new("Gamma").unwrap()).await);
    assert!(
        registry
            .is_cancelled(&ActorId::new("GammaChild").unwrap())
            .await
    );

    let events = events.lock().unwrap();
    assert!(matches!(
        events.first(),
        Some(ActorLifecycleEvent::Supervision { .. })
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ActorLifecycleEvent::Cancelled { .. }))
            .count(),
        4
    );
}

#[test]
fn next_actor_id_camel_case_from_role() {
    let registry = MailboxRegistry::new();
    let id = registry.next_actor_id("researcher");
    let s = id.as_str();
    assert!(s.starts_with('R'), "should start with 'R', got {s}");
    assert!(
        s.chars().all(|c| c.is_alphanumeric()),
        "must be alphanumeric: {s}"
    );
    assert!(s.len() <= 32, "must be ≤32 chars: {s}");
}

#[test]
fn next_actor_id_incrementing_suffix() {
    let registry = MailboxRegistry::new();
    let a = registry.next_actor_id("worker");
    let b = registry.next_actor_id("worker");
    assert_ne!(a, b);
}

#[test]
fn next_actor_id_empty_role_fallback() {
    let registry = MailboxRegistry::new();
    let id = registry.next_actor_id("");
    assert!(id.as_str().starts_with('A'));
}

#[test]
fn executor_not_set_returns_none() {
    let registry = MailboxRegistry::new();
    assert!(registry.executor().is_none());
}

#[tokio::test]
async fn mark_complete_with_digest_stores_uris() {
    let registry = MailboxRegistry::new();
    let id = ActorId::new("Worker").unwrap();
    registry.track(test_record("Worker")).await;
    registry
        .mark_complete_with_digest(
            &id,
            "summary".to_string(),
            Some("artifact://1".to_string()),
            Some("history://Worker".to_string()),
        )
        .await;
    let rec = registry.get(&id).await.unwrap();
    assert_eq!(rec.status, ActorStatus::Terminated);
    assert_eq!(rec.last_summary.as_deref(), Some("summary"));
    assert_eq!(rec.artifact_uri.as_deref(), Some("artifact://1"));
    assert_eq!(rec.history_uri.as_deref(), Some("history://Worker"));
}

#[tokio::test]
async fn store_and_get_transcript() {
    let registry = MailboxRegistry::new();
    let id = ActorId::new("Worker").unwrap();
    assert!(registry.get_transcript(&id).await.is_none());
    registry
        .store_transcript(&id, "line 1\nsk-testsecret123456\nline 2\n".to_string())
        .await;
    let content = registry.get_transcript(&id).await.unwrap();
    assert!(content.contains("line 1"));
    assert!(!content.contains("sk-testsecret123456"));
    assert!(content.contains("[REDACTED_TOKEN]"));
}

#[tokio::test]
async fn send_message_delivers_to_actor_inbox() {
    let registry = MailboxRegistry::new();
    let from = ActorId::new("Root").unwrap();
    let to = ActorId::new("Worker").unwrap();
    registry.track(test_record("Worker")).await;

    let receipt = registry
        .send_message(ActorMessage {
            from: from.clone(),
            to: to.clone(),
            text: "status?".to_string(),
            reply_to: None,
            sent_at: Utc::now(),
        })
        .await;

    assert_eq!(receipt, Receipt::Delivered);
    assert_eq!(registry.unread_count(&to).await, 1);
    let drained = registry.drain_inbox(&to, None).await;
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].from, from);
    assert_eq!(drained[0].text, "status?");
    assert_eq!(registry.unread_count(&to).await, 0);
}

#[tokio::test]
async fn send_message_to_terminated_actor_returns_unreachable() {
    let registry = MailboxRegistry::new();
    let to = ActorId::new("Worker").unwrap();
    registry.track(test_record("Worker")).await;
    registry.mark_complete(&to).await;

    let receipt = registry
        .send_message(ActorMessage {
            from: ActorId::new("Root").unwrap(),
            to: to.clone(),
            text: "status?".to_string(),
            reply_to: None,
            sent_at: Utc::now(),
        })
        .await;

    assert_eq!(receipt, Receipt::Unreachable);
    assert!(registry.drain_inbox(&to, None).await.is_empty());
}

#[tokio::test]
async fn send_message_to_full_inbox_returns_backpressured_and_drops() {
    let registry = MailboxRegistry::new();
    let to = ActorId::new("Worker").unwrap();
    registry.track(test_record("Worker")).await;

    for index in 0..MAX_INBOX_MESSAGES {
        let receipt = registry
            .send_message(ActorMessage {
                from: ActorId::new("Root").unwrap(),
                to: to.clone(),
                text: format!("queued {index}"),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;
        assert_eq!(receipt, Receipt::Delivered);
    }

    let receipt = registry
        .send_message(ActorMessage {
            from: ActorId::new("Root").unwrap(),
            to: to.clone(),
            text: "overflow".to_string(),
            reply_to: None,
            sent_at: Utc::now(),
        })
        .await;

    assert_eq!(receipt, Receipt::Backpressured);
    assert_eq!(registry.unread_count(&to).await, MAX_INBOX_MESSAGES);
    assert_eq!(registry.messages().await.len(), MAX_INBOX_MESSAGES);
    let drained = registry.drain_inbox(&to, None).await;
    assert_eq!(drained.len(), MAX_INBOX_MESSAGES);
    assert!(!drained.iter().any(|message| message.text == "overflow"));
}

#[tokio::test]
async fn live_direct_children_excludes_terminated_only() {
    let registry = MailboxRegistry::new();
    let root = ActorId::new("Root").unwrap();
    registry.track(test_record("RunningChild")).await;
    registry
        .track(ActorRecord {
            status: ActorStatus::Idle,
            ..test_record("IdleChild")
        })
        .await;
    registry
        .track(ActorRecord {
            status: ActorStatus::Parked,
            ..test_record("ParkedChild")
        })
        .await;
    registry
        .track(ActorRecord {
            status: ActorStatus::Terminated,
            completed_at: Some(Utc::now()),
            ..test_record("DoneChild")
        })
        .await;
    registry
        .track(test_record_with_parent("NestedChild", "RunningChild"))
        .await;

    let mut ids: Vec<_> = registry
        .live_direct_children(&root)
        .await
        .into_iter()
        .map(|record| record.id)
        .collect();
    ids.sort();

    assert_eq!(
        ids,
        vec![
            ActorId::new("IdleChild").unwrap(),
            ActorId::new("ParkedChild").unwrap(),
            ActorId::new("RunningChild").unwrap(),
        ]
    );
}

#[tokio::test]
async fn wait_for_message_filters_by_sender_without_losing_others() {
    let registry = MailboxRegistry::new();
    let worker = ActorId::new("Worker").unwrap();
    let alpha = ActorId::new("Alpha").unwrap();
    let beta = ActorId::new("Beta").unwrap();
    registry.track(test_record("Worker")).await;
    registry.track(test_record("Alpha")).await;
    registry.track(test_record("Beta")).await;

    for (from, text) in [(alpha.clone(), "from alpha"), (beta.clone(), "from beta")] {
        assert_eq!(
            registry
                .send_message(ActorMessage {
                    from,
                    to: worker.clone(),
                    text: text.to_string(),
                    reply_to: None,
                    sent_at: Utc::now(),
                })
                .await,
            Receipt::Delivered
        );
    }

    let beta_msg = registry
        .wait_for_message(&worker, Some(&beta), Duration::from_millis(50))
        .await
        .unwrap();
    assert_eq!(beta_msg.text, "from beta");

    let remaining = registry.drain_inbox(&worker, None).await;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].from, alpha);
    assert_eq!(remaining[0].text, "from alpha");
}

#[tokio::test]
async fn session_ownership_isolates_same_actor_id_root_mailboxes_and_cancellation() {
    let registry = MailboxRegistry::new();
    let worker = ActorId::new("Worker").unwrap();
    for session in ["session-a", "session-b"] {
        registry
            .track_for_session(session, test_record("Worker"))
            .await
            .unwrap();
    }

    registry
        .send_message_for_session(
            "session-a",
            ActorMessage {
                from: ActorId::new("Root").unwrap(),
                to: worker.clone(),
                text: "only a".to_string(),
                reply_to: None,
                sent_at: Utc::now(),
            },
        )
        .await
        .unwrap();
    assert!(
        registry
            .drain_inbox_for_session("session-b", &worker, None)
            .await
            .is_empty()
    );
    assert_eq!(
        registry
            .drain_inbox_for_session("session-a", &worker, None)
            .await[0]
            .text,
        "only a"
    );

    assert_eq!(
        registry.cancel_actor("session-a", &worker).await,
        CancelActorResult::Cancelled
    );
    assert!(
        registry
            .is_cancelled_for_session("session-a", &worker)
            .await
    );
    assert!(
        !registry
            .is_cancelled_for_session("session-b", &worker)
            .await
    );
    assert_eq!(
        registry
            .get_for_session("session-b", &worker)
            .await
            .unwrap()
            .status,
        ActorStatus::Running
    );

    for (session, text) in [("session-a", "root a"), ("session-b", "root b")] {
        registry
            .send_message_for_session(
                session,
                ActorMessage {
                    from: worker.clone(),
                    to: ActorId::new("Root").unwrap(),
                    text: text.to_string(),
                    reply_to: None,
                    sent_at: Utc::now(),
                },
            )
            .await
            .unwrap();
    }
    let root = ActorId::new("Root").unwrap();
    assert_eq!(
        registry
            .drain_inbox_for_session("session-a", &root, None)
            .await[0]
            .text,
        "root a"
    );
    assert_eq!(
        registry
            .drain_inbox_for_session("session-b", &root, None)
            .await[0]
            .text,
        "root b"
    );
}

#[tokio::test]
async fn message_and_aggregate_mailbox_budgets_fail_closed() {
    let registry = MailboxRegistry::new();
    let worker = ActorId::new("Worker").unwrap();
    registry
        .track_for_session("bounded", test_record("Worker"))
        .await
        .unwrap();

    let oversized = registry
        .send_message_for_session(
            "bounded",
            ActorMessage {
                from: ActorId::new("Root").unwrap(),
                to: worker.clone(),
                text: "x".repeat(crate::actor::MAX_ACTOR_MESSAGE_BYTES + 1),
                reply_to: None,
                sent_at: Utc::now(),
            },
        )
        .await;
    assert!(matches!(oversized, Err(RegistryError::InvalidText(_))));

    let payload = "x".repeat(crate::actor::MAX_ACTOR_MESSAGE_BYTES);
    let mut delivered = 0usize;
    loop {
        let receipt = registry
            .send_message_for_session(
                "bounded",
                ActorMessage {
                    from: ActorId::new("Root").unwrap(),
                    to: worker.clone(),
                    text: payload.clone(),
                    reply_to: None,
                    sent_at: Utc::now(),
                },
            )
            .await
            .unwrap();
        if receipt == Receipt::Backpressured {
            break;
        }
        delivered += 1;
    }
    assert!(delivered < MAX_INBOX_MESSAGES);

    for index in 0..200 {
        registry
            .record_message_for_session(
                "log-budget",
                ActorMessage {
                    from: ActorId::new("Root").unwrap(),
                    to: worker.clone(),
                    text: format!("{index:03}{}", "y".repeat(4_000)),
                    reply_to: None,
                    sent_at: Utc::now(),
                },
            )
            .await
            .unwrap();
    }
    let retained = registry.messages_for_session("log-budget").await;
    assert!(retained.len() < 200);
    assert!(
        retained
            .iter()
            .map(ActorMessage::retained_bytes)
            .sum::<usize>()
            <= MAX_MESSAGE_LOG_BYTES
    );
    assert!(retained.last().unwrap().text.starts_with("199"));
}

#[tokio::test]
async fn completed_actors_do_not_exhaust_the_live_session_limit() {
    let registry = MailboxRegistry::new();
    for index in 0..(MAX_ACTORS_PER_SESSION + 4) {
        let id = ActorId::new(format!("Worker{index}")).unwrap();
        registry
            .track_for_session("long-lived", test_record(id.as_str()))
            .await
            .unwrap();
        assert!(registry.mark_complete_for_session("long-lived", &id).await);
    }
}

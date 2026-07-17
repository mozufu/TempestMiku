use std::time::Duration;

use chrono::Utc;

use crate::actor::ActorId;
use crate::mailbox::{ActorMessage, MailboxRegistry, Receipt, RegistryError};

use super::super::inbox::MAX_INBOX_MESSAGES;
use super::super::messages::MAX_MESSAGE_LOG_BYTES;
use super::test_record;

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

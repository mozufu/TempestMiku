use chrono::Utc;

use crate::actor::{ActorId, ActorStatus};
use crate::mailbox::{ActorMessage, CancelActorResult, MailboxRegistry};

use super::test_record;

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

use std::sync::Arc;

use crate::actor::{ActorId, ActorLifecycleEvent, ActorStatus};
use crate::mailbox::MailboxRegistry;
use crate::supervise::{FailureReason, RestartStrategy, SupervisionAction, SupervisionPolicy};

use super::{test_record, test_record_with_parent};

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

#[tokio::test]
async fn mark_complete_with_resources_stores_uris() {
    let registry = MailboxRegistry::new();
    let id = ActorId::new("Worker").unwrap();
    registry.track(test_record("Worker")).await;
    registry
        .mark_complete_with_resources(
            &id,
            Some("artifact://1".to_string()),
            Some("history://Worker".to_string()),
        )
        .await;
    let rec = registry.get(&id).await.unwrap();
    assert_eq!(rec.status, ActorStatus::Terminated);
    assert_eq!(rec.artifact_uri.as_deref(), Some("artifact://1"));
    assert_eq!(rec.history_uri.as_deref(), Some("history://Worker"));
}

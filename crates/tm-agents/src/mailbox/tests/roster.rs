use chrono::Utc;

use crate::actor::{ActorId, ActorRecord, ActorStatus};
use crate::mailbox::{MAX_ACTORS_PER_SESSION, MailboxRegistry};

use super::{test_record, test_record_with_parent};

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

use super::super::*;
use super::support::*;

use crate::supervise::SupervisionAction;

#[tokio::test]
async fn agents_spawn_denied_without_grant() {
    let f = AgentsSpawnFn::new(make_roster());
    let ctx = ctx_without_agents();
    let err = f
        .call(json!({"role": "r", "task": "t"}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_SPAWN));
}

#[tokio::test]
async fn agents_spawn_executor_not_configured_returns_not_implemented() {
    let f = AgentsSpawnFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_SPAWN);
    let err = f
        .call(json!({"role": "r", "task": "t"}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::NotImplemented(_)));
}

#[tokio::test]
async fn agents_spawn_invalid_args_returns_invalid_args_error() {
    let roster = make_roster();
    let f = AgentsSpawnFn::new(roster);
    let ctx = ctx_with(caps::AGENTS_SPAWN);
    let err = f
        .call(json!({"role": 42, "task": "t"}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_spawn_returns_handle_actor_runs_in_background() {
    use crate::actor::{ActorDigest, ActorId, ActorSpec, ActorStatus};
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
    let f = AgentsSpawnFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_SPAWN);

    let result = f
        .call(json!({"role": "worker", "task": "do the thing"}), &ctx)
        .await
        .unwrap();

    let id_str = result["id"].as_str().expect("id field");
    let actor_id = ActorId::new(id_str).expect("valid actor id");

    // Actor is immediately tracked as Running
    let rec = roster.get(&actor_id).await.unwrap();
    assert_eq!(rec.status, ActorStatus::Running);

    // Wait for background thread to complete (EchoExecutor is trivially fast)
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let rec = roster.get(&actor_id).await.unwrap();
    assert_eq!(rec.status, ActorStatus::Terminated);
    assert!(rec.completed_at.is_some());
}

#[tokio::test]
async fn agents_spawn_supervision_group_cancels_sibling_on_failure() {
    use crate::actor::{ActorDigest, ActorId, ActorSpec};
    use crate::executor::{ActorError, ActorExecutor};

    struct FailAndWaitExecutor {
        roster: Arc<MailboxRegistry>,
    }

    #[async_trait::async_trait]
    impl ActorExecutor for FailAndWaitExecutor {
        async fn run_to_digest(
            &self,
            spec: ActorSpec,
        ) -> std::result::Result<ActorDigest, ActorError> {
            if spec.task == "fail" {
                let child = ActorId::new("WaiterChild").unwrap();
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
                while tokio::time::Instant::now() < deadline {
                    if self.roster.get(&child).await.is_some() {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                return Err(ActorError::Execution("spawn branch failed".to_string()));
            }

            track_actor(
                &self.roster,
                "WaiterChild",
                Some(spec.id.as_str()),
                ActorStatus::Running,
            )
            .await;

            let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
            while tokio::time::Instant::now() < deadline {
                if spec.cancellation.is_cancelled() {
                    return Err(ActorError::Cancelled);
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }

            Ok(ActorDigest {
                actor_id: spec.id,
                summary: "unexpected completion".to_string(),
                artifact_uri: None,
                history_uri: None,
                history_content: None,
            })
        }
    }

    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(FailAndWaitExecutor {
        roster: Arc::clone(&roster),
    }));
    let (events, hook) = lifecycle_hook();
    roster.set_lifecycle_hook(hook);

    let f = AgentsSpawnFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_SPAWN);
    let opts = json!({"supervision": {"group": "batch"}});
    f.call(
        json!({"role": "waiter", "task": "wait for cancellation", "opts": opts.clone()}),
        &ctx,
    )
    .await
    .unwrap();
    let waiter_child = ActorId::new("WaiterChild").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if roster.get(&waiter_child).await.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("waiter child should be tracked before failing sibling");
    f.call(
        json!({"role": "failer", "task": "fail", "opts": opts}),
        &ctx,
    )
    .await
    .unwrap();

    let waiter = ActorId::new("Waiter0").unwrap();
    let failer = ActorId::new("Failer1").unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let waiter_rec = roster.get(&waiter).await.expect("waiter record exists");
            let waiter_child_rec = roster
                .get(&waiter_child)
                .await
                .expect("waiter child record exists");
            let failer_rec = roster.get(&failer).await.expect("failer record exists");
            if waiter_rec.cancelled
                && waiter_child_rec.cancelled
                && failer_rec.failure_reason.is_some()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("spawn group failure should cancel sibling");

    let waiter_rec = roster.get(&waiter).await.unwrap();
    assert_eq!(waiter_rec.status, ActorStatus::Terminated);
    assert!(waiter_rec.cancelled);
    let waiter_child_rec = roster.get(&waiter_child).await.unwrap();
    assert_eq!(waiter_child_rec.status, ActorStatus::Terminated);
    assert!(waiter_child_rec.cancelled);
    let failer_rec = roster.get(&failer).await.unwrap();
    assert!(matches!(
        failer_rec.failure_reason,
        Some(FailureReason::Crash { .. })
    ));

    let evs = events.lock().unwrap();
    assert!(evs.iter().any(|event| matches!(
        event,
        ActorLifecycleEvent::Supervision {
            failed_actor_id,
            decision,
            ..
        } if failed_actor_id == &failer
            && decision.actions.iter().any(|action| matches!(
                action,
                SupervisionAction::Cancel { actor_id } if actor_id == &waiter
            ))
    )));
    assert!(evs.iter().any(|event| matches!(
        event,
        ActorLifecycleEvent::Cancelled { actor_id, .. } if actor_id == &waiter_child
    )));
}

#[tokio::test]
async fn agents_spawn_emits_spawned_and_eventually_completed() {
    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(EchoExec));
    let (events, hook) = lifecycle_hook();
    roster.set_lifecycle_hook(hook);

    let f = AgentsSpawnFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_SPAWN);
    f.call(json!({"role": "worker", "task": "bg"}), &ctx)
        .await
        .unwrap();

    // Spawned event fires synchronously before returning the handle
    {
        let evs = events.lock().unwrap();
        assert!(
            matches!(evs.first(), Some(ActorLifecycleEvent::Spawned { .. })),
            "first event should be Spawned immediately"
        );
    }

    // Background thread completes quickly (EchoExec is trivial); wait for Completed
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let evs = events.lock().unwrap();
    assert_eq!(
        evs.len(),
        3,
        "expected Spawned + StatusChanged + Completed after background completion, got {evs:?}"
    );
    assert!(matches!(
        &evs[1],
        ActorLifecycleEvent::StatusChanged {
            status: ActorStatus::Terminated,
            ..
        }
    ));
    assert!(matches!(&evs[2], ActorLifecycleEvent::Completed { .. }));
}

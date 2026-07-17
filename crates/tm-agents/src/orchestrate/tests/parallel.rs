use super::super::*;
use super::support::*;

use crate::supervise::SupervisionAction;

#[tokio::test]
async fn agents_parallel_denied_without_grant() {
    let f = AgentsParallelFn::new(make_roster());
    let ctx = ctx_without_agents();
    let err = f
        .call(json!({"tasks": [{"role": "r", "task": "t"}]}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_PARALLEL));
}

#[tokio::test]
async fn agents_parallel_executor_not_configured_returns_not_implemented() {
    let f = AgentsParallelFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_PARALLEL);
    let err = f
        .call(json!({"tasks": [{"role": "r", "task": "t"}]}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::NotImplemented(_)));
}

#[tokio::test]
async fn agents_parallel_empty_tasks_returns_empty_array() {
    let f = AgentsParallelFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_PARALLEL);
    let result = f.call(json!({"tasks": []}), &ctx).await.unwrap();
    assert_eq!(result, json!([]));
}

#[tokio::test]
async fn agents_parallel_invalid_args_returns_invalid_args_error() {
    let f = AgentsParallelFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_PARALLEL);
    // tasks is not an array
    let err = f.call(json!({"tasks": "oops"}), &ctx).await.unwrap_err();
    assert!(matches!(err, HostError::InvalidArgs(_)));
    // task.role is not a string
    let err = f
        .call(json!({"tasks": [{"role": 99, "task": "t"}]}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_parallel_runs_all_and_returns_ordered_digests() {
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
    let f = AgentsParallelFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_PARALLEL);

    let result = f
        .call(
            json!({"tasks": [
                {"role": "researcher", "task": "task A"},
                {"role": "writer",     "task": "task B"},
                {"role": "reviewer",   "task": "task C"},
            ]}),
            &ctx,
        )
        .await
        .unwrap();

    let digests = result.as_array().unwrap();
    assert_eq!(digests.len(), 3);

    // Results are ordered: digests[i] corresponds to tasks[i]
    assert!(digests[0]["summary"].as_str().unwrap().contains("task A"));
    assert!(digests[1]["summary"].as_str().unwrap().contains("task B"));
    assert!(digests[2]["summary"].as_str().unwrap().contains("task C"));

    // All actors have valid ids and are tracked as Terminated
    for d in digests {
        let id_str = d["actorId"].as_str().expect("actorId field");
        let actor_id = ActorId::new(id_str).expect("valid actor id");
        let rec = roster.get(&actor_id).await.unwrap();
        assert_eq!(rec.status, ActorStatus::Terminated);
    }
}

#[tokio::test]
async fn agents_parallel_applies_per_child_task_budgets() {
    use crate::actor::{ActorDigest, ActorSpec};
    use crate::executor::{ActorError, ActorExecutor};

    struct BudgetExecutor {
        budgets: Arc<std::sync::Mutex<Vec<ActorBudget>>>,
    }

    #[async_trait::async_trait]
    impl ActorExecutor for BudgetExecutor {
        async fn run_to_digest(
            &self,
            spec: ActorSpec,
        ) -> std::result::Result<ActorDigest, ActorError> {
            self.budgets.lock().unwrap().push(spec.budget.clone());
            Ok(ActorDigest {
                actor_id: spec.id,
                summary: "ok".to_string(),
                artifact_uri: None,
                history_uri: None,
                history_content: None,
            })
        }
    }

    let budgets = Arc::new(std::sync::Mutex::new(Vec::new()));
    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(BudgetExecutor {
        budgets: Arc::clone(&budgets),
    }));
    let f = AgentsParallelFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_PARALLEL);

    f.call(
        json!({"tasks": [
            {"role": "researcher", "task": "task A", "timeoutMs": 42},
            {"role": "writer", "task": "task B", "budget": {"wallMs": 55, "maxDepth": 2}},
        ]}),
        &ctx,
    )
    .await
    .unwrap();

    let budgets = budgets.lock().unwrap().clone();
    assert_eq!(budgets.len(), 2);
    assert_eq!(budgets[0].wall_ms, 42);
    assert_eq!(budgets[0].max_depth, ActorBudget::default().max_depth);
    assert_eq!(budgets[1].wall_ms, 55);
    assert_eq!(budgets[1].max_depth, 2);
}

#[tokio::test]
async fn agents_parallel_failure_cancels_sibling_wave_members() {
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
                return Err(ActorError::Execution("parallel branch failed".to_string()));
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

    let f = AgentsParallelFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_PARALLEL);
    let err = f
        .call(
            json!({"tasks": [
                {"role": "failer", "task": "fail"},
                {"role": "waiter", "task": "wait for cancellation"}
            ]}),
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(matches!(err, HostError::HostCall(_)));
    let failed = ActorId::new("Failer0").unwrap();
    let sibling = ActorId::new("Waiter1").unwrap();
    let sibling_child = ActorId::new("WaiterChild").unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let rec = roster.get(&sibling).await.expect("sibling record exists");
            let child_rec = roster
                .get(&sibling_child)
                .await
                .expect("sibling child record exists");
            if rec.cancelled && child_rec.cancelled {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("sibling should be cancelled by wave supervision");

    let failed_rec = roster.get(&failed).await.unwrap();
    assert!(matches!(
        failed_rec.failure_reason,
        Some(FailureReason::Crash { .. })
    ));
    let sibling_rec = roster.get(&sibling).await.unwrap();
    assert_eq!(sibling_rec.status, ActorStatus::Terminated);
    assert!(sibling_rec.cancelled);
    let sibling_child_rec = roster.get(&sibling_child).await.unwrap();
    assert_eq!(sibling_child_rec.status, ActorStatus::Terminated);
    assert!(sibling_child_rec.cancelled);

    let evs = events.lock().unwrap();
    assert!(evs.iter().any(|event| matches!(
        event,
        ActorLifecycleEvent::Supervision {
            failed_actor_id,
            decision,
            ..
        } if failed_actor_id == &failed
            && decision.actions.iter().any(|action| matches!(
                action,
                SupervisionAction::Cancel { actor_id } if actor_id == &sibling
            ))
            && decision.actions.iter().any(|action| matches!(
                action,
                SupervisionAction::Escalate { actor_id, .. } if actor_id == &failed
            ))
    )));
    assert!(evs.iter().any(|event| matches!(
        event,
        ActorLifecycleEvent::Cancelled { actor_id, .. } if actor_id == &sibling_child
    )));
}

#[tokio::test]
async fn agents_parallel_emits_spawned_and_completed_for_each_actor() {
    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(EchoExec));
    let (events, hook) = lifecycle_hook();
    roster.set_lifecycle_hook(hook);

    let f = AgentsParallelFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_PARALLEL);
    f.call(
        json!({"tasks": [{"role": "worker", "task": "A"}, {"role": "worker", "task": "B"}]}),
        &ctx,
    )
    .await
    .unwrap();

    let evs = events.lock().unwrap();
    // 2 Spawned + 2 Completed = 4 total (order: Spawned A, Spawned B, then Completed in any order)
    let spawned = evs
        .iter()
        .filter(|e| matches!(e, ActorLifecycleEvent::Spawned { .. }))
        .count();
    let completed = evs
        .iter()
        .filter(|e| matches!(e, ActorLifecycleEvent::Completed { .. }))
        .count();
    assert_eq!(spawned, 2, "expected 2 Spawned events");
    assert_eq!(completed, 2, "expected 2 Completed events");
}

#[tokio::test]
async fn agents_parallel_bounds_concurrency_and_cancels_whole_wave_on_drop() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ActiveGuard(Arc<AtomicUsize>);
    impl Drop for ActiveGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct BoundedExecutor {
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl crate::executor::ActorExecutor for BoundedExecutor {
        async fn run_to_digest(
            &self,
            spec: crate::actor::ActorSpec,
        ) -> std::result::Result<crate::actor::ActorDigest, crate::executor::ActorError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            let _guard = ActiveGuard(Arc::clone(&self.active));
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok(crate::actor::ActorDigest {
                actor_id: spec.id,
                summary: "done".to_string(),
                artifact_uri: None,
                history_uri: None,
                history_content: None,
            })
        }
    }

    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(BoundedExecutor {
        active: Arc::clone(&active),
        peak: Arc::clone(&peak),
    }));
    let parallel = AgentsParallelFn::new(Arc::clone(&roster));
    let tasks = (0..MAX_ACTORS_PER_CALL)
        .map(|index| json!({"role":"worker","task":format!("task {index}")}))
        .collect::<Vec<_>>();
    parallel
        .call(json!({"tasks":tasks}), &ctx_with(caps::AGENTS_PARALLEL))
        .await
        .unwrap();
    assert!(peak.load(Ordering::SeqCst) <= MAX_CONCURRENT_ACTORS);
    assert!(peak.load(Ordering::SeqCst) > 1);
    assert_eq!(active.load(Ordering::SeqCst), 0);

    struct PendingExecutor {
        started: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl crate::executor::ActorExecutor for PendingExecutor {
        async fn run_to_digest(
            &self,
            _spec: crate::actor::ActorSpec,
        ) -> std::result::Result<crate::actor::ActorDigest, crate::executor::ActorError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        }
    }

    let started = Arc::new(AtomicUsize::new(0));
    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(PendingExecutor {
        started: Arc::clone(&started),
    }));
    let parallel = AgentsParallelFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_PARALLEL);
    let tasks = (0..MAX_ACTORS_PER_CALL)
        .map(|index| json!({"role":"worker","task":format!("pending {index}")}))
        .collect::<Vec<_>>();
    let handle = tokio::spawn(async move { parallel.call(json!({"tasks":tasks}), &ctx).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while started.load(Ordering::SeqCst) < MAX_CONCURRENT_ACTORS {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle.abort();
    let _ = handle.await;
    tokio::task::yield_now().await;

    let records = roster.list_for_session("").await;
    assert_eq!(records.len(), MAX_ACTORS_PER_CALL);
    for record in records {
        assert!(
            roster.is_cancelled_for_session("", &record.id).await,
            "{} token should be cancelled when the wave future is dropped",
            record.id
        );
    }
}

use super::super::*;
use super::support::*;

use crate::supervise::SupervisionAction;
use tm_host::CapabilityGrants;

#[tokio::test]
async fn agents_run_denied_without_grant() {
    let f = AgentsRunFn::new(make_roster());
    let ctx = ctx_without_agents();
    let err = f
        .call(json!({"role": "r", "task": "t"}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_RUN));
}

#[tokio::test]
async fn agents_run_executor_not_configured_returns_not_implemented() {
    let f = AgentsRunFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_RUN);
    let err = f
        .call(json!({"role": "r", "task": "t"}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::NotImplemented(_)));
}

#[tokio::test]
async fn agents_run_preserves_authoritative_session_scope_for_child_executor() {
    use crate::actor::{ActorDigest, ActorSpec};
    use crate::executor::{ActorError, ActorExecutor};

    struct ScopeExecutor(Arc<std::sync::Mutex<Option<String>>>);

    #[async_trait::async_trait]
    impl ActorExecutor for ScopeExecutor {
        async fn run_to_digest(
            &self,
            spec: ActorSpec,
        ) -> std::result::Result<ActorDigest, ActorError> {
            *self.0.lock().unwrap() = spec.session_scope.clone();
            Ok(ActorDigest {
                actor_id: spec.id,
                summary: "scoped".to_string(),
                artifact_uri: None,
                history_uri: None,
                history_content: None,
            })
        }
    }

    let captured = Arc::new(std::sync::Mutex::new(None));
    let roster = make_roster();
    roster.set_executor(Arc::new(ScopeExecutor(Arc::clone(&captured))));
    let function = AgentsRunFn::new(roster);
    let context = ctx_with(caps::AGENTS_RUN)
        .with_session_id("session-scope-test")
        .with_session_scope("project:tempestmiku");
    function
        .call(json!({"role": "worker", "task": "check scope"}), &context)
        .await
        .unwrap();
    assert_eq!(
        captured.lock().unwrap().as_deref(),
        Some("project:tempestmiku")
    );
}

#[tokio::test]
async fn agents_run_invalid_args_returns_invalid_args_error() {
    let roster = make_roster();
    let f = AgentsRunFn::new(roster);
    let ctx = ctx_with(caps::AGENTS_RUN);
    // role is not a string
    let err = f
        .call(json!({"role": 42, "task": "t"}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_run_with_executor_tracks_and_returns_digest() {
    use crate::actor::ActorSpec;
    use crate::actor::{ActorDigest, ActorId};
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
    let f = AgentsRunFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_RUN);

    let result = f
        .call(json!({"role": "researcher", "task": "do the thing"}), &ctx)
        .await
        .unwrap();

    assert!(result["summary"].as_str().unwrap().contains("do the thing"));
    assert!(result["actorId"].as_str().is_some());

    // Actor is tracked as terminated in registry
    let actor_id_str = result["actorId"].as_str().unwrap();
    let actor_id = ActorId::new(actor_id_str).unwrap();
    let rec = roster.get(&actor_id).await.unwrap();
    assert_eq!(rec.status, crate::actor::ActorStatus::Terminated);
    assert!(rec.completed_at.is_some());
}

#[tokio::test]
async fn agents_run_records_current_actor_as_parent() {
    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(EchoExec));
    let f = AgentsRunFn::new(Arc::clone(&roster));
    let ctx = ctx_with_actor(caps::AGENTS_RUN, "Parent");

    let result = f
        .call(json!({"role": "worker", "task": "child task"}), &ctx)
        .await
        .unwrap();

    let actor_id = ActorId::new(result["actorId"].as_str().unwrap()).unwrap();
    let rec = roster.get(&actor_id).await.unwrap();
    assert_eq!(rec.parent.as_ref().map(|id| id.as_str()), Some("Parent"));
}

#[tokio::test]
async fn agents_run_emits_spawned_and_completed_lifecycle_events() {
    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(EchoExec));
    let (events, hook) = lifecycle_hook();
    roster.set_lifecycle_hook(hook);

    let f = AgentsRunFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_RUN);
    f.call(json!({"role": "worker", "task": "do it"}), &ctx)
        .await
        .unwrap();

    let evs = events.lock().unwrap();
    assert_eq!(
        evs.len(),
        3,
        "expected Spawned + StatusChanged + Completed, got {evs:?}"
    );
    assert!(matches!(&evs[0], ActorLifecycleEvent::Spawned { role, .. } if role == "worker"));
    assert!(matches!(
        &evs[1],
        ActorLifecycleEvent::StatusChanged {
            status: ActorStatus::Terminated,
            ..
        }
    ));
    assert!(
        matches!(&evs[2], ActorLifecycleEvent::Completed { summary, .. } if summary.as_deref() == Some("echo: do it"))
    );
}

#[tokio::test]
async fn agents_run_emits_failed_lifecycle_event_on_error() {
    use crate::executor::ActorError;
    struct FailExecutor;
    #[async_trait::async_trait]
    impl crate::executor::ActorExecutor for FailExecutor {
        async fn run_to_digest(
            &self,
            _spec: crate::actor::ActorSpec,
        ) -> std::result::Result<crate::actor::ActorDigest, ActorError> {
            Err(ActorError::Execution("boom".to_string()))
        }
    }

    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(FailExecutor));
    let (events, hook) = lifecycle_hook();
    roster.set_lifecycle_hook(hook);

    let f = AgentsRunFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_RUN);
    let _ = f
        .call(json!({"role": "worker", "task": "fail"}), &ctx)
        .await;

    let evs = events.lock().unwrap();
    assert!(
        evs.len() >= 4,
        "expected at least Spawned + StatusChanged + Failed + Supervision, got {evs:?}"
    );
    assert!(matches!(&evs[0], ActorLifecycleEvent::Spawned { .. }));
    assert!(matches!(
        &evs[1],
        ActorLifecycleEvent::StatusChanged {
            status: ActorStatus::Terminated,
            ..
        }
    ));
    assert!(matches!(&evs[2], ActorLifecycleEvent::Failed { .. }));
    assert!(matches!(
        &evs[3],
        ActorLifecycleEvent::Supervision { decision, .. }
            if matches!(
                decision.actions.as_slice(),
                [SupervisionAction::Restart { .. }]
            )
    ));
}

#[tokio::test]
async fn agents_run_failure_respawns_actor_from_supervision_restart_action() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::actor::{ActorDigest, ActorSpec};
    use crate::executor::{ActorError, ActorExecutor};

    struct FlakyExecutor {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ActorExecutor for FlakyExecutor {
        async fn run_to_digest(
            &self,
            spec: ActorSpec,
        ) -> std::result::Result<ActorDigest, ActorError> {
            let attempt = self.calls.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return Err(ActorError::Execution("first attempt failed".to_string()));
            }
            Ok(ActorDigest {
                actor_id: spec.id,
                summary: format!("restarted attempt {attempt}"),
                artifact_uri: None,
                history_uri: Some("history://Worker0".to_string()),
                history_content: Some("restart transcript".to_string()),
            })
        }
    }

    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(FlakyExecutor {
        calls: AtomicUsize::new(0),
    }));
    let (events, hook) = lifecycle_hook();
    roster.set_lifecycle_hook(hook);

    let f = AgentsRunFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_RUN);
    let err = f
        .call(json!({"role": "worker", "task": "recover"}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::HostCall(_)));

    let actor_id = ActorId::new("Worker0").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let rec = roster.get(&actor_id).await.expect("actor record exists");
            if rec.status == ActorStatus::Terminated
                && rec.failure_reason.is_none()
                && rec.last_summary.as_deref() == Some("restarted attempt 1")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("restart should complete");

    let rec = roster.get(&actor_id).await.unwrap();
    assert!(!rec.cancelled);
    assert_eq!(rec.history_uri.as_deref(), Some("history://Worker0"));
    assert_eq!(
        roster.get_transcript(&actor_id).await.as_deref(),
        Some("restart transcript")
    );

    let evs = events.lock().unwrap();
    assert!(matches!(&evs[0], ActorLifecycleEvent::Spawned { .. }));
    assert!(matches!(
        &evs[1],
        ActorLifecycleEvent::StatusChanged {
            status: ActorStatus::Terminated,
            ..
        }
    ));
    assert!(matches!(&evs[2], ActorLifecycleEvent::Failed { .. }));
    assert!(matches!(&evs[3], ActorLifecycleEvent::Supervision { .. }));
    assert!(evs.iter().any(|event| matches!(
        event,
        ActorLifecycleEvent::Completed {
            actor_id: completed_id,
            summary,
            ..
        } if completed_id == &actor_id && summary.as_deref() == Some("restarted attempt 1")
    )));
}

#[tokio::test]
async fn actor_wall_clock_timeout_trips_cancellation_token() {
    use crate::actor::{ActorCancelToken, ActorDigest, ActorSpec};
    use crate::executor::{ActorError, ActorExecutor};

    struct SlowExecutor;
    #[async_trait::async_trait]
    impl ActorExecutor for SlowExecutor {
        async fn run_to_digest(
            &self,
            spec: ActorSpec,
        ) -> std::result::Result<ActorDigest, ActorError> {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok(ActorDigest {
                actor_id: spec.id,
                summary: "late".to_string(),
                artifact_uri: None,
                history_uri: None,
                history_content: None,
            })
        }
    }

    let cancellation = ActorCancelToken::default();
    let spec = ActorSpec {
        id: ActorId::new("SlowWorker").unwrap(),
        session_id: "session-1".to_string(),
        session_scope: None,
        role: "worker".to_string(),
        task: "sleep".to_string(),
        mode: None,
        grants: CapabilityGrants::default(),
        budget: ActorBudget {
            wall_ms: 1,
            max_depth: 4,
        },
        parent: None,
        depth: 0,
        cancellation: cancellation.clone(),
    };

    let err = run_actor_to_digest(Arc::new(SlowExecutor), spec)
        .await
        .unwrap_err();

    assert!(matches!(err, ActorError::Timeout));
    assert!(cancellation.is_cancelled());
}

#[tokio::test]
async fn lifecycle_hook_no_op_when_not_set() {
    // Verify emit_lifecycle is safe with no hook installed (no panic, no error)
    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(EchoExec));
    let f = AgentsRunFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_RUN);
    // Should not panic even though no lifecycle hook is set
    f.call(json!({"role": "worker", "task": "t"}), &ctx)
        .await
        .unwrap();
}

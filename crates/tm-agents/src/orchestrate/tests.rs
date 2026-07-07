use super::*;
use crate::supervise::SupervisionAction;
use tm_host::{CapabilityGrants, InvocationCtx};

fn make_roster() -> Arc<MailboxRegistry> {
    Arc::new(MailboxRegistry::new())
}

fn ctx_without_agents() -> InvocationCtx {
    InvocationCtx::new(CapabilityGrants::default())
}

fn ctx_with(cap: &str) -> InvocationCtx {
    InvocationCtx::new(CapabilityGrants::default().allow(cap))
}

fn ctx_with_actor(cap: &str, actor_id: &str) -> InvocationCtx {
    InvocationCtx::new(CapabilityGrants::default().allow(cap))
        .with_actor_id(Some(actor_id.to_string()))
}

async fn track_running(roster: &MailboxRegistry, id: &str) {
    track_actor(roster, id, None, ActorStatus::Running).await;
}

async fn track_actor(
    roster: &MailboxRegistry,
    id: &str,
    parent: Option<&str>,
    status: ActorStatus,
) {
    roster
        .track(crate::actor::ActorRecord {
            id: ActorId::new(id).unwrap(),
            parent: parent.map(|id| ActorId::new(id).unwrap()),
            status,
            mode: Some("worker".to_string()),
            budget: ActorBudget::default(),
            spawned_at: chrono::Utc::now(),
            completed_at: matches!(status, ActorStatus::Terminated).then(chrono::Utc::now),
            cancelled: false,
            failure_reason: None,
            last_summary: None,
            artifact_uri: None,
            history_uri: None,
        })
        .await;
}

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

    struct FailAndWaitExecutor;

    #[async_trait::async_trait]
    impl ActorExecutor for FailAndWaitExecutor {
        async fn run_to_digest(
            &self,
            spec: ActorSpec,
        ) -> std::result::Result<ActorDigest, ActorError> {
            if spec.task == "fail" {
                return Err(ActorError::Execution("spawn branch failed".to_string()));
            }

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
    roster.set_executor(Arc::new(FailAndWaitExecutor));
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
            let failer_rec = roster.get(&failer).await.expect("failer record exists");
            if waiter_rec.cancelled && failer_rec.failure_reason.is_some() {
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
}

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
async fn agents_parallel_failure_cancels_sibling_wave_members() {
    use crate::actor::{ActorDigest, ActorId, ActorSpec};
    use crate::executor::{ActorError, ActorExecutor};

    struct FailAndWaitExecutor;

    #[async_trait::async_trait]
    impl ActorExecutor for FailAndWaitExecutor {
        async fn run_to_digest(
            &self,
            spec: ActorSpec,
        ) -> std::result::Result<ActorDigest, ActorError> {
            if spec.task == "fail" {
                return Err(ActorError::Execution("parallel branch failed".to_string()));
            }

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
    roster.set_executor(Arc::new(FailAndWaitExecutor));
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

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let rec = roster.get(&sibling).await.expect("sibling record exists");
            if rec.cancelled {
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
}

#[tokio::test]
async fn agents_pipeline_denied_without_grant() {
    let f = AgentsPipelineFn::new(make_roster());
    let ctx = ctx_without_agents();
    let err = f
        .call(
            json!({"items": ["A"], "stages": [{"role": "r", "task": "t"}]}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_PIPELINE));
}

#[tokio::test]
async fn agents_pipeline_executor_not_configured_returns_not_implemented() {
    let f = AgentsPipelineFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_PIPELINE);
    let err = f
        .call(
            json!({"items": ["A"], "stages": [{"role": "r", "task": "t"}]}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::NotImplemented(_)));
}

#[tokio::test]
async fn agents_pipeline_invalid_args_returns_invalid_args_error() {
    let roster = make_roster();
    roster.set_executor(Arc::new(EchoExec));
    let f = AgentsPipelineFn::new(roster);
    let ctx = ctx_with(caps::AGENTS_PIPELINE);

    for args in [
        json!({"items": "oops", "stages": [{"role": "r", "task": "t"}]}),
        json!({"items": ["A"], "stages": []}),
        json!({"items": ["A"], "stages": [{"role": 99, "task": "t"}]}),
        json!({"items": ["A"], "stages": [{"role": "r"}]}),
        json!({"items": ["A"], "stages": [{"role": "r", "tasks": []}]}),
    ] {
        let err = f.call(args, &ctx).await.unwrap_err();
        assert!(matches!(err, HostError::InvalidArgs(_)));
    }
}

#[tokio::test]
async fn agents_pipeline_runs_stage_barriers_and_feeds_digests() {
    struct ReferenceExec;
    #[async_trait::async_trait]
    impl crate::executor::ActorExecutor for ReferenceExec {
        async fn run_to_digest(
            &self,
            spec: crate::actor::ActorSpec,
        ) -> std::result::Result<crate::actor::ActorDigest, crate::executor::ActorError> {
            Ok(crate::actor::ActorDigest {
                artifact_uri: Some(format!("artifact://{}", spec.id.as_str())),
                history_uri: Some(format!("history://{}", spec.id.as_str())),
                history_content: Some(format!("FULL TRANSCRIPT: {}", spec.task)),
                actor_id: spec.id,
                summary: format!("echo: {}", spec.task),
            })
        }
    }

    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(ReferenceExec));
    let f = AgentsPipelineFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_PIPELINE);

    let result = f
        .call(
            json!({
                "items": ["alpha", "beta"],
                "stages": [
                    {"role": "researcher", "task": "research this input"},
                    {"role": "writer", "task": "summarize this digest"}
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();

    let stages = result.as_array().unwrap();
    assert_eq!(stages.len(), 2);
    let first = stages[0].as_array().unwrap();
    let second = stages[1].as_array().unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 2);
    assert!(first[0]["summary"].as_str().unwrap().contains("alpha"));
    assert!(first[1]["summary"].as_str().unwrap().contains("beta"));
    assert!(
        second[0]["summary"]
            .as_str()
            .unwrap()
            .contains("research this input"),
        "second stage should receive the first-stage digest reference"
    );
    assert!(
        second[0]["summary"]
            .as_str()
            .unwrap()
            .contains("agentUri=agent://Researcher0"),
        "second stage should receive the upstream actor by reference"
    );
    assert!(
        second[0]["summary"]
            .as_str()
            .unwrap()
            .contains("historyUri=history://Researcher0"),
        "second stage should receive the upstream transcript handle"
    );
    assert!(
        second[0]["summary"]
            .as_str()
            .unwrap()
            .contains("artifactUri=artifact://Researcher0"),
        "second stage should receive the upstream artifact handle"
    );
    assert!(
        !second[0]["summary"]
            .as_str()
            .unwrap()
            .contains("FULL TRANSCRIPT"),
        "pipeline must not re-inline upstream transcripts"
    );

    let records = roster.list().await;
    assert_eq!(records.len(), 4);
    assert!(
        records
            .iter()
            .all(|record| record.status == ActorStatus::Terminated)
    );
}

#[tokio::test]
async fn agents_msg_denied_without_grant() {
    let f = AgentsMsgFn::new(make_roster());
    let ctx = ctx_without_agents();
    let err = f
        .call(json!({"handle": {"id": "W"}, "text": "hello"}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_MSG));
}

#[tokio::test]
async fn agents_msg_invalid_args() {
    let f = AgentsMsgFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_MSG);

    // Non-string text
    let err = f
        .call(json!({"handle": {"id": "Worker"}, "text": 42}), &ctx)
        .await
        .unwrap_err();
    assert!(
        matches!(err, HostError::InvalidArgs(_)),
        "expected InvalidArgs, got {err:?}"
    );

    // Missing handle.id
    let err = f
        .call(json!({"handle": {}, "text": "hello"}), &ctx)
        .await
        .unwrap_err();
    assert!(
        matches!(err, HostError::InvalidArgs(_)),
        "expected InvalidArgs, got {err:?}"
    );

    // handle.id is not valid CamelCase (lowercase start)
    let err = f
        .call(json!({"handle": {"id": "worker"}, "text": "hello"}), &ctx)
        .await
        .unwrap_err();
    assert!(
        matches!(err, HostError::InvalidArgs(_)),
        "expected InvalidArgs, got {err:?}"
    );
}

#[tokio::test]
async fn agents_msg_rejects_json_control_payload() {
    let f = AgentsMsgFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_MSG);

    let err = f
        .call(
            json!({"handle": {"id": "Worker"}, "text": "{\"type\":\"done\"}"}),
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_msg_await_rejects_descendant_wait_edge() {
    let roster = make_roster();
    track_actor(&roster, "Parent", None, ActorStatus::Running).await;
    track_actor(&roster, "Child", Some("Parent"), ActorStatus::Running).await;
    let f = AgentsMsgFn::new(Arc::clone(&roster));
    let ctx = ctx_with_actor(caps::AGENTS_MSG, "Parent");

    let err = f
        .call(
            json!({"handle": {"id": "Child"}, "text": "status?", "opts": {"await": true}}),
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
    assert!(roster.messages().await.is_empty());
}

#[tokio::test]
async fn agents_msg_unknown_handle_returns_null() {
    let f = AgentsMsgFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_MSG);
    // "Nope" is CamelCase but not in roster
    let result = f
        .call(json!({"handle": {"id": "Nope"}, "text": "hi"}), &ctx)
        .await
        .unwrap();
    assert_eq!(result, Value::Null);
}

#[tokio::test]
async fn agents_msg_fire_and_forget_returns_delivered_receipt() {
    use crate::actor::{ActorBudget, ActorId, ActorRecord, ActorStatus};
    let roster = make_roster();
    // Pre-populate a tracked actor
    roster
        .track(ActorRecord {
            id: ActorId::new("Worker").unwrap(),
            parent: None,
            status: ActorStatus::Running,
            mode: Some("worker".to_string()),
            budget: ActorBudget::default(),
            spawned_at: chrono::Utc::now(),
            completed_at: None,
            cancelled: false,
            failure_reason: None,
            last_summary: None,
            artifact_uri: None,
            history_uri: None,
        })
        .await;

    let f = AgentsMsgFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_MSG);

    let result = f
        .call(json!({"handle": {"id": "Worker"}, "text": "status?"}), &ctx)
        .await
        .unwrap();
    assert_eq!(result, json!({"status": "delivered"}));

    let messages = roster.messages().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].from.as_str(), "Root");
    assert_eq!(messages[0].to.as_str(), "Worker");
    assert_eq!(messages[0].text, "status?");
}

#[tokio::test]
async fn agents_msg_fire_and_forget_reports_full_inbox() {
    let roster = make_roster();
    track_running(&roster, "Worker").await;

    for index in 0..64 {
        let receipt = roster
            .send_message(ActorMessage {
                from: ActorId::new("Root").unwrap(),
                to: ActorId::new("Worker").unwrap(),
                text: format!("queued {index}"),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;
        assert_eq!(receipt, Receipt::Delivered);
    }

    let f = AgentsMsgFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_MSG);
    let result = f
        .call(
            json!({"handle": {"id": "Worker"}, "text": "overflow"}),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(
        result,
        json!({"status": "failed", "reason": "backpressured"})
    );
    let drained = roster
        .drain_inbox(&ActorId::new("Worker").unwrap(), None)
        .await;
    assert!(!drained.iter().any(|message| message.text == "overflow"));
}

#[tokio::test]
async fn agents_msg_await_returns_reply() {
    use crate::actor::{ActorBudget, ActorDigest, ActorId, ActorRecord, ActorSpec, ActorStatus};
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

    // Track a completed actor with a stored summary
    let target_id = ActorId::new("ResearchWorker").unwrap();
    roster
        .track(ActorRecord {
            id: target_id.clone(),
            parent: None,
            status: ActorStatus::Terminated,
            mode: Some("researcher".to_string()),
            budget: ActorBudget::default(),
            spawned_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            cancelled: false,
            failure_reason: None,
            last_summary: Some("I found the answer.".to_string()),
            artifact_uri: None,
            history_uri: None,
        })
        .await;

    let f = AgentsMsgFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_MSG);

    let result = f
            .call(
                json!({"handle": {"id": "ResearchWorker"}, "text": "elaborate please", "opts": {"await": true}}),
                &ctx,
            )
            .await
            .unwrap();

    // EchoExecutor echoes the seeded task; result should contain both prior context and the new text
    let reply = result.as_str().expect("reply should be a string");
    assert!(
        reply.contains("I found the answer."),
        "reply should contain prior context"
    );
    assert!(
        reply.contains("elaborate please"),
        "reply should contain new message"
    );
}

#[tokio::test]
async fn agents_msg_await_without_executor_returns_not_implemented() {
    use crate::actor::{ActorBudget, ActorId, ActorRecord, ActorStatus};
    let roster = make_roster(); // no executor set
    roster
        .track(ActorRecord {
            id: ActorId::new("Worker").unwrap(),
            parent: None,
            status: ActorStatus::Terminated,
            mode: None,
            budget: ActorBudget::default(),
            spawned_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            cancelled: false,
            failure_reason: None,
            last_summary: None,
            artifact_uri: None,
            history_uri: None,
        })
        .await;

    let f = AgentsMsgFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_MSG);

    let err = f
        .call(
            json!({"handle": {"id": "Worker"}, "text": "hi", "opts": {"await": true}}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, HostError::NotImplemented(_)),
        "expected NotImplemented, got {err:?}"
    );
}

#[test]
fn caps_module_defines_mvp_set() {
    assert_eq!(caps::AGENTS_RUN, "agents.run");
    assert_eq!(caps::AGENTS_SPAWN, "agents.spawn");
    assert_eq!(caps::AGENTS_PARALLEL, "agents.parallel");
    assert_eq!(caps::AGENTS_PIPELINE, "agents.pipeline");
    assert_eq!(caps::AGENTS_MSG, "agents.msg");
    assert_eq!(caps::AGENTS_SEND, "agents.send");
    assert_eq!(caps::AGENTS_BROADCAST, "agents.broadcast");
    assert_eq!(caps::AGENTS_CANCEL, "agents.cancel");
    assert_eq!(caps::AGENTS_WAIT, "agents.wait");
    assert_eq!(caps::AGENTS_INBOX, "agents.inbox");
    assert_eq!(caps::AGENTS_LIST, "agents.list");
}

#[tokio::test]
async fn agents_cancel_denied_without_grant() {
    let f = AgentsCancelFn::new(make_roster());
    let ctx = ctx_without_agents();
    let err = f.call(json!({"target": "Worker"}), &ctx).await.unwrap_err();
    assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_CANCEL));
}

#[tokio::test]
async fn agents_cancel_root_child_marks_terminal_and_emits_once() {
    let roster = make_roster();
    track_running(&roster, "Worker").await;
    let (events, hook) = lifecycle_hook();
    roster.set_lifecycle_hook(hook);

    let f = AgentsCancelFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_CANCEL);
    let result = f
        .call(json!({"target": {"id": "Worker"}}), &ctx)
        .await
        .unwrap();

    assert_eq!(result["actorId"], Value::String("Worker".into()));
    assert_eq!(result["status"], Value::String("cancelled".into()));
    let rec = roster.get(&ActorId::new("Worker").unwrap()).await.unwrap();
    assert_eq!(rec.status, ActorStatus::Terminated);
    assert!(rec.cancelled);
    assert!(matches!(rec.failure_reason, Some(FailureReason::Cancelled)));
    assert!(roster.is_cancelled(&ActorId::new("Worker").unwrap()).await);

    let receipt = roster
        .send_message(ActorMessage {
            from: ActorId::new("Root").unwrap(),
            to: ActorId::new("Worker").unwrap(),
            text: "status?".to_string(),
            reply_to: None,
            sent_at: Utc::now(),
        })
        .await;
    assert_eq!(receipt, Receipt::Unreachable);

    let second = f.call(json!({"target": "Worker"}), &ctx).await.unwrap();
    assert_eq!(second["status"], Value::String("already_cancelled".into()));

    let evs = events.lock().unwrap();
    let cancelled: Vec<_> = evs
        .iter()
        .filter(|event| matches!(event, ActorLifecycleEvent::Cancelled { .. }))
        .collect();
    assert_eq!(cancelled.len(), 1, "expected one Cancelled event");
}

#[tokio::test]
async fn agents_cancel_rejects_non_child_target() {
    let roster = make_roster();
    track_actor(&roster, "Parent", None, ActorStatus::Running).await;
    track_actor(&roster, "Child", Some("Parent"), ActorStatus::Running).await;
    let f = AgentsCancelFn::new(roster);
    let ctx = ctx_with_actor(caps::AGENTS_CANCEL, "Sibling");

    let err = f.call(json!({"target": "Child"}), &ctx).await.unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_send_delivers_to_live_inbox() {
    let roster = make_roster();
    track_running(&roster, "Worker").await;
    let f = AgentsSendFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_SEND);

    let result = f
        .call(json!({"to": "Worker", "text": "status?"}), &ctx)
        .await
        .unwrap();

    assert_eq!(result["status"], Value::String("delivered".into()));
    assert_eq!(
        roster.unread_count(&ActorId::new("Worker").unwrap()).await,
        1
    );
    let messages = roster.messages().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].from.as_str(), "Root");
    assert_eq!(messages[0].to.as_str(), "Worker");
}

#[tokio::test]
async fn agents_send_to_unknown_actor_returns_failed_receipt() {
    let f = AgentsSendFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_SEND);

    let result = f
        .call(json!({"to": "Missing", "text": "hello"}), &ctx)
        .await
        .unwrap();

    assert_eq!(result, json!({"status": "failed", "reason": "unreachable"}));
}

#[tokio::test]
async fn agents_send_await_reports_full_inbox_without_waiting() {
    let roster = make_roster();
    track_running(&roster, "Worker").await;

    for index in 0..64 {
        let receipt = roster
            .send_message(ActorMessage {
                from: ActorId::new("Root").unwrap(),
                to: ActorId::new("Worker").unwrap(),
                text: format!("queued {index}"),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;
        assert_eq!(receipt, Receipt::Delivered);
    }

    let f = AgentsSendFn::new(Arc::clone(&roster));
    let ctx = ctx_with(caps::AGENTS_SEND);
    let result = f
        .call(
            json!({"to": "Worker", "text": "overflow", "opts": {"await": true, "timeoutMs": 1000}}),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(
        result,
        json!({"status": "failed", "reason": "backpressured"})
    );
}

#[tokio::test]
async fn agents_send_rejects_json_control_payload() {
    let f = AgentsSendFn::new(make_roster());
    let ctx = ctx_with(caps::AGENTS_SEND);

    let err = f
        .call(json!({"to": "Worker", "text": "{\"type\":\"done\"}"}), &ctx)
        .await
        .unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
}

#[tokio::test]
async fn agents_send_await_rejects_descendant_wait_edge() {
    let roster = make_roster();
    track_actor(&roster, "Parent", None, ActorStatus::Running).await;
    track_actor(&roster, "Child", Some("Parent"), ActorStatus::Running).await;
    let f = AgentsSendFn::new(Arc::clone(&roster));
    let ctx = ctx_with_actor(caps::AGENTS_SEND, "Parent");

    let err = f
        .call(
            json!({"to": "Child", "text": "status?", "opts": {"await": true, "timeoutMs": 1}}),
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(matches!(err, HostError::InvalidArgs(_)));
    assert!(roster.messages().await.is_empty());
}

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

// ─── P3.5: lifecycle event emission tests ────────────────────────────────

fn lifecycle_hook() -> (
    Arc<std::sync::Mutex<Vec<ActorLifecycleEvent>>>,
    impl Fn(String, ActorLifecycleEvent) + Send + Sync + 'static,
) {
    let captured: Arc<std::sync::Mutex<Vec<ActorLifecycleEvent>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured2 = Arc::clone(&captured);
    let hook = move |_session_id: String, event: ActorLifecycleEvent| {
        captured2.lock().unwrap().push(event);
    };
    (captured, hook)
}

struct EchoExec;
#[async_trait::async_trait]
impl crate::executor::ActorExecutor for EchoExec {
    async fn run_to_digest(
        &self,
        spec: crate::actor::ActorSpec,
    ) -> std::result::Result<crate::actor::ActorDigest, crate::executor::ActorError> {
        Ok(crate::actor::ActorDigest {
            actor_id: spec.id,
            summary: format!("echo: {}", spec.task),
            artifact_uri: None,
            history_uri: None,
            history_content: None,
        })
    }
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

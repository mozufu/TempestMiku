use super::super::*;
use super::support::*;
use tm_host::{CapabilityGrants, InvocationCtx};

fn delegation_context() -> InvocationCtx {
    InvocationCtx::new(CapabilityGrants::default().allow_many([
        "agents.*",
        "http.get",
        "resources.read:artifact",
    ]))
}

struct GrantCaptureExecutor(Arc<std::sync::Mutex<Vec<Vec<String>>>>);

#[async_trait::async_trait]
impl crate::executor::ActorExecutor for GrantCaptureExecutor {
    async fn run_to_digest(
        &self,
        spec: crate::actor::ActorSpec,
    ) -> std::result::Result<crate::actor::ActorDigest, crate::executor::ActorError> {
        self.0
            .lock()
            .unwrap()
            .push(spec.grants.names().map(str::to_string).collect::<Vec<_>>());
        Ok(crate::actor::ActorDigest {
            actor_id: spec.id,
            summary: "delegated".to_string(),
            artifact_uri: None,
            history_uri: None,
            history_content: None,
        })
    }
}

fn delegated_opts() -> Value {
    json!({"capabilities": ["http.get", "resources.read:artifact"]})
}

#[test]
fn child_grants_are_an_explicit_bounded_parent_subset() {
    let ctx = delegation_context();
    assert_eq!(child_agent_grants(&ctx, None).unwrap().names().count(), 0);
    let grants = child_agent_grants(&ctx, Some(&delegated_opts())).unwrap();
    assert_eq!(
        grants.names().collect::<Vec<_>>(),
        vec!["http.get", "resources.read:artifact"]
    );
    assert!(!grants.permits("agents.run"));

    let escalation =
        child_agent_grants(&ctx, Some(&json!({"capabilities": ["fs.read"]}))).unwrap_err();
    assert!(matches!(
        escalation,
        HostError::CapabilityDenied(message) if message.contains("not held by the parent")
    ));

    let control_plane = child_agent_grants(
        &InvocationCtx::new(CapabilityGrants::default().allow_many(["agents.*", "backend.coding"])),
        Some(&json!({"capabilities": ["backend.coding"]})),
    )
    .unwrap_err();
    assert!(matches!(
        control_plane,
        HostError::CapabilityDenied(message) if message.contains("not delegable")
    ));
    let mode_wildcard = child_agent_grants(
        &InvocationCtx::new(CapabilityGrants::default().allow("modes.*")),
        Some(&json!({"capabilities": ["modes.*"]})),
    )
    .unwrap_err();
    assert!(matches!(
        mode_wildcard,
        HostError::CapabilityDenied(message) if message.contains("not delegable")
    ));

    let too_many = (0..=MAX_CHILD_CAPABILITIES)
        .map(|_| Value::String("http.get".to_string()))
        .collect::<Vec<_>>();
    assert!(matches!(
        child_agent_grants(&ctx, Some(&json!({"capabilities": too_many}))),
        Err(HostError::InvalidArgs(_))
    ));

    let egress_parent = InvocationCtx::new(CapabilityGrants::default().allow_many([
        "agents.*",
        "http.get",
        "egress.destination:research",
        "secrets.use",
        "secrets.use:research_api",
    ]));
    let egress_child = child_agent_grants(
        &egress_parent,
        Some(&json!({"capabilities": [
            "http.get",
            "egress.destination:research",
            "secrets.use",
            "secrets.use:research_api"
        ]})),
    )
    .unwrap();
    assert_eq!(
        egress_child.names().collect::<Vec<_>>(),
        vec![
            "egress.destination:research",
            "http.get",
            "secrets.use",
            "secrets.use:research_api",
        ]
    );
    assert!(!egress_child.permits("egress.destination:other"));
    assert!(matches!(
        child_agent_grants(
            &egress_parent,
            Some(&json!({"capabilities": ["egress.destination:other"]}))
        ),
        Err(HostError::CapabilityDenied(message)) if message.contains("not held by the parent")
    ));
}

#[tokio::test]
async fn delegation_opts_reach_run_spawn_parallel_and_pipeline() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let roster = make_roster();
    roster.set_executor(Arc::new(GrantCaptureExecutor(Arc::clone(&captured))));
    let ctx = delegation_context();

    AgentsRunFn::new(Arc::clone(&roster))
        .call(
            json!({"role": "runner", "task": "run", "opts": delegated_opts()}),
            &ctx,
        )
        .await
        .unwrap();
    AgentsSpawnFn::new(Arc::clone(&roster))
        .call(
            json!({"role": "spawner", "task": "spawn", "opts": delegated_opts()}),
            &ctx,
        )
        .await
        .unwrap();
    AgentsParallelFn::new(Arc::clone(&roster))
        .call(
            json!({
                "tasks": [{"role": "parallel", "task": "parallel"}],
                "opts": delegated_opts()
            }),
            &ctx,
        )
        .await
        .unwrap();
    AgentsPipelineFn::new(roster)
        .call(
            json!({
                "items": ["input"],
                "stages": [{"role": "pipeline", "task": "pipeline"}],
                "opts": delegated_opts()
            }),
            &ctx,
        )
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if captured.lock().unwrap().len() == 4 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("all four child constructors should reach the executor");

    for grants in captured.lock().unwrap().iter() {
        assert_eq!(
            grants,
            &[
                "http.get".to_string(),
                "resources.read:artifact".to_string()
            ]
        );
    }
}

#[tokio::test]
async fn every_child_constructor_rejects_capability_escalation_before_spawn() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let roster = make_roster();
    roster.set_executor(Arc::new(GrantCaptureExecutor(Arc::clone(&captured))));
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("agents.*"));
    let opts = json!({"capabilities": ["fs.read"]});

    let run = AgentsRunFn::new(Arc::clone(&roster))
        .call(
            json!({"role": "runner", "task": "run", "opts": opts.clone()}),
            &ctx,
        )
        .await;
    let spawn = AgentsSpawnFn::new(Arc::clone(&roster))
        .call(
            json!({"role": "spawner", "task": "spawn", "opts": opts.clone()}),
            &ctx,
        )
        .await;
    let parallel = AgentsParallelFn::new(Arc::clone(&roster))
        .call(
            json!({
                "tasks": [{"role": "parallel", "task": "parallel"}],
                "opts": opts.clone()
            }),
            &ctx,
        )
        .await;
    let pipeline = AgentsPipelineFn::new(roster)
        .call(
            json!({
                "items": ["input"],
                "stages": [{"role": "pipeline", "task": "pipeline"}],
                "opts": opts
            }),
            &ctx,
        )
        .await;

    for result in [run, spawn, parallel, pipeline] {
        assert!(matches!(result, Err(HostError::CapabilityDenied(_))));
    }
    assert!(captured.lock().unwrap().is_empty());
}

#[test]
fn actor_diagnostics_are_redacted_before_logging_or_returning() {
    let diagnostic = redact_actor_diagnostic(
        "actor failed with Authorization: Bearer opaque-token-123456 and sk-testsecret123456",
    );
    assert!(!diagnostic.contains("opaque-token-123456"));
    assert!(!diagnostic.contains("sk-testsecret123456"));
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
async fn actor_fanout_and_aggregate_text_limits_reject_before_spawning() {
    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(EchoExec));
    let parallel = AgentsParallelFn::new(Arc::clone(&roster));
    let parallel_ctx = ctx_with(caps::AGENTS_PARALLEL);

    let too_many = (0..=MAX_ACTORS_PER_CALL)
        .map(|index| json!({"role":"worker","task":format!("task {index}")}))
        .collect::<Vec<_>>();
    assert!(matches!(
        parallel
            .call(json!({"tasks":too_many}), &parallel_ctx)
            .await,
        Err(HostError::InvalidArgs(_))
    ));
    let aggregate = (0..MAX_ACTORS_PER_CALL)
        .map(|_| json!({"role":"worker","task":"x".repeat(4_096)}))
        .collect::<Vec<_>>();
    assert!(matches!(
        parallel
            .call(json!({"tasks":aggregate}), &parallel_ctx)
            .await,
        Err(HostError::InvalidArgs(_))
    ));
    assert!(roster.list_for_session("").await.is_empty());

    let pipeline = AgentsPipelineFn::new(roster);
    let pipeline_ctx = ctx_with(caps::AGENTS_PIPELINE);
    assert!(matches!(
        pipeline
            .call(
                json!({
                    "items": (0..=MAX_ACTORS_PER_CALL).collect::<Vec<_>>(),
                    "stages": [{"role":"worker","task":"work"}]
                }),
                &pipeline_ctx,
            )
            .await,
        Err(HostError::InvalidArgs(_))
    ));

    let send = AgentsSendFn::new(make_roster());
    let send_ctx = ctx_with(caps::AGENTS_SEND);
    assert!(matches!(
        send.call(
            json!({
                "to":"Worker",
                "text":"x".repeat(crate::actor::MAX_ACTOR_MESSAGE_BYTES + 1)
            }),
            &send_ctx,
        )
        .await,
        Err(HostError::InvalidArgs(_))
    ));
}

#[tokio::test]
async fn returned_history_uri_resolves_only_in_the_owning_session() {
    use tm_host::ResourceHandler;

    struct HistoryExecutor;
    #[async_trait::async_trait]
    impl crate::executor::ActorExecutor for HistoryExecutor {
        async fn run_to_digest(
            &self,
            spec: crate::actor::ActorSpec,
        ) -> std::result::Result<crate::actor::ActorDigest, crate::executor::ActorError> {
            Ok(crate::actor::ActorDigest {
                history_uri: Some(format!("history://{}", spec.id)),
                history_content: Some("owned history".to_string()),
                actor_id: spec.id,
                summary: "done".to_string(),
                artifact_uri: None,
            })
        }
    }

    let roster = Arc::new(MailboxRegistry::new());
    roster.set_executor(Arc::new(HistoryExecutor));
    let run = AgentsRunFn::new(Arc::clone(&roster));
    let owner_ctx = ctx_with(caps::AGENTS_RUN).with_session_id("session-a");
    let result = run
        .call(
            json!({"role":"worker","task":"capture history"}),
            &owner_ctx,
        )
        .await
        .unwrap();
    let uri = result["historyUri"].as_str().unwrap();
    let histories = crate::resources::HistoryResourceHandler::new(roster);

    let owned = histories.read(uri, None, &owner_ctx).await.unwrap();
    assert_eq!(owned.content, "owned history");
    let other_ctx = ctx_with(caps::AGENTS_RUN).with_session_id("session-b");
    assert!(matches!(
        histories.read(uri, None, &other_ctx).await,
        Err(HostError::NotFound(_))
    ));
}

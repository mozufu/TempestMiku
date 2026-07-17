use super::super::*;
use super::support::*;

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

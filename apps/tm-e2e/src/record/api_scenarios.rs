use super::*;

pub(super) async fn run_public_api_scenario(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    live_api: bool,
) -> Result<()> {
    let started_at = timestamp();
    let result = async {
        let report = if live_api {
            let speaker = LiveSpeaker::from_env()?;
            run_workflow(
                client,
                &speaker,
                WorkflowOptions {
                    require_artifact: false,
                },
            )
            .await?
        } else {
            let speaker = ScriptedSpeaker::default();
            run_workflow(
                client,
                &speaker,
                WorkflowOptions {
                    require_artifact: true,
                },
            )
            .await?
        };
        let record_path = recorder.root().join("api-public-workflow-record.json");
        write_workflow_record(
            &record_path,
            if live_api { "live-api" } else { "api-public" },
            &report,
        )?;
        recorder.add_artifact("api-public workflow record", &record_path)?;
        recorder.append_transcript(format!("- Personal final: `{}`", report.personal_final));
        recorder.append_transcript(format!("- Coding final: `{}`", report.coding_final));
        capture_resource(
            recorder,
            client,
            &report.session_id,
            &report.memory_record_uri,
        )
        .await?;
        if let Some(uri) = &report.artifact_uri {
            capture_resource(recorder, client, &report.session_id, uri).await?;
        }
        capture_resource(
            recorder,
            client,
            &report.session_id,
            "project://tempestmiku",
        )
        .await?;
        Ok::<Value, anyhow::Error>(json!({
            "sessionId": report.session_id,
            "rounds": report.rounds.len(),
            "memoryRecordUri": report.memory_record_uri,
            "artifactUri": report.artifact_uri,
            "promotedCount": report.promoted_count
        }))
    }
    .await;
    record_scenario_result(recorder, "api-public", started_at, &result);
    result.map(|_| ())
}

pub(super) async fn run_actor_api_scenario(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
) -> Result<()> {
    let started_at = timestamp();
    let result = async {
        let report = run_actor_smoke(client).await?;
        capture_resource(recorder, client, &report.session_id, &report.artifact_uri).await?;
        capture_resource(recorder, client, &report.session_id, &report.history_uri).await?;
        capture_resource(recorder, client, &report.session_id, &report.agent_uri).await?;
        capture_resource(
            recorder,
            client,
            &report.session_id,
            &report.cancelled_agent_uri,
        )
        .await?;
        recorder.append_transcript(format!(
            "- Actor smoke resources: actor `{}`, approval `{}`, artifact `{}`, history `{}`, cancelled `{}`.",
            report.actor_id,
            report.approval_id,
            report.artifact_uri,
            report.history_uri,
            report.cancelled_agent_uri
        ));
        recorder.append_transcript(format!(
            "- Actor replay event types: `{}`",
            report.replayed_event_types.join("`, `")
        ));
        Ok::<Value, anyhow::Error>(json!({
            "sessionId": report.session_id,
            "actorId": report.actor_id,
            "agentUri": report.agent_uri,
            "approvalId": report.approval_id,
            "artifactUri": report.artifact_uri,
            "historyUri": report.history_uri,
            "cancelledActorId": report.cancelled_actor_id,
            "cancelledAgentUri": report.cancelled_agent_uri,
            "replayedEventTypes": report.replayed_event_types
        }))
    }
    .await;
    record_scenario_result(recorder, "api-actor", started_at, &result);
    result.map(|_| ())
}

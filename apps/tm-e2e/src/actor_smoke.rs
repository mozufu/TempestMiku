use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::MikuClient;
use crate::workflow::max_event_id;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActorSmokeReport {
    pub session_id: String,
    pub actor_id: String,
    pub agent_uri: String,
    pub approval_id: String,
    pub artifact_uri: String,
    pub history_uri: String,
    pub cancelled_actor_id: String,
    pub cancelled_agent_uri: String,
    pub replayed_event_types: Vec<String>,
}

pub async fn run_actor_smoke(client: &MikuClient) -> Result<ActorSmokeReport> {
    let session = client.create_session(Some("serious_engineer")).await?;
    ensure!(
        session.mode == "serious_engineer",
        "actor smoke should start in serious engineer mode, got {}",
        session.mode
    );

    let (created_events, _) = client
        .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
        .await?;
    let mut last_event_id = max_event_id(0, &created_events);

    let send_client = client.clone();
    let send_session_id = session.id.clone();
    let send_message = tokio::spawn(async move {
        send_client
            .send_message(
                &send_session_id,
                "handoff actor smoke: spawn a child, request approval, and return its artifact",
            )
            .await
    });

    let (approval_events, approval) = client
        .wait_for_event(&session.id, Some(last_event_id), |event| {
            event.event_type == "approval" && event.data["backend"] == json!("native-tm")
        })
        .await?;
    let replay_anchor = approval_events
        .iter()
        .find(|event| event.event_type == "actor_spawned")
        .and_then(|event| event.id)
        .unwrap_or_else(|| max_event_id(last_event_id, &approval_events))
        - 1;
    last_event_id = max_event_id(last_event_id, &approval_events);

    let approval_id = approval.data["approvalId"]
        .as_str()
        .context("child approval event did not include approvalId")?
        .to_string();
    let actor_id = approval.data["scope"]["actorId"]
        .as_str()
        .context("child approval event did not include scope.actorId")?
        .to_string();
    client
        .resolve_approval(&session.id, &approval_id, "approve")
        .await?;
    send_message
        .await
        .context("actor smoke send task panicked")?
        .context("actor smoke message request failed")?;

    let final_events = client
        .read_until_final(&session.id, Some(last_event_id))
        .await?;
    ensure!(
        final_events
            .iter()
            .any(|event| event.event_type == "approval_resolved"),
        "child approval resolution should stream before final"
    );
    let completed = final_events
        .iter()
        .find(|event| event.event_type == "actor_completed")
        .context("actor_completed event was not observed")?;
    let completed_actor = completed.data["actor_id"]
        .as_str()
        .or_else(|| completed.data["actorId"].as_str())
        .unwrap_or_default();
    ensure!(
        completed_actor == actor_id,
        "actor_completed should match approval actor id"
    );
    let artifact_uri = completed.data["artifact_uri"]
        .as_str()
        .or_else(|| completed.data["artifactUri"].as_str())
        .context("actor_completed did not include artifactUri")?
        .to_string();
    let history_uri = completed.data["history_uri"]
        .as_str()
        .or_else(|| completed.data["historyUri"].as_str())
        .context("actor_completed did not include historyUri")?
        .to_string();
    ensure!(
        final_events.iter().any(|event| {
            event.event_type == "actor_resources_linked"
                && event.data["actor_id"]
                    .as_str()
                    .or_else(|| event.data["actorId"].as_str())
                    == Some(actor_id.as_str())
                && event.data["artifact_uri"]
                    .as_str()
                    .or_else(|| event.data["artifactUri"].as_str())
                    == Some(artifact_uri.as_str())
                && event.data["history_uri"]
                    .as_str()
                    .or_else(|| event.data["historyUri"].as_str())
                    == Some(history_uri.as_str())
        }),
        "actor_resources_linked should surface child artifact/history provenance"
    );
    let cancelled = final_events
        .iter()
        .find(|event| event.event_type == "actor_cancelled")
        .context("actor_cancelled event was not observed")?;
    let cancelled_actor_id = cancelled.data["actor_id"]
        .as_str()
        .or_else(|| cancelled.data["actorId"].as_str())
        .context("actor_cancelled did not include actor id")?
        .to_string();

    let artifact = client.resolve_resource(&session.id, &artifact_uri).await?;
    ensure!(
        !artifact["content"].as_str().unwrap_or_default().is_empty(),
        "child artifact {artifact_uri} should open through the session resource gateway"
    );
    let history = client.resolve_resource(&session.id, &history_uri).await?;
    ensure!(
        !history["content"].as_str().unwrap_or_default().is_empty(),
        "child history {history_uri} should open through the session resource gateway"
    );
    let agent_uri = format!("agent://{actor_id}");
    let agent = client.resolve_resource(&session.id, &agent_uri).await?;
    let agent_record = parse_agent_resource(&agent, &agent_uri)?;
    ensure!(
        agent_record["status"] == json!("terminated")
            && agent_record["artifact_uri"] == json!(artifact_uri)
            && agent_record["history_uri"] == json!(history_uri),
        "completed actor resource should expose terminal artifact/history record"
    );
    let cancelled_agent_uri = format!("agent://{cancelled_actor_id}");
    let cancelled_agent = client
        .resolve_resource(&session.id, &cancelled_agent_uri)
        .await?;
    let cancelled_record = parse_agent_resource(&cancelled_agent, &cancelled_agent_uri)?;
    ensure!(
        cancelled_record["status"] == json!("terminated")
            && cancelled_record["cancelled"] == json!(true)
            && cancelled_record["failure_reason"]["kind"] == json!("cancelled"),
        "cancelled actor resource should expose terminal cancelled record"
    );

    let replayed = client
        .read_until_final(&session.id, Some(replay_anchor))
        .await?;
    let replayed_event_types = replayed
        .iter()
        .map(|event| event.event_type.clone())
        .collect::<Vec<_>>();
    ensure!(
        replayed_event_types.iter().any(|kind| kind == "approval")
            && replayed_event_types
                .iter()
                .any(|kind| kind == "approval_resolved")
            && replayed_event_types
                .iter()
                .any(|kind| kind == "actor_completed")
            && replayed_event_types
                .iter()
                .any(|kind| kind == "actor_resources_linked")
            && replayed_event_types
                .iter()
                .any(|kind| kind == "actor_cancelled"),
        "Last-Event-ID replay should include actor approval, output-link, and cancellation events"
    );

    Ok(ActorSmokeReport {
        session_id: session.id,
        actor_id,
        agent_uri,
        approval_id,
        artifact_uri,
        history_uri,
        cancelled_actor_id,
        cancelled_agent_uri,
        replayed_event_types,
    })
}

fn parse_agent_resource(value: &serde_json::Value, uri: &str) -> Result<serde_json::Value> {
    let content = value["content"]
        .as_str()
        .with_context(|| format!("{uri} resource did not include string content"))?;
    serde_json::from_str(content).with_context(|| format!("decoding {uri} actor record"))
}

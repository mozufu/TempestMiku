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
    pub approval_id: String,
    pub artifact_uri: String,
    pub replayed_event_types: Vec<String>,
}

pub async fn run_actor_smoke(client: &MikuClient) -> Result<ActorSmokeReport> {
    let session = client.create_session(Some("handoff")).await?;
    ensure!(
        session.mode == "handoff",
        "actor smoke should start in handoff mode, got {}",
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
            event.event_type == "approval" && event.data["backend"] == json!("native-deno")
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

    let artifact = client.resolve_resource(&session.id, &artifact_uri).await?;
    ensure!(
        !artifact["content"].as_str().unwrap_or_default().is_empty(),
        "child artifact {artifact_uri} should open through the session resource gateway"
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
                .any(|kind| kind == "actor_completed"),
        "Last-Event-ID replay should include actor approval and completion events"
    );

    Ok(ActorSmokeReport {
        session_id: session.id,
        actor_id,
        approval_id,
        artifact_uri,
        replayed_event_types,
    })
}

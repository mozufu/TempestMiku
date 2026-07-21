use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::workflow::max_event_id;
use crate::{E2eEvent, MikuClient};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveSmokeReport {
    pub session_id: String,
    pub approval_id: String,
    pub filed_uri: String,
    pub replayed_event_types: Vec<String>,
}

pub async fn run_drive_smoke(client: &MikuClient) -> Result<DriveSmokeReport> {
    let session = client
        .create_session_scoped(Some("serious_engineer"), Some("project:tempestmiku"))
        .await?;
    ensure!(
        session.mode == "serious_engineer",
        "drive smoke should start in Serious Engineer mode, got {}",
        session.mode
    );

    let (created_events, _) = client
        .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
        .await?;
    let mut last_event_id = max_event_id(0, &created_events);

    let send_client = client.clone();
    let send_session_id = session.id.clone();
    let send = tokio::spawn(async move {
        send_client
            .send_message(
                &send_session_id,
                "drive smoke: file the dropped document after approval, then search and research it",
            )
            .await
    });

    let (approval_events, approval) = client
        .wait_for_event(&session.id, Some(last_event_id), |event| {
            event.event_type == "approval"
                && event.data["backend"] == json!("native-tm")
                && event.data["action"]
                    .as_str()
                    .is_some_and(|action| action.starts_with("drive.put "))
        })
        .await?;
    let replay_anchor = approval_events
        .iter()
        .find(|event| event.event_type == "approval")
        .and_then(|event| event.id)
        .unwrap_or_else(|| max_event_id(last_event_id, &approval_events))
        - 1;
    last_event_id = max_event_id(last_event_id, &approval_events);

    let approval_id = approval.data["approvalId"]
        .as_str()
        .context("drive approval event did not include approvalId")?
        .to_string();
    let transcript = client.session_messages(&session.id).await?;
    let pending = transcript["pendingEvents"]
        .as_array()
        .context("transcript did not include pendingEvents")?;
    ensure!(
        pending.iter().any(|event| {
            event["type"] == json!("approval")
                && event["data"]["approvalId"] == json!(approval_id)
                && event["data"]["action"]
                    .as_str()
                    .is_some_and(|action| action.starts_with("drive.put "))
        }),
        "drive approval should be visible in transcript pendingEvents before it is resolved"
    );

    client
        .resolve_approval(&session.id, &approval_id, "approve")
        .await?;
    send.await
        .context("drive smoke message task panicked")?
        .context("drive smoke message request failed")?;

    let final_events = client
        .read_until_final(&session.id, Some(last_event_id))
        .await?;
    ensure!(
        final_events
            .iter()
            .any(|event| event.event_type == "approval_resolved"),
        "approved drive put should emit approval_resolved"
    );
    let filed = final_events
        .iter()
        .find(|event| event.event_type == "drive_put")
        .context("drive_put event was not observed")?;
    let filed_uri = filed.data["uri"]
        .as_str()
        .context("drive_put did not include uri")?
        .to_string();
    ensure!(
        filed.data["sourceUri"] == json!("drop://browser/approval-drop.md"),
        "drive_put should preserve drop source provenance"
    );
    ensure!(
        has_redacted_research_trace(&final_events),
        "native turn should prove search/get execution without leaking results; events: {final_events:#?}"
    );

    let preview = client.preview_resource(&session.id, &filed_uri).await?;
    ensure!(
        preview["preview"]
            .as_str()
            .unwrap_or_default()
            .contains("Approval Drop"),
        "drive preview should include dropped document title"
    );
    let resolved = client.resolve_resource(&session.id, &filed_uri).await?;
    ensure!(
        resolved["content"]
            .as_str()
            .unwrap_or_default()
            .contains("Research smoke citation body"),
        "resolved drive resource should open filed content"
    );
    let feed = client
        .drive_feed(&session.id, Some("tempestmiku"), 5)
        .await?;
    ensure!(
        feed["recent"]
            .as_array()
            .is_some_and(|recent| recent.iter().any(|entry| entry["uri"] == json!(filed_uri))),
        "drive feed should include the filed document"
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
            && replayed_event_types.iter().any(|kind| kind == "drive_put")
            && replayed_event_types.iter().any(|kind| kind == "final"),
        "Last-Event-ID replay should include drive approval, filing, and final events"
    );

    Ok(DriveSmokeReport {
        session_id: session.id,
        approval_id,
        filed_uri,
        replayed_event_types,
    })
}

fn has_redacted_research_trace(events: &[E2eEvent]) -> bool {
    let started = |capability: &str| {
        events.iter().any(|event| {
            event.event_type == "effect_start"
                && event.data["capability"] == json!(capability)
                && event.data["argsPreview"] == json!("[redacted]")
        })
    };
    started("drive.search")
        && started("drive.get")
        && events.iter().any(|event| {
            event.event_type == "display"
                && event.data["value"] == json!("[redacted]")
                && event.data["spec"] == json!("[redacted]")
        })
}

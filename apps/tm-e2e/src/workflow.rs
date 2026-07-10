use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{E2eEvent, E2eSpeaker, MikuClient, WorkflowContext, WorkflowStep};

pub const WORKFLOW_RECORD_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default)]
pub struct WorkflowOptions {
    pub require_artifact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRound {
    pub index: usize,
    pub step: String,
    pub user_message: String,
    pub assistant_streamed_text: String,
    pub assistant_final_text: String,
    pub mode: String,
    pub event_id_start: Option<i64>,
    pub event_id_end: Option<i64>,
    pub event_types: Vec<String>,
    pub resource_uris: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowReport {
    pub session_id: String,
    pub personal_final: String,
    pub coding_final: String,
    pub memory_record_uri: String,
    pub artifact_uri: Option<String>,
    pub promoted_count: usize,
    pub rounds: Vec<ConversationRound>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRecord {
    pub schema_version: u32,
    pub mode: String,
    pub session_id: String,
    pub personal_final: String,
    pub coding_final: String,
    pub memory_record_uri: String,
    pub artifact_uri: Option<String>,
    pub promoted_count: usize,
    pub rounds: Vec<ConversationRound>,
}

impl WorkflowReport {
    pub fn to_record(&self, mode: impl Into<String>) -> WorkflowRecord {
        WorkflowRecord {
            schema_version: WORKFLOW_RECORD_SCHEMA_VERSION,
            mode: mode.into(),
            session_id: self.session_id.clone(),
            personal_final: self.personal_final.clone(),
            coding_final: self.coding_final.clone(),
            memory_record_uri: self.memory_record_uri.clone(),
            artifact_uri: self.artifact_uri.clone(),
            promoted_count: self.promoted_count,
            rounds: self.rounds.clone(),
        }
    }
}

pub fn write_workflow_record(
    path: impl AsRef<Path>,
    mode: &str,
    report: &WorkflowReport,
) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating tm-e2e record directory {}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(&report.to_record(mode))
        .context("encoding tm-e2e workflow record")?;
    fs::write(path, json).with_context(|| format!("writing tm-e2e record {}", path.display()))
}

pub async fn run_workflow(
    client: &MikuClient,
    speaker: &dyn E2eSpeaker,
    options: WorkflowOptions,
) -> Result<WorkflowReport> {
    let session = client.create_session(None).await?;
    ensure!(
        session.mode == "general",
        "new session should start as general, got {}",
        session.mode
    );
    ensure!(session.label == "General");
    ensure!(session.voice_cap == "medium");
    ensure!(
        session
            .active_skills
            .iter()
            .any(|skill| skill == "miku-voice")
    );

    let (created_events, created_mode) = client
        .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
        .await?;
    ensure!(created_mode.data["mode"] == json!("general"));
    let mut last_event_id = max_event_id(0, &created_events);

    let mut rounds = Vec::new();
    let context = WorkflowContext::default();
    let personal_step = WorkflowStep::PersonalAssistantGreeting;
    let personal_message = speaker.message(personal_step, &context).await?;
    client.send_message(&session.id, &personal_message).await?;
    let personal_events = client
        .read_until_final(&session.id, Some(last_event_id))
        .await?;
    ensure!(
        personal_events
            .iter()
            .any(|event| event.event_type == "text"),
        "personal assistant turn should stream text"
    );
    let personal_final = final_text(&personal_events)?;
    ensure!(!personal_final.trim().is_empty());
    rounds.push(conversation_round(
        1,
        personal_step,
        &personal_message,
        "general",
        &personal_events,
        &personal_final,
    )?);
    let replay_start = personal_events
        .iter()
        .find_map(|event| event.id)
        .unwrap_or(last_event_id + 1)
        - 1;
    let replayed_personal = client
        .read_until_final(&session.id, Some(replay_start))
        .await?;
    ensure!(
        replayed_personal
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>()
            == personal_events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
        "Last-Event-ID replay should return the missed personal turn events in order"
    );
    last_event_id = max_event_id(last_event_id, &personal_events);

    let context = WorkflowContext {
        personal_final: Some(personal_final.clone()),
    };
    let coding_step = WorkflowStep::CodingModeProbe;
    let coding_message = speaker.message(coding_step, &context).await?;
    client
        .set_session_scope(&session.id, "project:tempestmiku")
        .await
        .context("selecting the linked TempestMiku project scope")?;
    // Modes no longer auto-switch from message keywords (they're sticky capability envelopes
    // now); the workflow drives the same explicit override a client's mode picker would use.
    client
        .override_mode(&session.id, "serious_engineer", "coding mode probe")
        .await
        .context("switching to Serious Engineer via mode override")?;
    client.send_message(&session.id, &coding_message).await?;
    let coding_events = client
        .read_until_final(&session.id, Some(last_event_id))
        .await?;
    let mode = coding_events
        .iter()
        .find(|event| event.event_type == "mode" && event.data["mode"] == json!("serious_engineer"))
        .context("coding prompt did not route to Serious Engineer")?;
    let coding_mode = mode.data["mode"].as_str().unwrap_or("serious_engineer");
    ensure!(mode.data["voice_cap"] == json!("off"));
    ensure!(mode.data["activeSkills"] == json!(["serious-engineer-ops"]));
    let coding_final = final_text(&coding_events)?;
    ensure!(!coding_final.contains("喵"));
    rounds.push(conversation_round(
        2,
        coding_step,
        &coding_message,
        coding_mode,
        &coding_events,
        &coding_final,
    )?);
    let artifact_uri = coding_events
        .iter()
        .find(|event| event.event_type == "artifact")
        .and_then(|event| event.data["artifact"]["uri"].as_str())
        .map(str::to_string);
    if options.require_artifact {
        ensure!(
            artifact_uri.is_some(),
            "workflow required an artifact event from the coding backend"
        );
    }
    last_event_id = max_event_id(last_event_id, &coding_events);

    let memory_client = client.clone();
    let session_id = session.id.clone();
    let proposal = tokio::spawn(async move {
        memory_client
            .propose_profile_fact(
                &session_id,
                "prefers",
                "LLM-powered E2E hatch coverage",
                5_000,
            )
            .await
    });
    let (approval_events, approval) = client
        .wait_for_event(&session.id, Some(last_event_id), |event| {
            event.event_type == "approval" && event.data["backend"] == json!("memory")
        })
        .await?;
    last_event_id = max_event_id(last_event_id, &approval_events);
    let approval_id = approval.data["approvalId"]
        .as_str()
        .context("memory approval event did not include approvalId")?;
    client
        .resolve_approval(&session.id, approval_id, "approve")
        .await?;
    let proposal_response = proposal
        .await
        .context("memory proposal task panicked")?
        .context("memory proposal request failed")?;
    ensure!(proposal_response["status"] == json!("approved"));
    let memory_record_uri = proposal_response["record"]["uri"]
        .as_str()
        .context("approved memory proposal did not return a record uri")?
        .to_string();
    let (memory_events, _) = client
        .wait_for_event(&session.id, Some(last_event_id), |event| {
            event.event_type == "write_proposal" && event.data["status"] == json!("approved")
        })
        .await?;
    last_event_id = max_event_id(last_event_id, &memory_events);

    let memory_record = client
        .resolve_resource(&session.id, &memory_record_uri)
        .await?;
    ensure!(
        memory_record["content"]
            .as_str()
            .unwrap_or_default()
            .contains("LLM-powered E2E hatch coverage")
    );
    let memory_preview = client
        .preview_resource(&session.id, &memory_record_uri)
        .await?;
    ensure!(
        memory_preview["preview"]
            .as_str()
            .unwrap_or_default()
            .contains("LLM-powered E2E hatch coverage")
    );
    let schemes = client.list_resources(&session.id, None).await?;
    ensure!(
        schemes
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|entry| entry["uri"] == json!("memory://"))
    );

    if let Some(uri) = &artifact_uri {
        let artifact = client.resolve_resource(&session.id, uri).await?;
        ensure!(
            !artifact["content"].as_str().unwrap_or_default().is_empty(),
            "artifact resource {uri} should be readable"
        );
    }

    let mut resources = Vec::new();
    if let Some(uri) = &artifact_uri {
        resources.push(uri.clone());
    }
    let open_loops = vec!["keep the LLM-to-Miku E2E hatch covered".to_string()];
    let decisions = vec!["keep the hatch HTTP-only and approval-bound".to_string()];
    let first_promotion = client
        .promote_session(
            &session.id,
            "LLM-to-Miku E2E hatch is wired through public session APIs.",
            &open_loops,
            &decisions,
            &resources,
        )
        .await?;
    ensure!(first_promotion["projectUri"] == json!("project://tempestmiku"));
    let promoted = first_promotion["promoted"]
        .as_array()
        .context("promotion response did not include promoted items")?;
    ensure!(!promoted.is_empty());
    let second_promotion = client
        .promote_session(
            &session.id,
            "LLM-to-Miku E2E hatch is wired through public session APIs.",
            &open_loops,
            &decisions,
            &resources,
        )
        .await?;
    ensure!(
        first_promotion["promoted"][0]["id"] == second_promotion["promoted"][0]["id"],
        "promotion should be idempotent"
    );

    let project = client.project_overview(&session.id).await?;
    ensure!(project["projectUri"] == json!("project://tempestmiku"));
    ensure!(
        !project["openLoops"]
            .as_array()
            .unwrap_or(&Vec::new())
            .is_empty(),
        "project overview should include open loops"
    );
    ensure!(
        !project["decisions"]
            .as_array()
            .unwrap_or(&Vec::new())
            .is_empty(),
        "project overview should include decisions"
    );
    ensure!(
        !project["nextActions"]
            .as_array()
            .unwrap_or(&Vec::new())
            .is_empty(),
        "project overview should include next actions"
    );
    let project_views = client
        .list_resources(&session.id, Some("project://tempestmiku"))
        .await?;
    ensure!(
        project_views
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|entry| entry["uri"] == json!("project://tempestmiku/resources"))
    );

    // Keep the variable live so future workflow edits do not accidentally stop proving replay
    // after the memory path.
    let _ = last_event_id;

    Ok(WorkflowReport {
        session_id: session.id,
        personal_final,
        coding_final,
        memory_record_uri,
        artifact_uri,
        promoted_count: promoted.len(),
        rounds,
    })
}

pub(crate) fn conversation_round(
    index: usize,
    step: WorkflowStep,
    user_message: &str,
    fallback_mode: &str,
    events: &[E2eEvent],
    assistant_final_text: &str,
) -> Result<ConversationRound> {
    let assistant_streamed_text = events
        .iter()
        .filter(|event| event.event_type == "text")
        .filter_map(|event| event.data["delta"].as_str())
        .collect::<String>();
    let event_id_start = events.iter().filter_map(|event| event.id).min();
    let event_id_end = events.iter().filter_map(|event| event.id).max();
    let mode = events
        .iter()
        .rev()
        .find(|event| event.event_type == "mode")
        .and_then(|event| event.data["mode"].as_str())
        .unwrap_or(fallback_mode)
        .to_string();
    let mut event_types = Vec::new();
    for event in events {
        if !event_types.contains(&event.event_type) {
            event_types.push(event.event_type.clone());
        }
    }
    let mut resource_uris = extract_resource_uris(assistant_final_text);
    for event in events {
        let data = serde_json::to_string(&event.data)
            .context("encoding SSE event data while extracting resources")?;
        for uri in extract_resource_uris(&data) {
            if !resource_uris.contains(&uri) {
                resource_uris.push(uri);
            }
        }
    }
    Ok(ConversationRound {
        index,
        step: step.as_str().to_string(),
        user_message: user_message.to_string(),
        assistant_streamed_text,
        assistant_final_text: assistant_final_text.to_string(),
        mode,
        event_id_start,
        event_id_end,
        event_types,
        resource_uris,
    })
}

fn extract_resource_uris(text: &str) -> Vec<String> {
    const SCHEMES: &[&str] = &[
        "artifact://",
        "workspace://",
        "linked://",
        "project://",
        "memory://",
    ];

    let mut uris = Vec::new();
    for scheme in SCHEMES {
        let mut offset = 0;
        while let Some(relative_start) = text[offset..].find(scheme) {
            let start = offset + relative_start;
            let rest = &text[start..];
            let end = rest
                .char_indices()
                .find_map(|(idx, ch)| resource_uri_delimiter(ch).then_some(idx))
                .unwrap_or(rest.len());
            let uri = rest[..end].trim_end_matches(['.', '。', ',', ';', ':']);
            if !uri.is_empty() && !uris.iter().any(|seen| seen == uri) {
                uris.push(uri.to_string());
            }
            offset = start + end.max(scheme.len());
        }
    }
    uris
}

fn resource_uri_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | ')' | ']' | '}' | '{' | ',')
}

fn final_text(events: &[E2eEvent]) -> Result<String> {
    events
        .iter()
        .rev()
        .find(|event| event.event_type == "final")
        .and_then(|event| event.data["text"].as_str())
        .map(str::to_string)
        .context("final event did not include text")
}

pub(crate) fn max_event_id(current: i64, events: &[E2eEvent]) -> i64 {
    events
        .iter()
        .filter_map(|event| event.id)
        .max()
        .unwrap_or(current)
        .max(current)
}

use super::*;

pub(crate) async fn record_project_observations<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    project_id: String,
    user_content: &str,
    response: &str,
) -> Result<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let final_event_seq = latest_event_seq(state, session_id, "final").await?;
    let selected_at = Utc::now();
    let summary_text = response.trim().to_string();
    let summary_key = format!("summary:{session_id}:{final_event_seq:?}");
    let summary_uri = format!("project://{project_id}/summary/{session_id}");
    state
        .store
        .upsert_project_item(NewProjectItem {
            project_id: project_id.clone(),
            kind: ProjectItemKind::Summary,
            text: summary_text.clone(),
            target_uri: summary_uri,
            source_session_id: session_id,
            source_event_seq: final_event_seq,
            source_uri: None,
            dedupe_key: summary_key,
            provenance_json: json!({
                "sourceSession": session_id,
                "sourceEventId": final_event_seq,
                "selectedAt": selected_at,
                "actor": "router",
            }),
        })
        .await?;
    state
        .store
        .upsert_project_item(NewProjectItem {
            project_id: project_id.clone(),
            kind: ProjectItemKind::NextAction,
            text: next_action_text(response),
            target_uri: format!("project://{project_id}/next-actions/{session_id}"),
            source_session_id: session_id,
            source_event_seq: final_event_seq,
            source_uri: None,
            dedupe_key: format!("next-action:{session_id}:{final_event_seq:?}"),
            provenance_json: json!({
                "sourceSession": session_id,
                "sourceEventId": final_event_seq,
                "selectedAt": selected_at,
                "actor": "router",
            }),
        })
        .await?;
    let combined = format!("{user_content}\n{response}").to_lowercase();
    if combined.contains("open loop") || combined.contains("todo") || combined.contains("follow up")
    {
        state
            .store
            .upsert_project_item(NewProjectItem {
                project_id: project_id.clone(),
                kind: ProjectItemKind::OpenLoop,
                text: extract_line_with_markers(response, &["open loop", "todo", "follow up"])
                    .unwrap_or_else(|| summary_text.clone()),
                target_uri: format!("project://{project_id}/open-loops/{session_id}"),
                source_session_id: session_id,
                source_event_seq: final_event_seq,
                source_uri: None,
                dedupe_key: format!("open-loop:{session_id}:{final_event_seq:?}"),
                provenance_json: json!({
                    "sourceSession": session_id,
                    "sourceEventId": final_event_seq,
                    "selectedAt": selected_at,
                    "actor": "router",
                }),
            })
            .await?;
    }
    if combined.contains("decision") || combined.contains("decided") {
        state
            .store
            .upsert_project_item(NewProjectItem {
                project_id: project_id.clone(),
                kind: ProjectItemKind::Decision,
                text: extract_line_with_markers(response, &["decision", "decided"])
                    .unwrap_or(summary_text),
                target_uri: format!("project://{project_id}/decisions/{session_id}"),
                source_session_id: session_id,
                source_event_seq: final_event_seq,
                source_uri: None,
                dedupe_key: format!("decision:{session_id}:{final_event_seq:?}"),
                provenance_json: json!({
                    "sourceSession": session_id,
                    "sourceEventId": final_event_seq,
                    "selectedAt": selected_at,
                    "actor": "router",
                }),
            })
            .await?;
    }
    Ok(())
}

async fn latest_event_seq<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    event_type: &str,
) -> Result<Option<i64>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    Ok(state
        .store
        .events_after(session_id, None)
        .await?
        .into_iter()
        .rev()
        .find(|event| event.event_type == event_type)
        .map(|event| event.seq))
}

fn next_action_text(response: &str) -> String {
    let first = response
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(response)
        .trim();
    format!("Continue from latest session result: {first}")
}

fn extract_line_with_markers(text: &str, markers: &[&str]) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| {
            let lower = line.to_lowercase();
            markers.iter().any(|marker| lower.contains(marker))
        })
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

pub(crate) fn project_id_from_scope(scope: &str) -> String {
    scope
        .strip_prefix("project:")
        .filter(|id| !id.is_empty())
        .unwrap_or("tempestmiku")
        .to_string()
}


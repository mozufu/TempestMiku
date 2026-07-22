use std::{collections::BTreeMap, sync::Arc};

use serde_json::{Value, json};
use tm_memory::{
    DreamQueueRecord, EpisodeStatus, EvolutionEpisodeRecord, FeedbackOutcome,
    NewEvolutionEpisodeRecord, NewExperienceTraceRecord, RewardSource, TraceKind,
    backfill_trace_values, error_signature,
};
use uuid::Uuid;

use crate::{CodingEventSink, Result, ServerError, SessionEvent, Store};

use super::EvolutionDreamConfig;

pub(super) async fn capture_episodes<S: Store>(
    store: &Arc<S>,
    config: &EvolutionDreamConfig,
    dream: &DreamQueueRecord,
    events: &[SessionEvent],
    sink: &dyn CodingEventSink,
) -> Result<Vec<EvolutionEpisodeRecord>> {
    if !config.enabled {
        return Ok(Vec::new());
    }
    if let Err(error) = store
        .ensure_memory_scope_active(&dream.subject, &dream.scope)
        .await
    {
        if matches!(error, ServerError::NotFound(_)) {
            sink.emit(
                "dream_progress",
                json!({
                    "dreamId": dream.id,
                    "phase": "evolution_skipped",
                    "reason": "scope_revoked",
                }),
            )
            .await?;
            return Ok(Vec::new());
        }
        return Err(error);
    }

    let mut events_by_turn = BTreeMap::<Uuid, Vec<&SessionEvent>>::new();
    for event in events {
        if let Some(turn_id) = event.turn_id {
            events_by_turn.entry(turn_id).or_default().push(event);
        }
    }

    let mut episodes = Vec::new();
    let mut trace_count = 0usize;
    for (turn_id, mut turn_events) in events_by_turn {
        turn_events.sort_by_key(|event| event.seq);
        if !turn_events
            .iter()
            .any(|event| matches!(event.event_type.as_str(), "final" | "error"))
        {
            continue;
        }
        let (episode, inserted) = store
            .upsert_evolution_episode(NewEvolutionEpisodeRecord {
                session_id: dream.session_id,
                turn_id,
                owner_subject: dream.subject.clone(),
                memory_scope: dream.scope.clone(),
            })
            .await?;
        if !inserted && episode.trace_count > 0 {
            trace_count += episode.trace_count as usize;
            episodes.push(episode);
            continue;
        }

        let traces = extract_traces(&turn_events, episode.id, config);
        trace_count += traces.len();
        let persisted = store.replace_experience_traces(episode.id, traces).await?;
        let mut episode = episode;
        episode.trace_count = u32::try_from(persisted.len())
            .map_err(|_| ServerError::InvalidRequest("too many experience traces".to_string()))?;
        episodes.push(episode);
    }

    sink.emit(
        "dream_progress",
        json!({
            "dreamId": dream.id,
            "phase": "evolution_episodes_captured",
            "episodes": episodes.len(),
            "traces": trace_count,
        }),
    )
    .await?;
    Ok(episodes)
}

pub(super) async fn value_episodes<S: Store>(
    store: &Arc<S>,
    config: &EvolutionDreamConfig,
    dream: &DreamQueueRecord,
    episodes: &[EvolutionEpisodeRecord],
    events: &[SessionEvent],
    sink: &dyn CodingEventSink,
) -> Result<Vec<EvolutionEpisodeRecord>> {
    if !config.enabled {
        return Ok(Vec::new());
    }
    let mut pending = Vec::new();
    for episode in episodes {
        if episode.status != EpisodeStatus::Captured {
            continue;
        }
        let turn_events = events
            .iter()
            .filter(|event| event.turn_id == Some(episode.turn_id))
            .collect::<Vec<_>>();
        let feedback = store.turn_feedback(episode.turn_id).await?;
        let (reward, source, feedback_outcome) = match feedback {
            Some((outcome, _)) => (
                explicit_reward(outcome),
                RewardSource::Explicit,
                Some(outcome),
            ),
            None => (runtime_reward(&turn_events), RewardSource::Runtime, None),
        };
        let traces = store.experience_traces(episode.id).await?;
        let values = backfill_trace_values(reward, config.gamma, config.alpha, traces.len());
        let trace_values = traces
            .iter()
            .zip(values)
            .map(|(trace, value)| (trace.id, value))
            .collect::<Vec<_>>();
        pending.push((episode.id, reward, source, feedback_outcome, trace_values));
    }
    let mut valued = Vec::new();
    for (episode_id, reward, source, feedback_outcome, trace_values) in pending {
        valued.push(
            store
                .set_episode_valuation(
                    episode_id,
                    reward,
                    source,
                    feedback_outcome,
                    &trace_values,
                    EpisodeStatus::Valued,
                )
                .await?,
        );
    }
    sink.emit(
        "dream_progress",
        json!({
            "dreamId": dream.id,
            "phase": "evolution_valued",
            "episodes": valued.len(),
        }),
    )
    .await?;
    Ok(valued)
}

fn explicit_reward(outcome: FeedbackOutcome) -> f32 {
    match outcome {
        FeedbackOutcome::Accepted => 1.0,
        FeedbackOutcome::Corrected => -0.4,
        FeedbackOutcome::Rejected => -0.8,
    }
}

fn runtime_reward(events: &[&SessionEvent]) -> f32 {
    if events.iter().any(|event| event.event_type == "error") {
        return -0.6;
    }
    let failed_cell = events.iter().any(|event| {
        event.event_type == "cell_result"
            && string_field(&event.payload_json, "status") == Some("failed")
    });
    if failed_cell { -0.2 } else { 0.5 }
}

fn extract_traces(
    events: &[&SessionEvent],
    episode_id: Uuid,
    config: &EvolutionDreamConfig,
) -> Vec<NewExperienceTraceRecord> {
    let mut effect_results = BTreeMap::<&str, &SessionEvent>::new();
    let mut cell_results = BTreeMap::<&str, &SessionEvent>::new();
    for event in events {
        match event.event_type.as_str() {
            "effect_result" => {
                if let Some(node_id) = string_field(&event.payload_json, "nodeId") {
                    effect_results.entry(node_id).or_insert(event);
                }
            }
            "cell_result" => {
                if let Some(cell_id) = string_field(&event.payload_json, "cellId") {
                    cell_results.entry(cell_id).or_insert(event);
                }
            }
            _ => {}
        }
    }

    let mut traces = Vec::new();
    for event in events {
        let trace = match event.event_type.as_str() {
            "effect_start" => effect_trace(
                event,
                effect_results
                    .get(string_field(&event.payload_json, "nodeId").unwrap_or_default())
                    .copied(),
                episode_id,
                config,
            ),
            "cell_start" => cell_trace(
                event,
                cell_results
                    .get(string_field(&event.payload_json, "cellId").unwrap_or_default())
                    .copied(),
                episode_id,
                config,
            ),
            "final" | "error" => terminal_trace(event, episode_id, config),
            _ => None,
        };
        if let Some(trace) = trace {
            traces.push(trace);
        }
    }

    if traces.len() > config.max_traces_per_episode {
        traces.drain(..traces.len() - config.max_traces_per_episode);
    }
    for (ordinal, trace) in traces.iter_mut().enumerate() {
        trace.ordinal = ordinal as u32;
    }
    traces
}

fn effect_trace(
    start: &SessionEvent,
    result: Option<&SessionEvent>,
    episode_id: Uuid,
    config: &EvolutionDreamConfig,
) -> Option<NewExperienceTraceRecord> {
    let capability = string_field(&start.payload_json, "capability")?;
    let action = field_text(&start.payload_json, &["argsPreview"]);
    let (observation, error) = match result {
        Some(result) => {
            let observation =
                field_text(&result.payload_json, &["resultPreview", "error", "status"]);
            let error = string_field(&result.payload_json, "error").map(error_signature);
            (observation, error)
        }
        None => ("unresolved".to_string(), Some("unresolved".to_string())),
    };
    Some(NewExperienceTraceRecord {
        episode_id,
        ordinal: 0,
        kind: TraceKind::Effect,
        capability: Some(bound(capability, config.max_trace_field_chars)),
        action_summary: bound(&action, config.max_trace_field_chars),
        observation_summary: bound(&observation, config.max_trace_field_chars),
        error_signature: error.map(|value| bound(&value, config.max_trace_field_chars)),
        event_seq: start.seq,
        result_event_seq: result.map(|event| event.seq),
    })
}

fn cell_trace(
    start: &SessionEvent,
    result: Option<&SessionEvent>,
    episode_id: Uuid,
    config: &EvolutionDreamConfig,
) -> Option<NewExperienceTraceRecord> {
    string_field(&start.payload_json, "cellId")?;
    let preview = field_text(&start.payload_json, &["sourcePreview"]);
    let action = if preview.is_empty() {
        "cell".to_string()
    } else {
        format!("cell {preview}")
    };
    let (observation, error) = match result {
        Some(result) => {
            let status = string_field(&result.payload_json, "status").unwrap_or("unknown");
            let detail = field_text(&result.payload_json, &["resultPreview", "error"]);
            let observation = if detail.is_empty() {
                status.to_string()
            } else {
                format!("{status} {detail}")
            };
            let error = (status == "failed").then(|| error_signature(&detail));
            (observation, error)
        }
        None => ("unresolved".to_string(), Some("unresolved".to_string())),
    };
    Some(NewExperienceTraceRecord {
        episode_id,
        ordinal: 0,
        kind: TraceKind::Cell,
        capability: None,
        action_summary: bound(&action, config.max_trace_field_chars),
        observation_summary: bound(&observation, config.max_trace_field_chars),
        error_signature: error.map(|value| bound(&value, config.max_trace_field_chars)),
        event_seq: start.seq,
        result_event_seq: result.map(|event| event.seq),
    })
}

fn terminal_trace(
    event: &SessionEvent,
    episode_id: Uuid,
    config: &EvolutionDreamConfig,
) -> Option<NewExperienceTraceRecord> {
    let text = if event.event_type == "final" {
        field_text(&event.payload_json, &["text"])
    } else {
        field_text(&event.payload_json, &["message", "error", "text"])
    };
    let error = (event.event_type == "error").then(|| error_signature(&text));
    Some(NewExperienceTraceRecord {
        episode_id,
        ordinal: 0,
        kind: TraceKind::Terminal,
        capability: None,
        action_summary: "turn terminal".to_string(),
        observation_summary: bound(&text, config.max_trace_field_chars),
        error_signature: error.map(|value| bound(&value, config.max_trace_field_chars)),
        event_seq: event.seq,
        result_event_seq: None,
    })
}

fn field_text(value: &Value, fields: &[&str]) -> String {
    fields
        .iter()
        .find_map(|field| value.get(*field))
        .map(|value| match value {
            Value::String(value) => value.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

fn string_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field)?.as_str()
}

fn bound(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

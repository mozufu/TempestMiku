use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use serde_json::{Value, json};
use tm_memory::{
    DreamQueueRecord, EnvironmentCognitionRecord, EpisodeStatus, EvolutionEpisodeRecord,
    EvolutionPolicyRecord, ExperienceTraceRecord, FeedbackOutcome, MemoryEvidenceRef,
    NewEvolutionEpisodeRecord, NewExperienceTraceRecord, PolicyStatus, RewardSource, TraceKind,
    backfill_trace_values, error_signature, policy_gain,
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
        let current = store.evolution_episode(episode.id).await?;
        if current.status != EpisodeStatus::Captured {
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
        let skill_outcomes = selected_skill_outcomes(store, dream, episode.turn_id, reward).await?;
        pending.push((
            episode.id,
            reward,
            source,
            feedback_outcome,
            trace_values,
            skill_outcomes,
        ));
    }
    let mut valued = Vec::new();
    for (episode_id, reward, source, feedback_outcome, trace_values, skill_outcomes) in pending {
        valued.push(
            store
                .set_episode_valuation(
                    episode_id,
                    reward,
                    source,
                    feedback_outcome,
                    &trace_values,
                    &skill_outcomes,
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

async fn selected_skill_outcomes<S: Store>(
    store: &Arc<S>,
    dream: &DreamQueueRecord,
    turn_id: Uuid,
    reward: f32,
) -> Result<Vec<(String, String, bool)>> {
    let pass = if reward >= 0.5 {
        Some(true)
    } else if reward <= -0.3 {
        Some(false)
    } else {
        None
    };
    let Some(pass) = pass else {
        return Ok(Vec::new());
    };
    let Some(event) = store
        .event_for_turn(dream.session_id, turn_id, "skill_selected")
        .await?
    else {
        return Ok(Vec::new());
    };
    let Some(skills) = event.payload_json.get("skills").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    Ok(skills
        .iter()
        .filter_map(|skill| {
            Some((
                skill.get("name")?.as_str()?.to_string(),
                skill.get("digest")?.as_str()?.to_string(),
                pass,
            ))
        })
        .collect())
}

pub(super) async fn update_policies<S: Store>(
    store: &Arc<S>,
    config: &EvolutionDreamConfig,
    dream: &DreamQueueRecord,
    episodes: &[EvolutionEpisodeRecord],
    sink: &dyn CodingEventSink,
) -> Result<Vec<EvolutionPolicyRecord>> {
    if !config.enabled {
        return Ok(Vec::new());
    }

    let mut policies = store
        .evolution_policies(&dream.subject, &dream.scope, None, usize::MAX)
        .await?
        .into_iter()
        .map(|policy| (policy.signature.clone(), policy))
        .collect::<BTreeMap<_, _>>();
    let mut pooled = BTreeMap::<String, Vec<ExperienceTraceRecord>>::new();
    let mut pending_links = BTreeMap::<String, Vec<(Uuid, Uuid, f32, bool)>>::new();

    for episode in episodes {
        if episode.status != EpisodeStatus::Valued && episode.status != EpisodeStatus::Evolved {
            continue;
        }
        for trace in store.experience_traces(episode.id).await? {
            let Some(value) = trace.value else {
                continue;
            };
            if value < config.v_min && value > config.v_counter {
                continue;
            }
            let Some((signature, _)) = trace_signature(&trace) else {
                continue;
            };
            let positive = value >= config.v_min;
            if policies.contains_key(&signature) {
                pending_links.entry(signature).or_default().push((
                    trace.id,
                    trace.episode_id,
                    value,
                    positive,
                ));
            } else if positive {
                pooled.entry(signature).or_default().push(trace);
            }
        }
    }

    let mut induced = 0usize;
    for (signature, mut traces) in pooled {
        let distinct_episodes = traces
            .iter()
            .map(|trace| trace.episode_id)
            .collect::<BTreeSet<_>>();
        if distinct_episodes.len() < config.n_min as usize {
            continue;
        }
        traces.sort_by(|left, right| {
            right
                .value
                .unwrap_or_default()
                .total_cmp(&left.value.unwrap_or_default())
                .then_with(|| left.episode_id.cmp(&right.episode_id))
                .then_with(|| left.ordinal.cmp(&right.ordinal))
        });
        let (_, capability_prefix) = trace_signature(&traces[0])
            .expect("pooled traces always have an associable capability");
        let now = chrono::Utc::now();
        let policy = EvolutionPolicyRecord {
            id: deterministic_policy_id(&dream.subject, &dream.scope, &signature),
            owner_subject: dream.subject.clone(),
            memory_scope: dream.scope.clone(),
            signature: signature.clone(),
            trigger: format!(
                "Recurring {capability_prefix} work matching signature `{signature}` in {}",
                dream.scope
            ),
            procedure: traces
                .iter()
                .take(6)
                .map(|trace| format!("- {} → {}", trace.action_summary, trace.observation_summary))
                .collect::<Vec<_>>()
                .join("\n"),
            verification: format!(
                "The {capability_prefix} call completes without an error signature and the turn ends with a final response."
            ),
            boundary: format!(
                "Applies only to memory scope {} and the {capability_prefix}.* capability family.",
                dream.scope
            ),
            support_episode_ids: Vec::new(),
            gain: 0.0,
            status: PolicyStatus::Candidate,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        let policy = store.upsert_evolution_policy(policy).await?;
        pending_links.insert(
            signature.clone(),
            traces
                .into_iter()
                .map(|trace| {
                    (
                        trace.id,
                        trace.episode_id,
                        trace.value.expect("pooled traces are valued"),
                        true,
                    )
                })
                .collect(),
        );
        policies.insert(signature, policy);
        induced += 1;
    }

    let mut updated = Vec::new();
    for (signature, links) in pending_links {
        let mut policy = policies
            .remove(&signature)
            .expect("pending links always reference a known policy");
        store.link_policy_traces(policy.id, &links).await?;
        let linked = store.policy_trace_values(policy.id).await?;
        let linked_trace_ids = linked
            .iter()
            .map(|(trace_id, _, _, _)| *trace_id)
            .collect::<BTreeSet<_>>();
        let linked_episode_ids = linked
            .iter()
            .map(|(_, episode_id, _, _)| *episode_id)
            .collect::<BTreeSet<_>>();
        let with = linked
            .iter()
            .map(|(_, _, value, _)| *value)
            .collect::<Vec<_>>();
        let mut without = Vec::new();
        for episode_id in &linked_episode_ids {
            without.extend(
                store
                    .experience_traces(*episode_id)
                    .await?
                    .into_iter()
                    .filter_map(|trace| {
                        (!linked_trace_ids.contains(&trace.id))
                            .then_some(trace.value)
                            .flatten()
                    }),
            );
        }
        policy.support_episode_ids = linked
            .iter()
            .filter_map(|(_, episode_id, _, positive)| positive.then_some(*episode_id))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        policy.gain = policy_gain(&with, &without, config.tau_v, config.n0, config.baseline);
        policy.status = if policy.gain > 0.0 {
            PolicyStatus::Active
        } else if policy.gain >= config.archive_gain {
            PolicyStatus::Candidate
        } else {
            PolicyStatus::Archived
        };
        policy.updated_at = chrono::Utc::now();
        updated.push(store.upsert_evolution_policy(policy).await?);
    }

    sink.emit(
        "dream_progress",
        json!({
            "dreamId": dream.id,
            "phase": "evolution_policies_updated",
            "policies": updated.len(),
            "induced": induced,
        }),
    )
    .await?;
    Ok(updated)
}

pub(super) async fn crystallize_skill_proposals<S: Store>(
    store: &Arc<S>,
    config: &EvolutionDreamConfig,
    dream: &DreamQueueRecord,
) -> Result<Vec<(EvolutionPolicyRecord, Vec<MemoryEvidenceRef>)>> {
    if !config.enabled {
        return Ok(Vec::new());
    }
    let mut policies = store
        .evolution_policies(
            &dream.subject,
            &dream.scope,
            Some(PolicyStatus::Active),
            usize::MAX,
        )
        .await?;
    let episode_sessions = store
        .evolution_episodes(&dream.subject, &dream.scope, usize::MAX)
        .await?
        .into_iter()
        .map(|episode| (episode.id, episode.session_id))
        .collect::<BTreeMap<_, _>>();
    policies.retain(|policy| {
        policy.gain > config.gain_threshold
            && policy.support_episode_ids.len() >= config.n_min as usize
    });
    policies.sort_by(|left, right| {
        right
            .gain
            .total_cmp(&left.gain)
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut eligible = Vec::new();
    for policy in policies {
        let mut links = store
            .policy_trace_values(policy.id)
            .await?
            .into_iter()
            .filter(|(_, _, _, positive)| *positive)
            .collect::<Vec<_>>();
        links.sort_by(|left, right| {
            right
                .2
                .total_cmp(&left.2)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.0.cmp(&right.0))
        });
        let mut evidence = Vec::new();
        for (trace_id, episode_id, _, _) in links {
            if evidence.len() >= config.max_evidence_per_skill {
                break;
            }
            let event_seq = store
                .experience_traces(episode_id)
                .await?
                .into_iter()
                .find(|trace| trace.id == trace_id)
                .map(|trace| trace.event_seq);
            let Some(event_seq) = event_seq else {
                continue;
            };
            evidence.push(MemoryEvidenceRef {
                session_id: episode_sessions
                    .get(&episode_id)
                    .copied()
                    .unwrap_or(dream.session_id),
                event_seq: Some(event_seq),
                message_seq: None,
                uri: Some(format!("memory://evolution/episodes/{episode_id}")),
                label: format!("policy {} positive trace", policy.id),
            });
        }
        if !evidence.is_empty() {
            eligible.push((policy, evidence));
        }
    }
    Ok(eligible)
}

pub(super) async fn update_environment<S: Store>(
    store: &Arc<S>,
    config: &EvolutionDreamConfig,
    dream: &DreamQueueRecord,
    sink: &dyn CodingEventSink,
) -> Result<Option<EnvironmentCognitionRecord>> {
    if !config.enabled || !dream.scope.starts_with("project:") {
        return Ok(None);
    }

    let mut policies = store
        .evolution_policies(
            &dream.subject,
            &dream.scope,
            Some(PolicyStatus::Active),
            usize::MAX,
        )
        .await?;
    if policies.len() < config.l3_min_policies {
        return Ok(None);
    }
    policies.sort_by(|left, right| {
        left.trigger
            .cmp(&right.trigger)
            .then_with(|| left.id.cmp(&right.id))
    });

    let capability_families = policies
        .iter()
        .filter_map(|policy| policy.signature.split_once('|').map(|(family, _)| family))
        .filter(|family| !family.is_empty())
        .collect::<BTreeSet<_>>();
    let failure_families = policies
        .iter()
        .filter_map(|policy| policy.signature.split_once('|').map(|(_, family)| family))
        .filter(|family| !family.is_empty() && *family != "ok")
        .collect::<BTreeSet<_>>();
    let capability_families = capability_families
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    let failure_families = if failure_families.is_empty() {
        "none observed".to_string()
    } else {
        failure_families.into_iter().collect::<Vec<_>>().join(", ")
    };
    let mut lines = vec![
        format!("Capability families in active use: {capability_families}."),
        format!("Recurring failure families: {failure_families}."),
    ];
    lines.extend(
        policies
            .iter()
            .take(10)
            .map(|policy| format!("Established procedures exist for: {}.", policy.trigger)),
    );
    let now = chrono::Utc::now();
    let cognition = EnvironmentCognitionRecord {
        id: deterministic_cognition_id(&dream.subject, &dream.scope),
        owner_subject: dream.subject.clone(),
        memory_scope: dream.scope.clone(),
        title: format!("Environment cognition for {}", dream.scope),
        body: lines.join("\n"),
        source_policy_ids: policies.iter().map(|policy| policy.id).collect(),
        confidence: (policies.len() as f32 / 5.0).min(1.0),
        version: 1,
        created_at: now,
        updated_at: now,
    };
    let cognition = store.upsert_environment_cognition(cognition).await?;
    sink.emit(
        "dream_progress",
        json!({
            "dreamId": dream.id,
            "phase": "evolution_environment_updated",
            "policies": policies.len(),
        }),
    )
    .await?;
    Ok(Some(cognition))
}

fn deterministic_cognition_id(owner_subject: &str, memory_scope: &str) -> Uuid {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(format!(
        "tempestmiku:environment-cognition:{owner_subject}:{memory_scope}"
    ));
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn trace_signature(trace: &ExperienceTraceRecord) -> Option<(String, String)> {
    let capability = trace.capability.as_deref()?;
    let capability_prefix = capability.split('.').next()?.trim();
    if capability_prefix.is_empty() {
        return None;
    }
    let family = trace.error_signature.as_deref().unwrap_or("ok");
    Some((
        format!("{capability_prefix}|{family}"),
        capability_prefix.to_string(),
    ))
}

fn deterministic_policy_id(owner_subject: &str, memory_scope: &str, signature: &str) -> Uuid {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(format!(
        "tempestmiku:evolution-policy:{owner_subject}:{memory_scope}:{signature}"
    ));
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
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

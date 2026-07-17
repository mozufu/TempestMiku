use super::*;
use futures::{StreamExt, stream};

#[derive(Debug, Clone)]
struct PipelineStage {
    role: String,
    task: Option<String>,
    tasks: Option<Vec<String>>,
}

// ─── agents.pipeline ─────────────────────────────────────────────────────────

pub struct AgentsPipelineFn {
    docs: ToolDocs,
    roster: Arc<MailboxRegistry>,
}

impl AgentsPipelineFn {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self {
            roster,
            docs: ToolDocs {
                name: caps::AGENTS_PIPELINE.to_string(),
                namespace: "agents".to_string(),
                summary: "Run staged actor waves with a barrier between stages".to_string(),
                description: Some(
                    "Runs a staged map pipeline. Each stage fans out one actor per current \
                     item, waits for the full wave to finish, then feeds compact digest \
                     references into the next stage. Downstream tasks receive actor/resource \
                     handles plus a bounded summary, never a transcript re-inline. Stage specs \
                     provide a role plus either one task applied to each input or a tasks array \
                     matching the current wave length. Returns one ordered digest array per \
                     stage. Children receive no parent capabilities unless opts.capabilities \
                     explicitly delegates a held, delegable subset shared by every stage. \
                     Requires agents.pipeline grant."
                        .to_string(),
                ),
                signature:
                    "@agents.pipeline AgentPipelineArgs -> List (List AgentDigest)"
                        .to_string(),
                args_schema: json!({
                    "type": "object",
                    "required": ["items", "stages"],
                    "additionalProperties": false,
                    "properties": {
                        "items": { "type": "array", "description": "Initial JSON inputs for the first stage." },
                        "stages": {
                            "type": "array",
                            "minItems": 1,
                            "items": {
                                "type": "object",
                                "required": ["role"],
                                "properties": {
                                    "role": { "type": "string" },
                                    "task": { "type": "string", "description": "Task prompt applied to each current input." },
                                    "tasks": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Per-input task prompts for the current wave."
                                    }
                                }
                            }
                        },
                        "opts": {
                            "type": "object",
                            "additionalProperties": false,
                            "description": "Optional child authority delegation shared by every stage. Absent means no delegated capabilities; at most 32 names, each at most 128 ASCII bytes and 2 KiB total.",
                            "properties": {
                                "capabilities": {
                                    "type": "array",
                                    "maxItems": 32,
                                    "items": { "type": "string", "maxLength": 128 }
                                }
                            }
                        }
                    }
                }),
                result_schema: Some(json!({
                    "type": "array",
                    "items": {
                        "type": "array",
                        "items": { "type": "object" }
                    }
                })),
                examples: vec![ToolExample {
                    title: Some("Research then summarize".to_string()),
                    code: "let waves = @agents.pipeline {items: [\"§21\", \"§22\"], stages: [\n  {role: \"researcher\", task: \"Find the important points for this input.\"},\n  {role: \"writer\", task: \"Turn this digest into a concise summary.\"}\n], opts: {capabilities: [\"http.get\", \"resources.read:artifact\"]}};\nwaves |> display {kind: \"json\"}"
                        .to_string(),
                    notes: Some(
                        "The parent must hold every explicitly delegated capability. Each stage waits for all actors in the previous stage; downstream prompts receive actor/resource references plus bounded summaries, not transcripts."
                            .to_string(),
                    ),
                }],
                errors: vec![denied_error_doc(), timeout_error_doc()],
                grants: vec![agents_grant_doc()],
                sensitive: false,
                approval: "none".to_string(),
                since: "P3-plus".to_string(),
                stability: "experimental".to_string(),
            },
        }
    }
}

#[async_trait]
impl HostFn for AgentsPipelineFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_PIPELINE);
        let grants = child_agent_grants(ctx, args.get("opts"))?;

        let mut current_inputs = args["items"]
            .as_array()
            .ok_or_else(|| HostError::InvalidArgs("items must be an array".to_string()))?
            .clone();
        if current_inputs.len() > MAX_ACTORS_PER_CALL {
            return Err(HostError::InvalidArgs(format!(
                "items must contain at most {MAX_ACTORS_PER_CALL} entries"
            )));
        }
        let input_bytes = serde_json::to_string(&current_inputs)
            .map_err(|error| {
                HostError::InvalidArgs(format!("items are not serializable: {error}"))
            })?
            .len();
        if input_bytes > MAX_PIPELINE_INPUT_BYTES {
            return Err(HostError::InvalidArgs(format!(
                "items must total at most {MAX_PIPELINE_INPUT_BYTES} serialized bytes, got {input_bytes}"
            )));
        }
        let stages = parse_pipeline_stages(&args)?;
        let executor = self.roster.executor().ok_or_else(|| {
            HostError::NotImplemented("agents.pipeline — executor not configured".to_string())
        })?;

        let session_id = ctx.session_id.clone();
        let parent_id = caller_parent_id(ctx)?;
        let session_scope = ctx.session_scope.clone();
        let mut stage_outputs = Vec::with_capacity(stages.len());

        for (stage_index, stage) in stages.into_iter().enumerate() {
            let wave_tasks = pipeline_wave_tasks(stage_index, &stage, &current_inputs)?;
            let digests = run_pipeline_wave(
                Arc::clone(&self.roster),
                Arc::clone(&executor),
                session_id.clone(),
                parent_id.clone(),
                grants.clone(),
                session_scope.clone(),
                wave_tasks,
            )
            .await?;
            current_inputs = digests.clone();
            stage_outputs.push(Value::Array(digests));
        }

        Ok(Value::Array(stage_outputs))
    }
}

fn parse_pipeline_stages(args: &Value) -> Result<Vec<PipelineStage>> {
    let stages = args["stages"]
        .as_array()
        .ok_or_else(|| HostError::InvalidArgs("stages must be an array".to_string()))?;
    if stages.is_empty() {
        return Err(HostError::InvalidArgs(
            "stages must contain at least one stage".to_string(),
        ));
    }
    if stages.len() > MAX_PIPELINE_STAGES {
        return Err(HostError::InvalidArgs(format!(
            "stages must contain at most {MAX_PIPELINE_STAGES} entries"
        )));
    }

    let parsed = stages
        .iter()
        .enumerate()
        .map(|(index, stage)| {
            let role = parse_actor_role(
                &stage["role"],
                &format!("stages[{index}].role"),
            )?;
            let task = stage
                .get("task")
                .filter(|value| !value.is_null())
                .map(|value| {
                    parse_actor_task(value, &format!("stages[{index}].task"))
                })
                .transpose()?;
            let tasks = stage
                .get("tasks")
                .filter(|value| !value.is_null())
                .map(|value| {
                    let items = value.as_array().ok_or_else(|| {
                        HostError::InvalidArgs(format!("stages[{index}].tasks must be an array"))
                    })?;
                    if items.len() > MAX_ACTORS_PER_CALL {
                        return Err(HostError::InvalidArgs(format!(
                            "stages[{index}].tasks must contain at most {MAX_ACTORS_PER_CALL} entries"
                        )));
                    }
                    items
                        .iter()
                        .enumerate()
                        .map(|(task_index, item)| {
                            parse_actor_task(
                                item,
                                &format!("stages[{index}].tasks[{task_index}]"),
                            )
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?;

            if task.is_none() && tasks.is_none() {
                return Err(HostError::InvalidArgs(format!(
                    "stages[{index}] must include task or tasks"
                )));
            }

            Ok(PipelineStage { role, task, tasks })
        })
        .collect::<Result<Vec<_>>>()?;
    validate_aggregate_text_bytes(
        parsed.iter().flat_map(|stage| {
            std::iter::once(stage.role.as_str())
                .chain(stage.task.as_deref())
                .chain(stage.tasks.iter().flatten().map(String::as_str))
        }),
        "pipeline role/task text",
    )?;
    Ok(parsed)
}

fn pipeline_wave_tasks(
    stage_index: usize,
    stage: &PipelineStage,
    inputs: &[Value],
) -> Result<Vec<(String, String)>> {
    if let Some(tasks) = stage.tasks.as_ref() {
        if tasks.len() != inputs.len() {
            return Err(HostError::InvalidArgs(format!(
                "stages[{stage_index}].tasks length must match current item count"
            )));
        }
        return Ok(tasks
            .iter()
            .map(|task| (stage.role.clone(), task.clone()))
            .collect());
    }

    let task = stage
        .task
        .as_ref()
        .expect("parse_pipeline_stages requires task or tasks");
    inputs
        .iter()
        .map(|input| {
            let composed = format!(
                "{task}\n\nPipeline input: {}",
                compact_pipeline_input(input)
            );
            validate_bounded_text(
                &composed,
                &format!("stages[{stage_index}].task"),
                MAX_ACTOR_TASK_BYTES,
            )?;
            Ok((stage.role.clone(), composed))
        })
        .collect()
}

fn compact_pipeline_input(input: &Value) -> String {
    if let Some(actor_id) = input.get("actorId").and_then(Value::as_str) {
        return compact_pipeline_digest_reference(input, actor_id);
    }

    input
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| serde_json::to_string(input).unwrap_or_else(|_| "null".to_string()))
}

fn compact_pipeline_digest_reference(input: &Value, actor_id: &str) -> String {
    let summary = input.get("summary").and_then(Value::as_str).unwrap_or("");
    let artifact_uri = compact_optional_string(input.get("artifactUri"));
    let history_uri = compact_optional_string(input.get("historyUri"));
    format!(
        "digest actorId={actor_id}; agentUri=agent://{actor_id}; historyUri={history_uri}; \
         artifactUri={artifact_uri}; summary={summary}"
    )
}

fn compact_optional_string(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "null".to_string())
}

async fn run_pipeline_wave(
    roster: Arc<MailboxRegistry>,
    executor: Arc<dyn crate::executor::ActorExecutor>,
    session_id: String,
    parent_id: Option<ActorId>,
    grants: CapabilityGrants,
    session_scope: Option<String>,
    tasks: Vec<(String, String)>,
) -> Result<Vec<Value>> {
    if tasks.len() > MAX_ACTORS_PER_CALL {
        return Err(HostError::InvalidArgs(format!(
            "pipeline wave must contain at most {MAX_ACTORS_PER_CALL} actors"
        )));
    }
    let supervisor_id = roster.next_supervisor_id("PipelineWave");
    roster
        .set_supervision_policy_for_session(
            &session_id,
            supervisor_id.clone(),
            SupervisionPolicy {
                strategy: RestartStrategy::OneForAll,
                max_restarts: 0,
            },
        )
        .await;
    let planned = tasks
        .into_iter()
        .map(|(role, task)| {
            let actor_id = roster.next_actor_id(&role);
            (actor_id, role, task, ActorBudget::default(), Utc::now())
        })
        .collect::<Vec<_>>();
    roster
        .track_batch_for_session(
            &session_id,
            planned
                .iter()
                .map(|(actor_id, role, _, budget, spawned_at)| {
                    (
                        ActorRecord {
                            id: actor_id.clone(),
                            parent: parent_id.clone(),
                            status: ActorStatus::Running,
                            mode: Some(role.clone()),
                            budget: budget.clone(),
                            spawned_at: *spawned_at,
                            completed_at: None,
                            cancelled: false,
                            failure_reason: None,
                            last_summary: None,
                            artifact_uri: None,
                            history_uri: None,
                        },
                        supervisor_id.clone(),
                    )
                })
                .collect(),
        )
        .await
        .map_err(registry_error)?;

    let mut actor_specs = Vec::with_capacity(planned.len());
    for (actor_id, role, task, budget, spawned_at) in planned {
        roster.emit_lifecycle(
            &session_id,
            ActorLifecycleEvent::Spawned {
                actor_id: actor_id.clone(),
                parent_id: parent_id.clone(),
                role: role.clone(),
                task: task.clone(),
                depth: 0,
                budget: budget.clone(),
                spawned_at,
            },
        );
        let spec = ActorSpec {
            id: actor_id.clone(),
            session_id: session_id.clone(),
            session_scope: session_scope.clone(),
            role,
            task,
            mode: None,
            grants: grants.clone(),
            budget,
            parent: parent_id.clone(),
            depth: 0,
            cancellation: roster
                .cancel_token_for_session(&session_id, &actor_id)
                .await,
        };
        roster.remember_restart_spec(&spec).await;
        actor_specs.push((actor_id.clone(), spec));
    }

    let mut batch_cancel = CancelActorsOnDrop::new(
        actor_specs
            .iter()
            .map(|(_, spec)| spec.cancellation.clone())
            .collect(),
    );
    let results = stream::iter(actor_specs.into_iter().map(|(actor_id, spec)| {
        let executor = Arc::clone(&executor);
        let roster = Arc::clone(&roster);
        let session_id = session_id.clone();
        async move {
            let mut cancel_on_drop = CancelActorOnDrop::new(spec.cancellation.clone());
            let result = match run_actor_to_digest(executor, spec).await {
                Ok(digest) => {
                    let summary = digest.summary.clone();
                    let artifact_uri = digest.artifact_uri.clone();
                    let history_uri = digest.history_uri.clone();
                    if mark_actor_completed(&roster, &session_id, &actor_id, digest).await {
                        tracing::debug!(actor_id = %actor_id, "pipeline actor completed");
                    }
                    Ok(json!({
                        "actorId": actor_id.as_str(),
                        "summary": summary,
                        "artifactUri": artifact_uri,
                        "historyUri": history_uri,
                    }))
                }
                Err(err) => {
                    let reason = failure_reason_for_error(&err);
                    let error = redact_actor_diagnostic(&err);
                    tracing::warn!(actor_id = %actor_id, %error, "pipeline actor failed");
                    mark_actor_error(&roster, &session_id, &actor_id, reason).await;
                    Err(HostError::HostCall(format!(
                        "actor {actor_id} failed: {error}"
                    )))
                }
            };
            cancel_on_drop.disarm();
            result
        }
    }))
    .buffered(MAX_CONCURRENT_ACTORS)
    .collect::<Vec<_>>()
    .await;
    batch_cancel.disarm();

    let mut digests = Vec::with_capacity(results.len());
    for result in results {
        match result {
            Ok(digest) => digests.push(digest),
            Err(err) => return Err(err),
        }
    }

    Ok(digests)
}

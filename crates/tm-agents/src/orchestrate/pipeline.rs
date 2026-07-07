use super::*;

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
                     item, waits for the full wave to finish, then feeds only bounded digest \
                     JSON into the next stage. Stage specs provide a role plus either one \
                     task applied to each input or a tasks array matching the current wave \
                     length. Returns one ordered digest array per stage. Requires \
                     agents.pipeline grant."
                        .to_string(),
                ),
                signature:
                    "agents.pipeline(items: JsonValue[], ...stages: AgentPipelineStage[]): Promise<AgentDigest[][]>"
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
                    code: "const waves = await agents.pipeline(\n  ['§21', '§22'],\n  { role: 'researcher', task: 'Find the important points for this input.' },\n  { role: 'writer', task: 'Turn this digest into a concise summary.' },\n);\ndisplay(waves.at(-1).map(d => d.summary));"
                        .to_string(),
                    notes: Some(
                        "Each stage waits for all actors in the previous stage; downstream prompts receive digest JSON, not transcripts."
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

        let mut current_inputs = args["items"]
            .as_array()
            .ok_or_else(|| HostError::InvalidArgs("items must be an array".to_string()))?
            .clone();
        let stages = parse_pipeline_stages(&args)?;
        let executor = self.roster.executor().ok_or_else(|| {
            HostError::NotImplemented("agents.pipeline — executor not configured".to_string())
        })?;

        let session_id = ctx.session_id.clone();
        let parent_id = caller_parent_id(ctx)?;
        let grants = child_agent_grants(ctx);
        let mut stage_outputs = Vec::with_capacity(stages.len());

        for (stage_index, stage) in stages.into_iter().enumerate() {
            let wave_tasks = pipeline_wave_tasks(stage_index, &stage, &current_inputs)?;
            let digests = run_pipeline_wave(
                Arc::clone(&self.roster),
                Arc::clone(&executor),
                session_id.clone(),
                parent_id.clone(),
                grants.clone(),
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

    stages
        .iter()
        .enumerate()
        .map(|(index, stage)| {
            let role = stage["role"]
                .as_str()
                .ok_or_else(|| {
                    HostError::InvalidArgs(format!("stages[{index}].role must be a string"))
                })?
                .to_string();
            let task = stage
                .get("task")
                .filter(|value| !value.is_null())
                .map(|value| {
                    value.as_str().map(str::to_string).ok_or_else(|| {
                        HostError::InvalidArgs(format!("stages[{index}].task must be a string"))
                    })
                })
                .transpose()?;
            let tasks = stage
                .get("tasks")
                .filter(|value| !value.is_null())
                .map(|value| {
                    let items = value.as_array().ok_or_else(|| {
                        HostError::InvalidArgs(format!("stages[{index}].tasks must be an array"))
                    })?;
                    items
                        .iter()
                        .enumerate()
                        .map(|(task_index, item)| {
                            item.as_str().map(str::to_string).ok_or_else(|| {
                                HostError::InvalidArgs(format!(
                                    "stages[{index}].tasks[{task_index}] must be a string"
                                ))
                            })
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
        .collect()
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
    Ok(inputs
        .iter()
        .map(|input| {
            (
                stage.role.clone(),
                format!(
                    "{task}\n\nPipeline input: {}",
                    compact_pipeline_input(input)
                ),
            )
        })
        .collect())
}

fn compact_pipeline_input(input: &Value) -> String {
    input
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| serde_json::to_string(input).unwrap_or_else(|_| "null".to_string()))
}

async fn run_pipeline_wave(
    roster: Arc<MailboxRegistry>,
    executor: Arc<dyn crate::executor::ActorExecutor>,
    session_id: String,
    parent_id: Option<ActorId>,
    grants: CapabilityGrants,
    tasks: Vec<(String, String)>,
) -> Result<Vec<Value>> {
    let mut actor_specs = Vec::with_capacity(tasks.len());
    for (role, task) in tasks {
        let actor_id = roster.next_actor_id(&role);
        let budget = ActorBudget::default();
        let spawned_at = Utc::now();
        roster
            .track(ActorRecord {
                id: actor_id.clone(),
                parent: parent_id.clone(),
                status: ActorStatus::Running,
                mode: Some(role.clone()),
                budget: budget.clone(),
                spawned_at,
                completed_at: None,
                cancelled: false,
                failure_reason: None,
                last_summary: None,
                artifact_uri: None,
                history_uri: None,
            })
            .await;
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
        actor_specs.push((
            actor_id.clone(),
            ActorSpec {
                id: actor_id.clone(),
                session_id: session_id.clone(),
                role,
                task,
                mode: None,
                grants: grants.clone(),
                budget,
                parent: parent_id.clone(),
                depth: 0,
                cancellation: roster.cancel_token(&actor_id).await,
            },
        ));
    }

    let handles: Vec<tokio::task::JoinHandle<Result<Value>>> = actor_specs
        .into_iter()
        .map(|(actor_id, spec)| {
            let executor = Arc::clone(&executor);
            let roster = Arc::clone(&roster);
            let session_id = session_id.clone();
            tokio::spawn(async move {
                match executor.run_to_digest(spec).await {
                    Ok(digest) => {
                        if let Some(content) = digest.history_content.clone() {
                            roster.store_transcript(&actor_id, content).await;
                        }
                        let completed = roster
                            .mark_complete_with_digest(
                                &actor_id,
                                digest.summary.clone(),
                                digest.artifact_uri.clone(),
                                digest.history_uri.clone(),
                            )
                            .await;
                        if completed {
                            roster.emit_lifecycle(
                                &session_id,
                                ActorLifecycleEvent::Completed {
                                    actor_id: actor_id.clone(),
                                    completed_at: Utc::now(),
                                    summary: Some(digest.summary.clone()),
                                    artifact_uri: digest.artifact_uri.clone(),
                                    history_uri: digest.history_uri.clone(),
                                },
                            );
                            tracing::debug!(actor_id = %actor_id, "pipeline actor completed");
                        }
                        Ok(json!({
                            "actorId": digest.actor_id.as_str(),
                            "summary": digest.summary,
                            "artifactUri": digest.artifact_uri,
                            "historyUri": digest.history_uri,
                        }))
                    }
                    Err(err) => {
                        let reason = map_actor_error(&err);
                        tracing::warn!(actor_id = %actor_id, error = %err, "pipeline actor failed");
                        mark_actor_error(&roster, &session_id, &actor_id, reason).await;
                        Err(HostError::HostCall(format!(
                            "actor {actor_id} failed: {err}"
                        )))
                    }
                }
            })
        })
        .collect();

    let mut digests = Vec::with_capacity(handles.len());
    for handle in handles {
        let result = handle
            .await
            .map_err(|err| HostError::HostCall(format!("pipeline actor panicked: {err}")))?;
        match result {
            Ok(digest) => digests.push(digest),
            Err(err) => return Err(err),
        }
    }

    Ok(digests)
}

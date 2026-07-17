use super::*;
use futures::{StreamExt, stream};

// ─── agents.parallel ──────────────────────────────────────────────────────────

pub struct AgentsParallelFn {
    docs: ToolDocs,
    roster: Arc<MailboxRegistry>,
}

impl AgentsParallelFn {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self {
            roster,
            docs: ToolDocs {
                name: caps::AGENTS_PARALLEL.to_string(),
                namespace: "agents".to_string(),
                summary: "Fan out tasks across a bounded pool and return ordered digests".to_string(),
                description: Some(
                    "One-wave fan-out: spawns N actors concurrently (bounded pool), awaits all, \
                     and returns ordered digest results. Only digests return to the parent context. \
                     Children receive no parent capabilities unless opts.capabilities explicitly \
                     delegates a held, delegable subset. Requires agents.parallel grant."
                        .to_string(),
                ),
                signature: "@agents.parallel {tasks} -> List AgentDigest".to_string(),
                args_schema: json!({
                    "type": "object",
                    "required": ["tasks"],
                    "additionalProperties": false,
                    "properties": {
                        "tasks": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["role", "task"],
                                "properties": {
                                    "role": { "type": "string" },
                                    "task": { "type": "string" },
                                    "timeoutMs": { "type": "integer", "minimum": 0 },
                                    "budget": {
                                        "type": "object",
                                        "additionalProperties": false,
                                        "properties": {
                                            "wallMs": { "type": "integer", "minimum": 0 },
                                            "maxDepth": { "type": "integer", "minimum": 0 }
                                        }
                                    }
                                }
                            }
                        },
                        "opts": {
                            "type": "object",
                            "additionalProperties": false,
                            "description": "Optional child authority delegation shared by this wave. Absent means no delegated capabilities; at most 32 names, each at most 128 ASCII bytes and 2 KiB total.",
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
                result_schema: Some(json!({ "type": "array", "items": { "type": "object" } })),
                examples: vec![ToolExample {
                    title: Some("Research three topics in parallel".to_string()),
                    code: "let results = @agents.parallel {tasks: [\n  {role: \"researcher\", task: \"Research and summarize §21\"},\n  {role: \"researcher\", task: \"Research and summarize §22\"}\n], opts: {capabilities: [\"http.get\", \"resources.read:artifact\"]}};\nresults |> map (fun d -> d.summary) |> display {kind: \"json\"}"
                        .to_string(),
                    notes: Some(
                        "The parent must hold every explicitly delegated capability. Digests only — use d.historyUri for the full output of each actor."
                            .to_string(),
                    ),
                }],
                errors: vec![denied_error_doc(), timeout_error_doc()],
                grants: vec![agents_grant_doc()],
                sensitive: false,
                approval: "none".to_string(),
                since: "P3".to_string(),
                stability: "experimental".to_string(),
            },
        }
    }
}

#[async_trait]
impl HostFn for AgentsParallelFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_PARALLEL);
        let grants = child_agent_grants(ctx, args.get("opts"))?;

        let tasks = args["tasks"]
            .as_array()
            .ok_or_else(|| HostError::InvalidArgs("tasks must be an array".to_string()))?;

        if tasks.is_empty() {
            return Ok(json!([]));
        }
        if tasks.len() > MAX_ACTORS_PER_CALL {
            return Err(HostError::InvalidArgs(format!(
                "tasks must contain at most {MAX_ACTORS_PER_CALL} items"
            )));
        }

        // Validate all role/task strings before touching the executor or roster.
        let parsed: Vec<ParsedParallelTask> = tasks
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let role = parse_actor_role(&item["role"], &format!("tasks[{i}].role"))?;
                let task_str = parse_actor_task(&item["task"], &format!("tasks[{i}].task"))?;
                let budget = parse_parallel_budget(item, i)?;
                Ok(ParsedParallelTask {
                    role,
                    task: task_str,
                    budget,
                })
            })
            .collect::<Result<_>>()?;
        validate_aggregate_text_bytes(
            parsed
                .iter()
                .flat_map(|task| [task.role.as_str(), task.task.as_str()]),
            "tasks role/task text",
        )?;

        let executor = self.roster.executor().ok_or_else(|| {
            HostError::NotImplemented("agents.parallel — executor not configured".to_string())
        })?;

        let session_id = ctx.session_id.clone();
        let parent_id = caller_parent_id(ctx)?;
        let supervisor_id = self.roster.next_supervisor_id("ParallelGroup");
        self.roster
            .set_supervision_policy_for_session(
                &session_id,
                supervisor_id.clone(),
                SupervisionPolicy {
                    strategy: RestartStrategy::OneForAll,
                    max_restarts: 0,
                },
            )
            .await;

        let planned = parsed
            .into_iter()
            .map(|task| {
                let actor_id = self.roster.next_actor_id(&task.role);
                (actor_id, task, Utc::now())
            })
            .collect::<Vec<_>>();
        self.roster
            .track_batch_for_session(
                &session_id,
                planned
                    .iter()
                    .map(|(actor_id, task, spawned_at)| {
                        (
                            ActorRecord {
                                id: actor_id.clone(),
                                parent: parent_id.clone(),
                                status: ActorStatus::Running,
                                mode: Some(task.role.clone()),
                                budget: task.budget.clone(),
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

        // Emit lifecycle only after the entire wave has been reserved atomically.
        let mut actor_specs: Vec<(ActorId, ActorSpec)> = Vec::with_capacity(planned.len());
        for (actor_id, task, spawned_at) in planned {
            let ParsedParallelTask {
                role,
                task: task_str,
                budget,
            } = task;
            self.roster.emit_lifecycle(
                &session_id,
                ActorLifecycleEvent::Spawned {
                    actor_id: actor_id.clone(),
                    parent_id: parent_id.clone(),
                    role: role.clone(),
                    task: task_str.clone(),
                    depth: 0,
                    budget: budget.clone(),
                    spawned_at,
                },
            );
            let spec = ActorSpec {
                id: actor_id.clone(),
                session_id: session_id.clone(),
                session_scope: ctx.session_scope.clone(),
                role,
                task: task_str,
                mode: None,
                grants: grants.clone(),
                budget,
                parent: parent_id.clone(),
                depth: 0,
                cancellation: self
                    .roster
                    .cancel_token_for_session(&session_id, &actor_id)
                    .await,
            };
            self.roster.remember_restart_spec(&spec).await;
            actor_specs.push((actor_id.clone(), spec));
        }

        // Run a bounded, ordered stream. Dropping the host-call future drops every child
        // future and trips its cancellation token; no detached task survives an error/drop.
        let mut batch_cancel = CancelActorsOnDrop::new(
            actor_specs
                .iter()
                .map(|(_, spec)| spec.cancellation.clone())
                .collect(),
        );
        let results = stream::iter(actor_specs.into_iter().map(|(actor_id, spec)| {
            let executor = Arc::clone(&executor);
            let roster = Arc::clone(&self.roster);
            let session_id = session_id.clone();
            async move {
                let mut cancel_on_drop = CancelActorOnDrop::new(spec.cancellation.clone());
                let result = match run_actor_to_digest(executor, spec).await {
                    Ok(digest) => {
                        let summary = digest.summary.clone();
                        let artifact_uri = digest.artifact_uri.clone();
                        let history_uri = digest.history_uri.clone();
                        if mark_actor_completed(&roster, &session_id, &actor_id, digest).await {
                            tracing::debug!(actor_id = %actor_id, "parallel actor completed");
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
                        tracing::warn!(actor_id = %actor_id, %error, "parallel actor failed");
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

        Ok(Value::Array(digests))
    }
}

struct ParsedParallelTask {
    role: String,
    task: String,
    budget: ActorBudget,
}

fn parse_parallel_budget(item: &Value, index: usize) -> Result<ActorBudget> {
    let budget_value = item.get("budget");
    let mut budget = ActorBudget::default();
    if let Some(value) = budget_value
        && !value.is_object()
    {
        return Err(HostError::InvalidArgs(format!(
            "tasks[{index}].budget must be an object"
        )));
    }

    let wall_ms = item
        .get("timeoutMs")
        .or_else(|| budget_value.and_then(|budget| budget.get("wallMs")));
    if let Some(value) = wall_ms {
        budget.wall_ms = value.as_u64().ok_or_else(|| {
            HostError::InvalidArgs(format!(
                "tasks[{index}].timeoutMs must be a non-negative integer"
            ))
        })?;
    }

    if let Some(value) = budget_value.and_then(|budget| budget.get("maxDepth")) {
        let max_depth = value.as_u64().ok_or_else(|| {
            HostError::InvalidArgs(format!(
                "tasks[{index}].budget.maxDepth must be a non-negative integer"
            ))
        })?;
        budget.max_depth = u32::try_from(max_depth).map_err(|_| {
            HostError::InvalidArgs(format!("tasks[{index}].budget.maxDepth is too large"))
        })?;
    }

    Ok(budget)
}

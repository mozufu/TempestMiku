use super::*;

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
                     Requires agents.parallel grant."
                        .to_string(),
                ),
                signature: "agents.parallel(tasks: AgentTask[]): Promise<AgentDigest[]>".to_string(),
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
                                    "task": { "type": "string" }
                                }
                            }
                        }
                    }
                }),
                result_schema: Some(json!({ "type": "array", "items": { "type": "object" } })),
                examples: vec![ToolExample {
                    title: Some("Research three topics in parallel".to_string()),
                    code: "const results = await agents.parallel([\n  { role: 'researcher', task: 'Summarize §21' },\n  { role: 'researcher', task: 'Summarize §22' },\n]);\nresults.forEach(d => display(d.summary));"
                        .to_string(),
                    notes: Some(
                        "Digests only — use d.historyUri for the full output of each actor."
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

        let tasks = args["tasks"]
            .as_array()
            .ok_or_else(|| HostError::InvalidArgs("tasks must be an array".to_string()))?;

        if tasks.is_empty() {
            return Ok(json!([]));
        }

        // Validate all role/task strings before touching the executor or roster.
        let parsed: Vec<(String, String)> = tasks
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let role = item["role"]
                    .as_str()
                    .ok_or_else(|| {
                        HostError::InvalidArgs(format!("tasks[{i}].role must be a string"))
                    })?
                    .to_string();
                let task_str = item["task"]
                    .as_str()
                    .ok_or_else(|| {
                        HostError::InvalidArgs(format!("tasks[{i}].task must be a string"))
                    })?
                    .to_string();
                Ok((role, task_str))
            })
            .collect::<Result<_>>()?;

        let executor = self.roster.executor().ok_or_else(|| {
            HostError::NotImplemented("agents.parallel — executor not configured".to_string())
        })?;

        let session_id = ctx.session_id.clone();
        let parent_id = caller_parent_id(ctx)?;
        let supervisor_id = self.roster.next_supervisor_id("ParallelGroup");
        self.roster
            .set_supervision_policy(
                supervisor_id.clone(),
                SupervisionPolicy {
                    strategy: RestartStrategy::OneForAll,
                    max_restarts: 0,
                },
            )
            .await;

        // Register each actor as Running before spawning.
        let mut actor_specs: Vec<(ActorId, ActorSpec)> = Vec::with_capacity(parsed.len());
        for (role, task_str) in parsed {
            let actor_id = self.roster.next_actor_id(&role);
            let budget = ActorBudget::default();
            let spawned_at = Utc::now();
            self.roster
                .track_with_supervisor(
                    ActorRecord {
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
                    },
                    supervisor_id.clone(),
                )
                .await;
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
                role,
                task: task_str,
                mode: None,
                grants: child_agent_grants(ctx),
                budget,
                parent: parent_id.clone(),
                depth: 0,
                cancellation: self.roster.cancel_token(&actor_id).await,
            };
            self.roster.remember_restart_spec(&spec).await;
            actor_specs.push((actor_id.clone(), spec));
        }

        // Fan out: spawn all concurrently, collect handles in submission order.
        let handles: Vec<tokio::task::JoinHandle<Result<Value>>> = actor_specs
            .into_iter()
            .map(|(actor_id, spec)| {
                let executor = Arc::clone(&executor);
                let roster = Arc::clone(&self.roster);
                let session_id = session_id.clone();
                tokio::spawn(async move {
                    match run_actor_to_digest(executor, spec).await {
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
                            tracing::warn!(actor_id = %actor_id, error = %err, "parallel actor failed");
                            mark_actor_error(&roster, &session_id, &actor_id, reason).await;
                            Err(HostError::HostCall(format!("actor {actor_id} failed: {err}")))
                        }
                    }
                })
            })
            .collect();

        // Await in submission order so results[i] matches tasks[i].
        // On first actor failure, return the error (remaining actors run to completion
        // in their detached tokio tasks and update the roster via Arc).
        let mut digests: Vec<Value> = Vec::with_capacity(handles.len());
        for handle in handles {
            let result = handle
                .await
                .map_err(|e| HostError::HostCall(format!("parallel actor panicked: {e}")))?;
            match result {
                Ok(digest) => digests.push(digest),
                Err(err) => return Err(err),
            }
        }

        Ok(Value::Array(digests))
    }
}

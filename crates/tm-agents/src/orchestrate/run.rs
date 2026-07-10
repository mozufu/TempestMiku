use super::*;

// ─── agents.run ──────────────────────────────────────────────────────────────

pub struct AgentsRunFn {
    docs: ToolDocs,
    roster: Arc<MailboxRegistry>,
}

impl AgentsRunFn {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self {
            roster,
            docs: ToolDocs {
                name: caps::AGENTS_RUN.to_string(),
                namespace: "agents".to_string(),
                summary: "Spawn one actor and await its bounded digest result".to_string(),
                description: Some(
                    "Spawns a child actor, runs it to completion, and returns a bounded digest. \
                     Full output spills to artifact://; transcript is at history://<id>. \
                     The agent DAG must be acyclic. Requires agents.run grant."
                        .to_string(),
                ),
                signature:
                    "agents.run(role: string, task: string, opts?: AgentRunOpts): Promise<AgentDigest>"
                        .to_string(),
                args_schema: json!({
                    "type": "object",
                    "required": ["role", "task"],
                    "additionalProperties": false,
                    "properties": {
                        "role": { "type": "string", "description": "Mode/role for the child actor." },
                        "task": { "type": "string", "description": "Plain-prose task description." },
                        "opts": { "type": "object", "description": "Optional spawn options." }
                    }
                }),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "actorId":    { "type": "string" },
                        "summary":    { "type": "string" },
                        "artifactUri": { "type": ["string", "null"] },
                        "historyUri": { "type": ["string", "null"] }
                    }
                })),
                examples: vec![ToolExample {
                    title: Some("Run a one-shot research subtask".to_string()),
                    code: "const d = await agents.run('researcher', 'Summarize §23');\ndisplay(d.summary);"
                        .to_string(),
                    notes: Some(
                        "Only the digest returns to parent context; full output is at d.historyUri."
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
impl HostFn for AgentsRunFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_RUN);

        let role = args["role"]
            .as_str()
            .ok_or_else(|| HostError::InvalidArgs("role must be a string".to_string()))?
            .to_string();
        let task = args["task"]
            .as_str()
            .ok_or_else(|| HostError::InvalidArgs("task must be a string".to_string()))?
            .to_string();

        let executor = self.roster.executor().ok_or_else(|| {
            HostError::NotImplemented("agents.run — executor not configured".to_string())
        })?;

        let session_id = ctx.session_id.clone();
        let actor_id = self.roster.next_actor_id(&role);
        let parent_id = caller_parent_id(ctx)?;
        let budget = ActorBudget::default();
        let spawned_at = Utc::now();

        let record = ActorRecord {
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
        };
        self.roster.track(record).await;
        self.roster.emit_lifecycle(
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

        tracing::debug!(actor_id = %actor_id, role = %role, "actor spawned");

        let spec = ActorSpec {
            id: actor_id.clone(),
            session_id: session_id.clone(),
            session_scope: ctx.session_scope.clone(),
            role,
            task,
            mode: None,
            grants: child_agent_grants(ctx),
            budget,
            parent: parent_id,
            depth: 0,
            cancellation: self.roster.cancel_token(&actor_id).await,
        };
        self.roster.remember_restart_spec(&spec).await;

        match run_actor_to_digest(executor, spec).await {
            Ok(digest) => {
                let summary = digest.summary.clone();
                let artifact_uri = digest.artifact_uri.clone();
                let history_uri = digest.history_uri.clone();
                if mark_actor_completed(&self.roster, &session_id, &actor_id, digest).await {
                    tracing::debug!(actor_id = %actor_id, "actor completed");
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
                tracing::warn!(actor_id = %actor_id, %error, "actor failed");
                mark_actor_error(&self.roster, &session_id, &actor_id, reason).await;
                Err(HostError::HostCall(error))
            }
        }
    }
}

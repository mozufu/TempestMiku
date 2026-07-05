use super::*;

// ─── agents.spawn ─────────────────────────────────────────────────────────────

pub struct AgentsSpawnFn {
    docs: ToolDocs,
    roster: Arc<MailboxRegistry>,
}

impl AgentsSpawnFn {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self {
            roster,
            docs: ToolDocs {
                name: caps::AGENTS_SPAWN.to_string(),
                namespace: "agents".to_string(),
                summary: "Spawn a long-running child actor and return a handle".to_string(),
                description: Some(
                    "Non-blocking spawn; returns a handle for later coordination via agents.msg. \
                     The actor runs in the background and is tracked through the agent:// roster. \
                     Requires agents.spawn grant."
                        .to_string(),
                ),
                signature: "agents.spawn(role: string, task: string): Promise<AgentHandle>"
                    .to_string(),
                args_schema: json!({
                    "type": "object",
                    "required": ["role", "task"],
                    "additionalProperties": false,
                    "properties": {
                        "role": { "type": "string" },
                        "task": { "type": "string" }
                    }
                }),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": { "id": { "type": "string" } }
                })),
                examples: vec![ToolExample {
                    title: Some("Spawn a background worker".to_string()),
                    code: "const h = await agents.spawn('worker', 'Process the batch');\nconst reply = await agents.msg(h, 'Status?', { await: true });\ndisplay(reply);"
                        .to_string(),
                    notes: None,
                }],
                errors: vec![denied_error_doc()],
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
impl HostFn for AgentsSpawnFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_SPAWN);

        let role = args["role"]
            .as_str()
            .ok_or_else(|| HostError::InvalidArgs("role must be a string".to_string()))?
            .to_string();
        let task = args["task"]
            .as_str()
            .ok_or_else(|| HostError::InvalidArgs("task must be a string".to_string()))?
            .to_string();

        let executor = self.roster.executor().ok_or_else(|| {
            HostError::NotImplemented("agents.spawn — executor not configured".to_string())
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

        let spec = ActorSpec {
            id: actor_id.clone(),
            session_id: session_id.clone(),
            role,
            task,
            mode: None,
            grants: child_agent_grants(ctx),
            budget,
            parent: parent_id,
            depth: 0,
        };

        // Background thread: actor runs in its own current_thread runtime so it
        // survives past the parent cell's await and doesn't block the caller.
        let roster = Arc::clone(&self.roster);
        let actor_id_bg = actor_id.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("spawn worker runtime");
            tracing::debug!(actor_id = %actor_id_bg, "spawned actor started");
            match rt.block_on(executor.run_to_digest(spec)) {
                Ok(digest) => {
                    if let Some(content) = digest.history_content {
                        rt.block_on(roster.store_transcript(&actor_id_bg, content));
                    }
                    rt.block_on(roster.mark_complete_with_digest(
                        &actor_id_bg,
                        digest.summary.clone(),
                        digest.artifact_uri.clone(),
                        digest.history_uri.clone(),
                    ));
                    roster.emit_lifecycle(
                        &session_id,
                        ActorLifecycleEvent::Completed {
                            actor_id: actor_id_bg.clone(),
                            completed_at: Utc::now(),
                            summary: Some(digest.summary),
                            artifact_uri: digest.artifact_uri,
                            history_uri: digest.history_uri,
                        },
                    );
                    tracing::debug!(actor_id = %actor_id_bg, "spawned actor completed");
                }
                Err(err) => {
                    let reason = map_actor_error(&err);
                    tracing::warn!(actor_id = %actor_id_bg, error = %err, "spawned actor failed");
                    roster.emit_lifecycle(
                        &session_id,
                        ActorLifecycleEvent::Failed {
                            actor_id: actor_id_bg.clone(),
                            failed_at: Utc::now(),
                            reason: reason.clone(),
                        },
                    );
                    rt.block_on(roster.mark_failed(&actor_id_bg, reason));
                }
            }
        });

        Ok(json!({ "id": actor_id.as_str() }))
    }
}

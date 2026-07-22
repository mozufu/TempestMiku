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
                    "Non-blocking spawn; returns a handle for later coordination via agents.send. \
                     The actor runs in the background and is tracked through the agent:// roster. \
                     Children receive no parent capabilities unless opts.capabilities explicitly \
                     delegates a held, delegable subset. Requires agents.spawn grant."
                        .to_string(),
                ),
                signature: "@agents.spawn AgentSpawnArgs -> AgentHandle"
                    .to_string(),
                args_schema: json!({
                    "type": "object",
                    "required": ["role", "task"],
                    "additionalProperties": false,
                    "properties": {
                        "role": { "type": "string" },
                        "task": { "type": "string" },
                        "opts": {
                            "type": "object",
                            "additionalProperties": false,
                            "description": "Optional child authority delegation. Absent means no delegated capabilities; at most 32 names, each at most 128 ASCII bytes and 2 KiB total.",
                            "properties": {
                                "capabilities": {
                                    "type": "array",
                                    "maxItems": 32,
                                    "items": { "type": "string", "maxLength": 128 }
                                },
                                "supervision": {
                                    "type": "object",
                                    "properties": {
                                        "group": { "type": "string" },
                                        "strategy": {
                                            "type": "string",
                                            "enum": ["one_for_one", "one_for_all", "rest_for_one"]
                                        },
                                        "maxRestarts": { "type": "integer", "minimum": 0 }
                                    }
                                }
                            }
                        }
                    }
                }),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": { "id": { "type": "string" } }
                })),
                examples: vec![ToolExample {
                    title: Some("Spawn a background worker".to_string()),
                    code: "let h = @agents.spawn {role: \"worker\", task: \"Process the batch and reply to parent messages\", opts: {capabilities: [\"agents.*\"]}};\nlet reply = @agents.send {to: h, text: \"Status?\", opts: {await: true}};\nreply |> display {kind: \"json\"}"
                        .to_string(),
                    notes: Some(
                        "The parent must hold agents.* before explicitly delegating it to the child."
                            .to_string(),
                    ),
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

        let role = parse_actor_role(&args["role"], "role")?;
        let task = parse_actor_task(&args["task"], "task")?;
        let opts = args.get("opts");
        let grants = child_agent_grants(ctx, opts)?;

        let executor = self.roster.executor().ok_or_else(|| {
            HostError::NotImplemented("agents.spawn — executor not configured".to_string())
        })?;

        let session_id = ctx.session_id.clone();
        let actor_id = self.roster.next_actor_id(&role);
        let parent_id = caller_parent_id(ctx)?;
        let supervisor_id =
            configure_supervision_from_opts(&self.roster, &session_id, parent_id.as_ref(), opts)
                .await?;
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
            artifact_uri: None,
            history_uri: None,
        };
        match supervisor_id {
            Some(supervisor_id) => self
                .roster
                .track_with_supervisor_for_session(&session_id, record, supervisor_id)
                .await
                .map_err(registry_error)?,
            None => self
                .roster
                .track_for_session(&session_id, record)
                .await
                .map_err(registry_error)?,
        }
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
            project_id: ctx.project_id.clone(),
            role,
            task,
            mode: None,
            grants,
            budget,
            parent: parent_id,
            depth: 0,
            cancellation: self
                .roster
                .cancel_token_for_session(&session_id, &actor_id)
                .await,
        };
        self.roster.remember_restart_spec(&spec).await;

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
            match rt.block_on(run_actor_to_digest(executor, spec)) {
                Ok(digest) => {
                    let completed = rt.block_on(mark_actor_completed(
                        &roster,
                        &session_id,
                        &actor_id_bg,
                        digest,
                    ));
                    if completed {
                        tracing::debug!(actor_id = %actor_id_bg, "spawned actor completed");
                    }
                }
                Err(err) => {
                    let reason = failure_reason_for_error(&err);
                    let error = redact_actor_diagnostic(&err);
                    tracing::warn!(actor_id = %actor_id_bg, %error, "spawned actor failed");
                    rt.block_on(mark_actor_error(&roster, &session_id, &actor_id_bg, reason));
                }
            }
        });

        Ok(json!({ "id": actor_id.as_str() }))
    }
}

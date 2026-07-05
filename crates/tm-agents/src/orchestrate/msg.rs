use super::*;

// ─── agents.msg ───────────────────────────────────────────────────────────────

pub struct AgentsMsgFn {
    docs: ToolDocs,
    roster: Arc<MailboxRegistry>,
}

impl AgentsMsgFn {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self {
            roster,
            docs: ToolDocs {
                name: caps::AGENTS_MSG.to_string(),
                namespace: "agents".to_string(),
                summary: "Send a plain-prose message to a spawned actor".to_string(),
                description: Some(
                    "Sends a plain-prose message to a spawned actor handle. \
                     Fire-and-forget (default): delivers to a running actor's live bounded inbox \
                     and returns null immediately. \
                     Request/reply (opts.await = true): for running actors, delivers to the live \
                     inbox and waits for a reply to the caller inbox. For already completed actors, \
                     keeps the P3 compatibility behavior: a one-shot seeded continuation from the \
                     target's last digest summary + the new text. \
                     A null reply means the actor is unreachable — do not retry-loop (§23.9). \
                     Messages are plain prose — never control-payload blobs. \
                     Pass large payloads by reference (artifact://, memory://). \
                     Requires agents.msg grant."
                        .to_string(),
                ),
                signature: "agents.msg(handle: AgentHandle, text: string, opts?: MsgOpts): Promise<string | void>"
                    .to_string(),
                args_schema: json!({
                    "type": "object",
                    "required": ["handle", "text"],
                    "additionalProperties": false,
                    "properties": {
                        "handle": { "type": "object", "description": "Handle from agents.spawn." },
                        "text":   { "type": "string",  "description": "Plain-prose message body." },
                        "opts": {
                            "type": "object",
                            "properties": {
                                "await": {
                                    "type": "boolean",
                                    "description": "If true, block for the actor's reply."
                                }
                            }
                        }
                    }
                }),
                result_schema: Some(json!({ "type": ["string", "null"] })),
                examples: vec![ToolExample {
                    title: Some("Request a status update".to_string()),
                    code: "const reply = await agents.msg(handle, 'What is your current status?', { await: true });\nif (reply) display(reply);"
                        .to_string(),
                    notes: Some("null reply means unreachable — move on.".to_string()),
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
impl HostFn for AgentsMsgFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_MSG);

        // ── Parse arguments ───────────────────────────────────────────────────
        let id_str = args["handle"]["id"]
            .as_str()
            .ok_or_else(|| HostError::InvalidArgs("handle.id must be a string".to_string()))?;
        let target_id = ActorId::new(id_str)
            .map_err(|e| HostError::InvalidArgs(format!("handle.id invalid: {e}")))?;

        let text = args["text"]
            .as_str()
            .ok_or_else(|| HostError::InvalidArgs("text must be a string".to_string()))?
            .to_string();

        let do_await = args["opts"]["await"].as_bool().unwrap_or(false);

        // ── Resolve target ────────────────────────────────────────────────────
        // Not found → unreachable receipt (§23.9): return null, do not error.
        let target_record = match self.roster.get(&target_id).await {
            Some(rec) => rec,
            None => return Ok(Value::Null),
        };

        let from_id = caller_actor_id(ctx)?;

        // ── Fire-and-forget ───────────────────────────────────────────────────
        if !do_await {
            let message = ActorMessage {
                from: from_id,
                to: target_id,
                text,
                reply_to: None,
                sent_at: Utc::now(),
            };
            let receipt = self.roster.send_message(message.clone()).await;
            maybe_emit_message(&self.roster, &ctx.session_id, &message, receipt);
            return Ok(Value::Null);
        }

        // Live request/reply for running actors: deliver to the target inbox, then
        // wait for a plain-prose reply back to the caller inbox.
        if target_record.status != ActorStatus::Terminated {
            let timeout_ms = parse_timeout_ms(&args, 30_000)?;
            let message = ActorMessage {
                from: from_id.clone(),
                to: target_id.clone(),
                text,
                reply_to: Some(from_id.clone()),
                sent_at: Utc::now(),
            };
            let receipt = self.roster.send_message(message.clone()).await;
            maybe_emit_message(&self.roster, &ctx.session_id, &message, receipt);
            if receipt == Receipt::Failed {
                return Ok(Value::Null);
            }
            return Ok(self
                .roster
                .wait_for_message(
                    &from_id,
                    Some(&target_id),
                    Duration::from_millis(timeout_ms),
                )
                .await
                .map(|message| Value::String(message.text))
                .unwrap_or(Value::Null));
        }

        // ── Request/reply ─────────────────────────────────────────────────────
        // Requires executor — mirrors the pattern in agents.run / agents.spawn.
        let executor = self.roster.executor().ok_or_else(|| {
            HostError::NotImplemented("agents.msg await — executor not configured".to_string())
        })?;

        // Record the outgoing message before running the continuation. The target
        // is already terminated, so this is a compatibility log row rather than a
        // live inbox delivery.
        self.roster
            .record_message(ActorMessage {
                from: from_id,
                to: target_id.clone(),
                text: text.clone(),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;

        // Seed the continuation from the target's stored summary (one-shot model).
        // Each await call is stateless: re-seeds from the original summary, not prior replies.
        let prior_context = target_record
            .last_summary
            .as_deref()
            .unwrap_or("(no prior context)");
        let continuation_role = target_record
            .mode
            .as_deref()
            .unwrap_or("worker")
            .to_string();
        let seeded_task = format!("Prior context: {prior_context}\n\nNew message: {text}");

        let continuation_id = self.roster.next_actor_id(&continuation_role);
        let budget = ActorBudget::default();
        let spec = ActorSpec {
            id: continuation_id.clone(),
            session_id: ctx.session_id.clone(),
            role: continuation_role,
            task: seeded_task,
            mode: None,
            grants: child_agent_grants(ctx),
            budget,
            parent: Some(target_id),
            depth: 0,
        };

        // Continuation actors are ephemeral — NOT tracked in the roster.
        // Tracking them would let repeated top-level msgs grow the roster unboundedly.
        let digest = executor
            .run_to_digest(spec)
            .await
            .map_err(|e| HostError::HostCall(e.to_string()))?;

        Ok(Value::String(digest.summary))
    }
}

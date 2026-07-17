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
                     and returns a delivered/failed receipt. \
                     Request/reply (opts.await = true): for running actors, delivers to the live \
                     inbox and waits for a reply to the caller inbox. If live delivery fails, \
                     returns a failed receipt instead of waiting. For already completed actors, \
                     keeps the P3 compatibility behavior: a one-shot seeded continuation from the \
                     target's last digest summary + the new text. \
                     A failed receipt means the actor is unreachable or backpressured — do not retry-loop (§23.9). \
                     Live request/reply from a real actor to its own descendant is rejected to keep \
                     the actor DAG acyclic. \
                     Messages are plain prose — never control-payload blobs. \
                     Pass large payloads by reference (artifact://, memory://). \
                     Requires agents.msg grant."
                        .to_string(),
                ),
                signature: "@agents.msg AgentMsgArgs -> AgentReceipt | String | null"
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
                result_schema: Some(json!({
                    "oneOf": [
                        { "type": "object", "properties": { "status": { "type": "string" }, "reason": { "type": "string", "enum": ["unreachable", "backpressured"] } } },
                        { "type": "string" },
                        { "type": "null" }
                    ]
                })),
                examples: vec![ToolExample {
                    title: Some("Request a status update".to_string()),
                    code: "let reply = @agents.msg {handle: handle, text: \"What is your current status?\", opts: {await: true}};\nreply |> display {kind: \"json\"}"
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

        let text = parse_plain_prose_text(&args, "text")?;

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
            return Ok(receipt_json(receipt));
        }

        // Live request/reply for running actors: deliver to the target inbox, then
        // wait for a plain-prose reply back to the caller inbox.
        if MailboxRegistry::is_live_status(target_record.status) {
            reject_descendant_wait(&self.roster, &from_id, &target_id, "handle").await?;
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
            if receipt.is_failed() {
                return Ok(receipt_json(receipt));
            }
            return Ok(wait_for_actor_message_or_cancel(
                &self.roster,
                &from_id,
                Some(&target_id),
                Duration::from_millis(timeout_ms),
            )
            .await?
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
            session_scope: ctx.session_scope.clone(),
            role: continuation_role,
            task: seeded_task,
            mode: None,
            grants: child_agent_grants(ctx),
            budget,
            parent: Some(target_id),
            depth: 0,
            cancellation: crate::actor::ActorCancelToken::default(),
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

use super::*;

// ─── agents.broadcast ────────────────────────────────────────────────────────

pub struct AgentsBroadcastFn {
    docs: ToolDocs,
    roster: Arc<MailboxRegistry>,
}

impl AgentsBroadcastFn {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self {
            roster,
            docs: ToolDocs {
                name: caps::AGENTS_BROADCAST.to_string(),
                namespace: "agents".to_string(),
                summary: "Deliver a plain-prose message to direct live child actors".to_string(),
                description: Some(
                    "Broadcasts a plain-prose message to the caller's direct live children \
                     through bounded per-actor inboxes. Top-level sessions use the synthetic \
                     Root actor, so they target root-level live children. Broadcast is \
                     fire-and-forget only; no replies are awaited. JSON control payloads \
                     are rejected. Returns one receipt per target. Requires agents.broadcast grant."
                        .to_string(),
                ),
                signature:
                    "agents.broadcast(text: string): Promise<AgentBroadcastReceipt[]>".to_string(),
                args_schema: json!({
                    "type": "object",
                    "required": ["text"],
                    "additionalProperties": false,
                    "properties": {
                        "text": { "type": "string" }
                    }
                }),
                result_schema: Some(json!({
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "actorId": { "type": "string" },
                            "status": { "type": "string" },
                            "reason": { "type": "string", "enum": ["unreachable", "backpressured"] }
                        }
                    }
                })),
                examples: vec![ToolExample {
                    title: Some("Notify all direct children".to_string()),
                    code: "const receipts = await agents.broadcast('Please send your final status when ready.');"
                        .to_string(),
                    notes: Some(
                        "A failed receipt means that target was unreachable or backpressured — move on."
                            .to_string(),
                    ),
                }],
                errors: vec![denied_error_doc()],
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
impl HostFn for AgentsBroadcastFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_BROADCAST);

        let text = parse_plain_prose_text(&args, "text")?;
        let from = caller_actor_id(ctx)?;

        let mut targets = self.roster.live_direct_children(&from).await;
        targets.sort_by(|a, b| a.id.cmp(&b.id));

        let mut receipts = Vec::with_capacity(targets.len());
        for target in targets {
            let message = ActorMessage {
                from: from.clone(),
                to: target.id.clone(),
                text: text.clone(),
                reply_to: None,
                sent_at: Utc::now(),
            };
            let receipt = self.roster.send_message(message.clone()).await;
            maybe_emit_message(&self.roster, &ctx.session_id, &message, receipt);
            let mut receipt_value = receipt_json(receipt);
            if let Some(object) = receipt_value.as_object_mut() {
                object.insert("actorId".to_string(), json!(target.id.as_str()));
            }
            receipts.push(receipt_value);
        }

        Ok(Value::Array(receipts))
    }
}

// ─── agents.cancel ───────────────────────────────────────────────────────────

pub struct AgentsCancelFn {
    docs: ToolDocs,
    roster: Arc<MailboxRegistry>,
}

impl AgentsCancelFn {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self {
            roster,
            docs: ToolDocs {
                name: caps::AGENTS_CANCEL.to_string(),
                namespace: "agents".to_string(),
                summary: "Cancel a direct child actor".to_string(),
                description: Some(
                    "Requests cancellation for a direct child actor. The actor record is \
                     marked terminal immediately, the child cancellation token is tripped, \
                     and a replayable actor_cancelled lifecycle event is emitted once. \
                     Only the direct parent, or the synthetic top-level Root for root-level \
                     children, may cancel an actor. Requires agents.cancel grant."
                        .to_string(),
                ),
                signature:
                    "agents.cancel(target: AgentHandle | string): Promise<AgentCancelReceipt>"
                        .to_string(),
                args_schema: json!({
                    "type": "object",
                    "required": ["target"],
                    "additionalProperties": false,
                    "properties": {
                        "target": { "description": "Actor id string or AgentHandle." }
                    }
                }),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "actorId": { "type": "string" },
                        "status": {
                            "type": "string",
                            "enum": ["cancelled", "already_cancelled", "already_terminated", "not_found"]
                        }
                    }
                })),
                examples: vec![ToolExample {
                    title: Some("Cancel a child worker".to_string()),
                    code: "const worker = await agents.spawn('worker', 'Monitor the queue');\nconst receipt = await agents.cancel(worker);\ndisplay(receipt);"
                        .to_string(),
                    notes: Some(
                        "A cancelled actor is terminal; future sends to it return failed receipts."
                            .to_string(),
                    ),
                }],
                errors: vec![denied_error_doc()],
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
impl HostFn for AgentsCancelFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_CANCEL);

        let target = parse_actor_ref(
            args.get("target")
                .ok_or_else(|| HostError::InvalidArgs("target is required".to_string()))?,
            "target",
        )?;
        let Some(record) = self.roster.get(&target).await else {
            return Ok(json!({
                "actorId": target.as_str(),
                "status": "not_found",
            }));
        };

        let caller = caller_actor_id(ctx)?;
        let is_direct_child = match record.parent.as_ref() {
            Some(parent) => parent == &caller,
            None => caller.as_str() == "Root",
        };
        if !is_direct_child {
            return Err(HostError::InvalidArgs(
                "target must be a direct child of the caller".to_string(),
            ));
        }

        let result = self.roster.cancel_actor(&ctx.session_id, &target).await;
        Ok(json!({
            "actorId": target.as_str(),
            "status": result.as_str(),
        }))
    }
}

// ─── agents.send ──────────────────────────────────────────────────────────────

pub struct AgentsSendFn {
    docs: ToolDocs,
    roster: Arc<MailboxRegistry>,
}

impl AgentsSendFn {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self {
            roster,
            docs: ToolDocs {
                name: caps::AGENTS_SEND.to_string(),
                namespace: "agents".to_string(),
                summary: "Deliver a plain-prose message to one live actor inbox".to_string(),
                description: Some(
                    "Sends a plain-prose message to a live actor id or handle through the \
                     bounded per-actor inbox. Fire-and-forget returns a delivered/failed \
                     receipt. With opts.await = true, waits for the recipient to reply to \
                     the caller inbox and returns the reply message, returns a failed receipt \
                     on unreachable/backpressured delivery, or null on timeout. Awaiting a real actor's own descendant is rejected \
                     to keep the actor DAG acyclic. Messages are plain prose; JSON control \
                     payloads are rejected. Pass large payloads by reference. \
                     Requires agents.send grant."
                        .to_string(),
                ),
                signature: "agents.send(to: AgentHandle | string, text: string, opts?: SendOpts): Promise<AgentReceipt | AgentMessage | null>"
                    .to_string(),
                args_schema: json!({
                    "type": "object",
                    "required": ["to", "text"],
                    "additionalProperties": false,
                    "properties": {
                        "to": { "description": "Actor id string or AgentHandle." },
                        "text": { "type": "string" },
                        "opts": {
                            "type": "object",
                            "properties": {
                                "await": { "type": "boolean" },
                                "timeoutMs": { "type": "integer", "minimum": 0 }
                            }
                        }
                    }
                }),
                result_schema: Some(json!({
                    "oneOf": [
                        { "type": "object", "properties": { "status": { "type": "string" }, "reason": { "type": "string", "enum": ["unreachable", "backpressured"] } } },
                        { "type": "object", "properties": { "from": { "type": "string" }, "to": { "type": "string" }, "text": { "type": "string" } } },
                        { "type": "null" }
                    ]
                })),
                examples: vec![ToolExample {
                    title: Some("Send a live actor message".to_string()),
                    code: "const receipt = await agents.send(worker, 'Please send your status when ready.');"
                        .to_string(),
                    notes: Some(
                        "A failed receipt means unreachable or backpressured — move on."
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
impl HostFn for AgentsSendFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_SEND);

        let to = parse_actor_ref(
            args.get("to")
                .ok_or_else(|| HostError::InvalidArgs("to is required".to_string()))?,
            "to",
        )?;
        let text = parse_plain_prose_text(&args, "text")?;
        let do_await = args["opts"]["await"].as_bool().unwrap_or(false);
        let from = caller_actor_id(ctx)?;
        if do_await {
            reject_descendant_wait(&self.roster, &from, &to, "to").await?;
        }

        let message = ActorMessage {
            from: from.clone(),
            to: to.clone(),
            text,
            reply_to: do_await.then_some(from.clone()),
            sent_at: Utc::now(),
        };
        let receipt = self.roster.send_message(message.clone()).await;
        maybe_emit_message(&self.roster, &ctx.session_id, &message, receipt);

        if !do_await {
            return Ok(receipt_json(receipt));
        }
        if receipt.is_failed() {
            return Ok(receipt_json(receipt));
        }

        let timeout_ms = parse_timeout_ms(&args, 30_000)?;
        Ok(wait_for_actor_message_or_cancel(
            &self.roster,
            &from,
            Some(&to),
            Duration::from_millis(timeout_ms),
        )
        .await?
        .map(message_json)
        .unwrap_or(Value::Null))
    }
}

// ─── agents.wait ──────────────────────────────────────────────────────────────

pub struct AgentsWaitFn {
    docs: ToolDocs,
    roster: Arc<MailboxRegistry>,
}

impl AgentsWaitFn {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self {
            roster,
            docs: ToolDocs {
                name: caps::AGENTS_WAIT.to_string(),
                namespace: "agents".to_string(),
                summary: "Wait for one message in the current actor inbox".to_string(),
                description: Some(
                    "Blocks until the current actor inbox receives a matching message, \
                     optionally filtered by sender. Top-level sessions use the synthetic \
                     Root inbox. A real actor cannot target a wait at itself or its own \
                     descendant. Returns null on timeout. Requires agents.wait grant."
                        .to_string(),
                ),
                signature: "agents.wait(from?: AgentHandle | string, timeoutMs?: number): Promise<AgentMessage | null>"
                    .to_string(),
                args_schema: json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "from": { "description": "Optional sender actor id string or AgentHandle." },
                        "timeoutMs": { "type": "integer", "minimum": 0 }
                    }
                }),
                result_schema: Some(json!({
                    "type": ["object", "null"],
                    "properties": {
                        "from": { "type": "string" },
                        "to": { "type": "string" },
                        "text": { "type": "string" },
                        "replyTo": { "type": ["string", "null"] },
                        "sentAt": { "type": "string" }
                    }
                })),
                examples: vec![ToolExample {
                    title: Some("Wait for a worker reply".to_string()),
                    code: "const msg = await agents.wait(worker, 5000);\nif (msg) display(msg.text);"
                        .to_string(),
                    notes: Some("null means timeout; do not retry-loop.".to_string()),
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
impl HostFn for AgentsWaitFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_WAIT);

        let actor_id = caller_actor_id(ctx)?;
        let from = match args.get("from").filter(|value| !value.is_null()) {
            Some(value) => Some(parse_actor_ref(value, "from")?),
            None => None,
        };
        if let Some(from) = from.as_ref() {
            reject_descendant_wait(&self.roster, &actor_id, from, "from").await?;
        }
        let timeout_ms = parse_timeout_ms(&args, 30_000)?;

        Ok(wait_for_actor_message_or_cancel(
            &self.roster,
            &actor_id,
            from.as_ref(),
            Duration::from_millis(timeout_ms),
        )
        .await?
        .map(message_json)
        .unwrap_or(Value::Null))
    }
}

// ─── agents.inbox ─────────────────────────────────────────────────────────────

pub struct AgentsInboxFn {
    docs: ToolDocs,
    roster: Arc<MailboxRegistry>,
}

impl AgentsInboxFn {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self {
            roster,
            docs: ToolDocs {
                name: caps::AGENTS_INBOX.to_string(),
                namespace: "agents".to_string(),
                summary: "Drain pending messages from the current actor inbox".to_string(),
                description: Some(
                    "Drains all currently pending messages for the current actor without \
                     blocking. Top-level sessions use the synthetic Root inbox. Requires \
                     agents.inbox grant."
                        .to_string(),
                ),
                signature: "agents.inbox(): Promise<AgentMessage[]>".to_string(),
                args_schema: json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {}
                }),
                result_schema: Some(json!({ "type": "array", "items": { "type": "object" } })),
                examples: vec![ToolExample {
                    title: Some("Drain pending messages".to_string()),
                    code: "for (const msg of await agents.inbox()) display(msg.text);".to_string(),
                    notes: None,
                }],
                errors: vec![denied_error_doc()],
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
impl HostFn for AgentsInboxFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, _args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_INBOX);
        let actor_id = caller_actor_id(ctx)?;
        Ok(Value::Array(
            self.roster
                .drain_inbox(&actor_id, None)
                .await
                .into_iter()
                .map(message_json)
                .collect(),
        ))
    }
}

// ─── agents.list ──────────────────────────────────────────────────────────────

pub struct AgentsListFn {
    docs: ToolDocs,
    roster: Arc<MailboxRegistry>,
}

impl AgentsListFn {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self {
            roster,
            docs: ToolDocs {
                name: caps::AGENTS_LIST.to_string(),
                namespace: "agents".to_string(),
                summary: "List actor roster entries with inbox metadata".to_string(),
                description: Some(
                    "Returns the current actor roster with status, unread inbox count, \
                     last activity timestamp, and resource links. Requires agents.list grant."
                        .to_string(),
                ),
                signature: "agents.list(): Promise<AgentRosterEntry[]>".to_string(),
                args_schema: json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {}
                }),
                result_schema: Some(json!({ "type": "array", "items": { "type": "object" } })),
                examples: vec![ToolExample {
                    title: Some("Show live actors".to_string()),
                    code: "const actors = await agents.list();\ndisplay(actors, { kind: 'json' });"
                        .to_string(),
                    notes: None,
                }],
                errors: vec![denied_error_doc()],
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
impl HostFn for AgentsListFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, _args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_LIST);
        let mut entries = Vec::new();
        for record in self.roster.list().await {
            entries.push(json!({
                "id": record.id.as_str(),
                "parentId": record.parent.as_ref().map(|id| id.as_str().to_string()),
                "status": record.status,
                "mode": record.mode,
                "unread": self.roster.unread_count(&record.id).await,
                "lastActivity": self
                    .roster
                    .last_activity(&record.id)
                    .await
                    .map(|at| at.to_rfc3339()),
                "artifactUri": record.artifact_uri,
                "historyUri": record.history_uri,
            }));
        }
        Ok(Value::Array(entries))
    }
}

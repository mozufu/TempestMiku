use super::*;

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
                     the caller inbox and returns the reply message, or null on timeout or \
                     unreachable target. Messages are plain prose; pass large payloads by \
                     reference. Requires agents.send grant."
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
                        { "type": "object", "properties": { "status": { "type": "string" } } },
                        { "type": "object", "properties": { "from": { "type": "string" }, "to": { "type": "string" }, "text": { "type": "string" } } },
                        { "type": "null" }
                    ]
                })),
                examples: vec![ToolExample {
                    title: Some("Send a live actor message".to_string()),
                    code: "const receipt = await agents.send(worker, 'Please send your status when ready.');"
                        .to_string(),
                    notes: Some("A failed receipt means unreachable — move on.".to_string()),
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
        let text = args["text"]
            .as_str()
            .ok_or_else(|| HostError::InvalidArgs("text must be a string".to_string()))?
            .to_string();
        let do_await = args["opts"]["await"].as_bool().unwrap_or(false);
        let from = caller_actor_id(ctx)?;

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
        if receipt == Receipt::Failed {
            return Ok(Value::Null);
        }

        let timeout_ms = parse_timeout_ms(&args, 30_000)?;
        Ok(self
            .roster
            .wait_for_message(&from, Some(&to), Duration::from_millis(timeout_ms))
            .await
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
                     Root inbox. Returns null on timeout. Requires agents.wait grant."
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
        let timeout_ms = parse_timeout_ms(&args, 30_000)?;

        Ok(self
            .roster
            .wait_for_message(&actor_id, from.as_ref(), Duration::from_millis(timeout_ms))
            .await
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

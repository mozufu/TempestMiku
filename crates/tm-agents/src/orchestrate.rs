use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};
use tm_host::{
    CapabilityGrants, GrantDoc, HostError, HostFn, HostRegistry, InvocationCtx, ResourceRegistry,
    Result, ToolDocs, ToolErrorDoc, ToolExample,
};

use crate::actor::{
    ActorBudget, ActorId, ActorLifecycleEvent, ActorRecord, ActorSpec, ActorStatus,
};
use crate::executor::ActorError;
use crate::mailbox::{ActorMessage, MailboxRegistry, Receipt};
use crate::supervise::FailureReason;

/// Capability names for the P3/P3-plus `agents.*` calls (§23.3, ROADMAP authority).
///
/// `pipeline`, `broadcast`, active supervision, cancel, and child approval routing remain later
/// P3-plus work.
pub mod caps {
    pub const AGENTS_RUN: &str = "agents.run";
    pub const AGENTS_SPAWN: &str = "agents.spawn";
    pub const AGENTS_PARALLEL: &str = "agents.parallel";
    pub const AGENTS_MSG: &str = "agents.msg";
    pub const AGENTS_SEND: &str = "agents.send";
    pub const AGENTS_WAIT: &str = "agents.wait";
    pub const AGENTS_INBOX: &str = "agents.inbox";
    pub const AGENTS_LIST: &str = "agents.list";
}

/// Register `agents.*` HostFns and `agent://`/`history://` ResourceHandlers.
///
/// Called from `tm-server/src/main.rs` at startup. `agents.run` executes when the registry
/// holds an injected executor (set via `MailboxRegistry::set_executor` after this call).
/// `agents.spawn` and `agents.parallel` also execute with the injected executor; live P3-plus
/// mailbox primitives deliver through per-actor bounded inboxes.
pub fn register(
    host_registry: &mut HostRegistry,
    resource_registry: &mut ResourceRegistry,
    roster: Arc<MailboxRegistry>,
) {
    host_registry.register(Arc::new(AgentsRunFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsSpawnFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsParallelFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsMsgFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsSendFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsWaitFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsInboxFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsListFn::new(Arc::clone(&roster))));
    resource_registry.register(Arc::new(crate::resources::AgentResourceHandler::new(
        Arc::clone(&roster),
    )));
    resource_registry.register(Arc::new(crate::resources::HistoryResourceHandler::new(
        roster,
    )));
}

macro_rules! check_grant {
    ($ctx:expr, $cap:expr) => {
        if !$ctx.grants.permits($cap) {
            return Err(HostError::CapabilityDenied($cap.to_string()));
        }
    };
}

fn agents_grant_doc() -> GrantDoc {
    GrantDoc {
        kind: "agents".to_string(),
        description: "Spawn and coordinate sub-agents. Only available in granted sessions."
            .to_string(),
    }
}

fn denied_error_doc() -> ToolErrorDoc {
    ToolErrorDoc {
        name: "CapabilityDeniedError".to_string(),
        when: "The session does not hold the required agents.* grant.".to_string(),
        retryable: false,
    }
}

fn timeout_error_doc() -> ToolErrorDoc {
    ToolErrorDoc {
        name: "TimeoutError".to_string(),
        when: "The actor exceeded its wall-clock budget.".to_string(),
        retryable: true,
    }
}

fn map_actor_error(err: &ActorError) -> FailureReason {
    match err {
        ActorError::Execution(msg) => FailureReason::Crash {
            message: msg.clone(),
        },
        ActorError::InvalidSpec(msg) => FailureReason::Crash {
            message: msg.clone(),
        },
        ActorError::DepthExceeded(_) => FailureReason::DepthExceeded,
    }
}

fn root_actor_id() -> ActorId {
    ActorId::new("Root").expect("Root is always valid")
}

fn caller_actor_id(ctx: &InvocationCtx) -> Result<ActorId> {
    match ctx.actor_id.as_deref() {
        Some(id) => ActorId::new(id)
            .map_err(|err| HostError::InvalidArgs(format!("actor_id invalid: {err}"))),
        None => Ok(root_actor_id()),
    }
}

fn caller_parent_id(ctx: &InvocationCtx) -> Result<Option<ActorId>> {
    ctx.actor_id
        .as_deref()
        .map(|id| {
            ActorId::new(id)
                .map_err(|err| HostError::InvalidArgs(format!("actor_id invalid: {err}")))
        })
        .transpose()
}

fn parse_actor_ref(value: &Value, field: &str) -> Result<ActorId> {
    let id = value
        .as_str()
        .or_else(|| value.get("id").and_then(Value::as_str))
        .ok_or_else(|| {
            HostError::InvalidArgs(format!("{field} must be an actor id string or handle"))
        })?;
    ActorId::new(id).map_err(|err| HostError::InvalidArgs(format!("{field} invalid: {err}")))
}

fn parse_timeout_ms(args: &Value, default_ms: u64) -> Result<u64> {
    let value = args
        .get("timeoutMs")
        .or_else(|| args.get("opts").and_then(|opts| opts.get("timeoutMs")));
    match value.and_then(Value::as_u64) {
        Some(ms) => Ok(ms),
        None if value.is_some() => Err(HostError::InvalidArgs(
            "timeoutMs must be a non-negative integer".to_string(),
        )),
        None => Ok(default_ms),
    }
}

fn child_agent_grants(ctx: &InvocationCtx) -> CapabilityGrants {
    let mut grants = CapabilityGrants::default();
    for name in ctx
        .grants
        .names()
        .filter(|name| name.starts_with("agents."))
    {
        grants = grants.allow(name.to_string());
    }
    grants
}

fn message_json(message: ActorMessage) -> Value {
    json!({
        "from": message.from.as_str(),
        "to": message.to.as_str(),
        "text": message.text,
        "replyTo": message.reply_to.map(|id| id.as_str().to_string()),
        "sentAt": message.sent_at.to_rfc3339(),
    })
}

fn receipt_json(receipt: Receipt) -> Value {
    json!({
        "status": match receipt {
            Receipt::Delivered => "delivered",
            Receipt::Failed => "failed",
        }
    })
}

fn maybe_emit_message(
    roster: &MailboxRegistry,
    session_id: &str,
    message: &ActorMessage,
    receipt: Receipt,
) {
    if receipt == Receipt::Delivered {
        roster.emit_lifecycle(
            session_id,
            ActorLifecycleEvent::MessageSent {
                from: message.from.clone(),
                to: message.to.clone(),
                reply_to: message.reply_to.clone(),
                sent_at: message.sent_at,
            },
        );
    }
}

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
            role,
            task,
            mode: None,
            grants: child_agent_grants(ctx),
            budget,
            parent: parent_id,
            depth: 0,
        };

        match executor.run_to_digest(spec).await {
            Ok(digest) => {
                if let Some(content) = digest.history_content.clone() {
                    self.roster.store_transcript(&actor_id, content).await;
                }
                self.roster
                    .mark_complete_with_digest(
                        &actor_id,
                        digest.summary.clone(),
                        digest.artifact_uri.clone(),
                        digest.history_uri.clone(),
                    )
                    .await;
                self.roster.emit_lifecycle(
                    &session_id,
                    ActorLifecycleEvent::Completed {
                        actor_id: actor_id.clone(),
                        completed_at: Utc::now(),
                        summary: Some(digest.summary.clone()),
                        artifact_uri: digest.artifact_uri.clone(),
                        history_uri: digest.history_uri.clone(),
                    },
                );
                tracing::debug!(actor_id = %actor_id, "actor completed");
                Ok(json!({
                    "actorId": digest.actor_id.as_str(),
                    "summary": digest.summary,
                    "artifactUri": digest.artifact_uri,
                    "historyUri": digest.history_uri,
                }))
            }
            Err(err) => {
                let reason = map_actor_error(&err);
                tracing::warn!(actor_id = %actor_id, error = %err, "actor failed");
                self.roster.emit_lifecycle(
                    &session_id,
                    ActorLifecycleEvent::Failed {
                        actor_id: actor_id.clone(),
                        failed_at: Utc::now(),
                        reason: reason.clone(),
                    },
                );
                self.roster.mark_failed(&actor_id, reason).await;
                Err(HostError::HostCall(err.to_string()))
            }
        }
    }
}

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

        // Register each actor as Running before spawning.
        let mut actor_specs: Vec<(ActorId, ActorSpec)> = Vec::with_capacity(parsed.len());
        for (role, task_str) in parsed {
            let actor_id = self.roster.next_actor_id(&role);
            let budget = ActorBudget::default();
            let spawned_at = Utc::now();
            self.roster
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
            actor_specs.push((
                actor_id.clone(),
                ActorSpec {
                    id: actor_id,
                    session_id: session_id.clone(),
                    role,
                    task: task_str,
                    mode: None,
                    grants: child_agent_grants(ctx),
                    budget,
                    parent: parent_id.clone(),
                    depth: 0,
                },
            ));
        }

        // Fan out: spawn all concurrently, collect handles in submission order.
        let handles: Vec<tokio::task::JoinHandle<Result<Value>>> = actor_specs
            .into_iter()
            .map(|(actor_id, spec)| {
                let executor = Arc::clone(&executor);
                let roster = Arc::clone(&self.roster);
                let session_id = session_id.clone();
                tokio::spawn(async move {
                    match executor.run_to_digest(spec).await {
                        Ok(digest) => {
                            if let Some(content) = digest.history_content.clone() {
                                roster.store_transcript(&actor_id, content).await;
                            }
                            roster
                                .mark_complete_with_digest(
                                    &actor_id,
                                    digest.summary.clone(),
                                    digest.artifact_uri.clone(),
                                    digest.history_uri.clone(),
                                )
                                .await;
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
                            tracing::debug!(actor_id = %actor_id, "parallel actor completed");
                            Ok(json!({
                                "actorId": digest.actor_id.as_str(),
                                "summary": digest.summary,
                                "artifactUri": digest.artifact_uri,
                                "historyUri": digest.history_uri,
                            }))
                        }
                        Err(err) => {
                            let reason = map_actor_error(&err);
                            tracing::warn!(actor_id = %actor_id, error = %err, "parallel actor failed");
                            roster.emit_lifecycle(
                                &session_id,
                                ActorLifecycleEvent::Failed {
                                    actor_id: actor_id.clone(),
                                    failed_at: Utc::now(),
                                    reason: reason.clone(),
                                },
                            );
                            roster.mark_failed(&actor_id, reason).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use tm_host::{CapabilityGrants, InvocationCtx};

    fn make_roster() -> Arc<MailboxRegistry> {
        Arc::new(MailboxRegistry::new())
    }

    fn ctx_without_agents() -> InvocationCtx {
        InvocationCtx::new(CapabilityGrants::default())
    }

    fn ctx_with(cap: &str) -> InvocationCtx {
        InvocationCtx::new(CapabilityGrants::default().allow(cap))
    }

    fn ctx_with_actor(cap: &str, actor_id: &str) -> InvocationCtx {
        InvocationCtx::new(CapabilityGrants::default().allow(cap))
            .with_actor_id(Some(actor_id.to_string()))
    }

    async fn track_running(roster: &MailboxRegistry, id: &str) {
        roster
            .track(crate::actor::ActorRecord {
                id: ActorId::new(id).unwrap(),
                parent: None,
                status: ActorStatus::Running,
                mode: Some("worker".to_string()),
                budget: ActorBudget::default(),
                spawned_at: chrono::Utc::now(),
                completed_at: None,
                cancelled: false,
                failure_reason: None,
                last_summary: None,
                artifact_uri: None,
                history_uri: None,
            })
            .await;
    }

    #[tokio::test]
    async fn agents_run_denied_without_grant() {
        let f = AgentsRunFn::new(make_roster());
        let ctx = ctx_without_agents();
        let err = f
            .call(json!({"role": "r", "task": "t"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_RUN));
    }

    #[tokio::test]
    async fn agents_run_executor_not_configured_returns_not_implemented() {
        let f = AgentsRunFn::new(make_roster());
        let ctx = ctx_with(caps::AGENTS_RUN);
        let err = f
            .call(json!({"role": "r", "task": "t"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::NotImplemented(_)));
    }

    #[tokio::test]
    async fn agents_spawn_denied_without_grant() {
        let f = AgentsSpawnFn::new(make_roster());
        let ctx = ctx_without_agents();
        let err = f
            .call(json!({"role": "r", "task": "t"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_SPAWN));
    }

    #[tokio::test]
    async fn agents_spawn_executor_not_configured_returns_not_implemented() {
        let f = AgentsSpawnFn::new(make_roster());
        let ctx = ctx_with(caps::AGENTS_SPAWN);
        let err = f
            .call(json!({"role": "r", "task": "t"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::NotImplemented(_)));
    }

    #[tokio::test]
    async fn agents_spawn_invalid_args_returns_invalid_args_error() {
        let roster = make_roster();
        let f = AgentsSpawnFn::new(roster);
        let ctx = ctx_with(caps::AGENTS_SPAWN);
        let err = f
            .call(json!({"role": 42, "task": "t"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn agents_spawn_returns_handle_actor_runs_in_background() {
        use crate::actor::{ActorDigest, ActorId, ActorSpec, ActorStatus};
        use crate::executor::{ActorError, ActorExecutor};

        struct EchoExecutor;
        #[async_trait::async_trait]
        impl ActorExecutor for EchoExecutor {
            async fn run_to_digest(
                &self,
                spec: ActorSpec,
            ) -> std::result::Result<ActorDigest, ActorError> {
                Ok(ActorDigest {
                    actor_id: spec.id,
                    summary: format!("echo: {}", spec.task),
                    artifact_uri: None,
                    history_uri: None,
                    history_content: None,
                })
            }
        }

        let roster = Arc::new(MailboxRegistry::new());
        roster.set_executor(Arc::new(EchoExecutor));
        let f = AgentsSpawnFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_SPAWN);

        let result = f
            .call(json!({"role": "worker", "task": "do the thing"}), &ctx)
            .await
            .unwrap();

        let id_str = result["id"].as_str().expect("id field");
        let actor_id = ActorId::new(id_str).expect("valid actor id");

        // Actor is immediately tracked as Running
        let rec = roster.get(&actor_id).await.unwrap();
        assert_eq!(rec.status, ActorStatus::Running);

        // Wait for background thread to complete (EchoExecutor is trivially fast)
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let rec = roster.get(&actor_id).await.unwrap();
        assert_eq!(rec.status, ActorStatus::Terminated);
        assert!(rec.completed_at.is_some());
    }

    #[tokio::test]
    async fn agents_parallel_denied_without_grant() {
        let f = AgentsParallelFn::new(make_roster());
        let ctx = ctx_without_agents();
        let err = f
            .call(json!({"tasks": [{"role": "r", "task": "t"}]}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_PARALLEL));
    }

    #[tokio::test]
    async fn agents_parallel_executor_not_configured_returns_not_implemented() {
        let f = AgentsParallelFn::new(make_roster());
        let ctx = ctx_with(caps::AGENTS_PARALLEL);
        let err = f
            .call(json!({"tasks": [{"role": "r", "task": "t"}]}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::NotImplemented(_)));
    }

    #[tokio::test]
    async fn agents_parallel_empty_tasks_returns_empty_array() {
        let f = AgentsParallelFn::new(make_roster());
        let ctx = ctx_with(caps::AGENTS_PARALLEL);
        let result = f.call(json!({"tasks": []}), &ctx).await.unwrap();
        assert_eq!(result, json!([]));
    }

    #[tokio::test]
    async fn agents_parallel_invalid_args_returns_invalid_args_error() {
        let f = AgentsParallelFn::new(make_roster());
        let ctx = ctx_with(caps::AGENTS_PARALLEL);
        // tasks is not an array
        let err = f.call(json!({"tasks": "oops"}), &ctx).await.unwrap_err();
        assert!(matches!(err, HostError::InvalidArgs(_)));
        // task.role is not a string
        let err = f
            .call(json!({"tasks": [{"role": 99, "task": "t"}]}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn agents_parallel_runs_all_and_returns_ordered_digests() {
        use crate::actor::{ActorDigest, ActorId, ActorSpec, ActorStatus};
        use crate::executor::{ActorError, ActorExecutor};

        struct EchoExecutor;
        #[async_trait::async_trait]
        impl ActorExecutor for EchoExecutor {
            async fn run_to_digest(
                &self,
                spec: ActorSpec,
            ) -> std::result::Result<ActorDigest, ActorError> {
                Ok(ActorDigest {
                    actor_id: spec.id,
                    summary: format!("echo: {}", spec.task),
                    artifact_uri: None,
                    history_uri: None,
                    history_content: None,
                })
            }
        }

        let roster = Arc::new(MailboxRegistry::new());
        roster.set_executor(Arc::new(EchoExecutor));
        let f = AgentsParallelFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_PARALLEL);

        let result = f
            .call(
                json!({"tasks": [
                    {"role": "researcher", "task": "task A"},
                    {"role": "writer",     "task": "task B"},
                    {"role": "reviewer",   "task": "task C"},
                ]}),
                &ctx,
            )
            .await
            .unwrap();

        let digests = result.as_array().unwrap();
        assert_eq!(digests.len(), 3);

        // Results are ordered: digests[i] corresponds to tasks[i]
        assert!(digests[0]["summary"].as_str().unwrap().contains("task A"));
        assert!(digests[1]["summary"].as_str().unwrap().contains("task B"));
        assert!(digests[2]["summary"].as_str().unwrap().contains("task C"));

        // All actors have valid ids and are tracked as Terminated
        for d in digests {
            let id_str = d["actorId"].as_str().expect("actorId field");
            let actor_id = ActorId::new(id_str).expect("valid actor id");
            let rec = roster.get(&actor_id).await.unwrap();
            assert_eq!(rec.status, ActorStatus::Terminated);
        }
    }

    #[tokio::test]
    async fn agents_msg_denied_without_grant() {
        let f = AgentsMsgFn::new(make_roster());
        let ctx = ctx_without_agents();
        let err = f
            .call(json!({"handle": {"id": "W"}, "text": "hello"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_MSG));
    }

    #[tokio::test]
    async fn agents_msg_invalid_args() {
        let f = AgentsMsgFn::new(make_roster());
        let ctx = ctx_with(caps::AGENTS_MSG);

        // Non-string text
        let err = f
            .call(json!({"handle": {"id": "Worker"}, "text": 42}), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostError::InvalidArgs(_)),
            "expected InvalidArgs, got {err:?}"
        );

        // Missing handle.id
        let err = f
            .call(json!({"handle": {}, "text": "hello"}), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostError::InvalidArgs(_)),
            "expected InvalidArgs, got {err:?}"
        );

        // handle.id is not valid CamelCase (lowercase start)
        let err = f
            .call(json!({"handle": {"id": "worker"}, "text": "hello"}), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostError::InvalidArgs(_)),
            "expected InvalidArgs, got {err:?}"
        );
    }

    #[tokio::test]
    async fn agents_msg_unknown_handle_returns_null() {
        let f = AgentsMsgFn::new(make_roster());
        let ctx = ctx_with(caps::AGENTS_MSG);
        // "Nope" is CamelCase but not in roster
        let result = f
            .call(json!({"handle": {"id": "Nope"}, "text": "hi"}), &ctx)
            .await
            .unwrap();
        assert_eq!(result, Value::Null);
    }

    #[tokio::test]
    async fn agents_msg_fire_and_forget_records_and_returns_null() {
        use crate::actor::{ActorBudget, ActorId, ActorRecord, ActorStatus};
        let roster = make_roster();
        // Pre-populate a tracked actor
        roster
            .track(ActorRecord {
                id: ActorId::new("Worker").unwrap(),
                parent: None,
                status: ActorStatus::Running,
                mode: Some("worker".to_string()),
                budget: ActorBudget::default(),
                spawned_at: chrono::Utc::now(),
                completed_at: None,
                cancelled: false,
                failure_reason: None,
                last_summary: None,
                artifact_uri: None,
                history_uri: None,
            })
            .await;

        let f = AgentsMsgFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_MSG);

        let result = f
            .call(json!({"handle": {"id": "Worker"}, "text": "status?"}), &ctx)
            .await
            .unwrap();
        assert_eq!(result, Value::Null);

        let messages = roster.messages().await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from.as_str(), "Root");
        assert_eq!(messages[0].to.as_str(), "Worker");
        assert_eq!(messages[0].text, "status?");
    }

    #[tokio::test]
    async fn agents_msg_await_returns_reply() {
        use crate::actor::{
            ActorBudget, ActorDigest, ActorId, ActorRecord, ActorSpec, ActorStatus,
        };
        use crate::executor::{ActorError, ActorExecutor};

        struct EchoExecutor;
        #[async_trait::async_trait]
        impl ActorExecutor for EchoExecutor {
            async fn run_to_digest(
                &self,
                spec: ActorSpec,
            ) -> std::result::Result<ActorDigest, ActorError> {
                Ok(ActorDigest {
                    actor_id: spec.id,
                    summary: format!("echo: {}", spec.task),
                    artifact_uri: None,
                    history_uri: None,
                    history_content: None,
                })
            }
        }

        let roster = Arc::new(MailboxRegistry::new());
        roster.set_executor(Arc::new(EchoExecutor));

        // Track a completed actor with a stored summary
        let target_id = ActorId::new("ResearchWorker").unwrap();
        roster
            .track(ActorRecord {
                id: target_id.clone(),
                parent: None,
                status: ActorStatus::Terminated,
                mode: Some("researcher".to_string()),
                budget: ActorBudget::default(),
                spawned_at: chrono::Utc::now(),
                completed_at: Some(chrono::Utc::now()),
                cancelled: false,
                failure_reason: None,
                last_summary: Some("I found the answer.".to_string()),
                artifact_uri: None,
                history_uri: None,
            })
            .await;

        let f = AgentsMsgFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_MSG);

        let result = f
            .call(
                json!({"handle": {"id": "ResearchWorker"}, "text": "elaborate please", "opts": {"await": true}}),
                &ctx,
            )
            .await
            .unwrap();

        // EchoExecutor echoes the seeded task; result should contain both prior context and the new text
        let reply = result.as_str().expect("reply should be a string");
        assert!(
            reply.contains("I found the answer."),
            "reply should contain prior context"
        );
        assert!(
            reply.contains("elaborate please"),
            "reply should contain new message"
        );
    }

    #[tokio::test]
    async fn agents_msg_await_without_executor_returns_not_implemented() {
        use crate::actor::{ActorBudget, ActorId, ActorRecord, ActorStatus};
        let roster = make_roster(); // no executor set
        roster
            .track(ActorRecord {
                id: ActorId::new("Worker").unwrap(),
                parent: None,
                status: ActorStatus::Terminated,
                mode: None,
                budget: ActorBudget::default(),
                spawned_at: chrono::Utc::now(),
                completed_at: Some(chrono::Utc::now()),
                cancelled: false,
                failure_reason: None,
                last_summary: None,
                artifact_uri: None,
                history_uri: None,
            })
            .await;

        let f = AgentsMsgFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_MSG);

        let err = f
            .call(
                json!({"handle": {"id": "Worker"}, "text": "hi", "opts": {"await": true}}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostError::NotImplemented(_)),
            "expected NotImplemented, got {err:?}"
        );
    }

    #[test]
    fn caps_module_defines_mvp_set() {
        assert_eq!(caps::AGENTS_RUN, "agents.run");
        assert_eq!(caps::AGENTS_SPAWN, "agents.spawn");
        assert_eq!(caps::AGENTS_PARALLEL, "agents.parallel");
        assert_eq!(caps::AGENTS_MSG, "agents.msg");
        assert_eq!(caps::AGENTS_SEND, "agents.send");
        assert_eq!(caps::AGENTS_WAIT, "agents.wait");
        assert_eq!(caps::AGENTS_INBOX, "agents.inbox");
        assert_eq!(caps::AGENTS_LIST, "agents.list");
    }

    #[tokio::test]
    async fn agents_send_delivers_to_live_inbox() {
        let roster = make_roster();
        track_running(&roster, "Worker").await;
        let f = AgentsSendFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_SEND);

        let result = f
            .call(json!({"to": "Worker", "text": "status?"}), &ctx)
            .await
            .unwrap();

        assert_eq!(result["status"], Value::String("delivered".into()));
        assert_eq!(
            roster.unread_count(&ActorId::new("Worker").unwrap()).await,
            1
        );
        let messages = roster.messages().await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from.as_str(), "Root");
        assert_eq!(messages[0].to.as_str(), "Worker");
    }

    #[tokio::test]
    async fn agents_send_to_unknown_actor_returns_failed_receipt() {
        let f = AgentsSendFn::new(make_roster());
        let ctx = ctx_with(caps::AGENTS_SEND);

        let result = f
            .call(json!({"to": "Missing", "text": "hello"}), &ctx)
            .await
            .unwrap();

        assert_eq!(result["status"], Value::String("failed".into()));
    }

    #[tokio::test]
    async fn agents_inbox_drains_current_actor_messages() {
        let roster = make_roster();
        track_running(&roster, "Worker").await;
        roster
            .send_message(ActorMessage {
                from: ActorId::new("Root").unwrap(),
                to: ActorId::new("Worker").unwrap(),
                text: "status?".to_string(),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;

        let f = AgentsInboxFn::new(Arc::clone(&roster));
        let ctx = ctx_with_actor(caps::AGENTS_INBOX, "Worker");
        let result = f.call(json!({}), &ctx).await.unwrap();
        let messages = result.as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["from"], Value::String("Root".into()));
        assert_eq!(messages[0]["text"], Value::String("status?".into()));
        assert!(
            roster
                .drain_inbox(&ActorId::new("Worker").unwrap(), None)
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn agents_wait_returns_prequeued_root_message() {
        let roster = make_roster();
        track_running(&roster, "Worker").await;
        roster
            .send_message(ActorMessage {
                from: ActorId::new("Worker").unwrap(),
                to: ActorId::new("Root").unwrap(),
                text: "done".to_string(),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;

        let f = AgentsWaitFn::new(roster);
        let ctx = ctx_with(caps::AGENTS_WAIT);
        let result = f
            .call(json!({"from": "Worker", "timeoutMs": 50}), &ctx)
            .await
            .unwrap();

        assert_eq!(result["from"], Value::String("Worker".into()));
        assert_eq!(result["to"], Value::String("Root".into()));
        assert_eq!(result["text"], Value::String("done".into()));
    }

    #[tokio::test]
    async fn agents_wait_times_out_with_null() {
        let f = AgentsWaitFn::new(make_roster());
        let ctx = ctx_with(caps::AGENTS_WAIT);
        let result = f.call(json!({"timeoutMs": 1}), &ctx).await.unwrap();
        assert_eq!(result, Value::Null);
    }

    #[tokio::test]
    async fn agents_list_includes_unread_and_last_activity() {
        let roster = make_roster();
        track_running(&roster, "Worker").await;
        roster
            .send_message(ActorMessage {
                from: ActorId::new("Root").unwrap(),
                to: ActorId::new("Worker").unwrap(),
                text: "status?".to_string(),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;

        let f = AgentsListFn::new(roster);
        let ctx = ctx_with(caps::AGENTS_LIST);
        let result = f.call(json!({}), &ctx).await.unwrap();
        let entries = result.as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["id"], Value::String("Worker".into()));
        assert_eq!(entries[0]["status"], Value::String("running".into()));
        assert_eq!(entries[0]["unread"], Value::Number(1.into()));
        assert!(entries[0]["lastActivity"].as_str().is_some());
    }

    #[tokio::test]
    async fn agents_run_invalid_args_returns_invalid_args_error() {
        let roster = make_roster();
        let f = AgentsRunFn::new(roster);
        let ctx = ctx_with(caps::AGENTS_RUN);
        // role is not a string
        let err = f
            .call(json!({"role": 42, "task": "t"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn agents_run_with_executor_tracks_and_returns_digest() {
        use crate::actor::ActorSpec;
        use crate::actor::{ActorDigest, ActorId};
        use crate::executor::{ActorError, ActorExecutor};

        struct EchoExecutor;
        #[async_trait::async_trait]
        impl ActorExecutor for EchoExecutor {
            async fn run_to_digest(
                &self,
                spec: ActorSpec,
            ) -> std::result::Result<ActorDigest, ActorError> {
                Ok(ActorDigest {
                    actor_id: spec.id,
                    summary: format!("echo: {}", spec.task),
                    artifact_uri: None,
                    history_uri: None,
                    history_content: None,
                })
            }
        }

        let roster = Arc::new(MailboxRegistry::new());
        roster.set_executor(Arc::new(EchoExecutor));
        let f = AgentsRunFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_RUN);

        let result = f
            .call(json!({"role": "researcher", "task": "do the thing"}), &ctx)
            .await
            .unwrap();

        assert!(result["summary"].as_str().unwrap().contains("do the thing"));
        assert!(result["actorId"].as_str().is_some());

        // Actor is tracked as terminated in registry
        let actor_id_str = result["actorId"].as_str().unwrap();
        let actor_id = ActorId::new(actor_id_str).unwrap();
        let rec = roster.get(&actor_id).await.unwrap();
        assert_eq!(rec.status, crate::actor::ActorStatus::Terminated);
        assert!(rec.completed_at.is_some());
    }

    #[tokio::test]
    async fn agents_run_records_current_actor_as_parent() {
        let roster = Arc::new(MailboxRegistry::new());
        roster.set_executor(Arc::new(EchoExec));
        let f = AgentsRunFn::new(Arc::clone(&roster));
        let ctx = ctx_with_actor(caps::AGENTS_RUN, "Parent");

        let result = f
            .call(json!({"role": "worker", "task": "child task"}), &ctx)
            .await
            .unwrap();

        let actor_id = ActorId::new(result["actorId"].as_str().unwrap()).unwrap();
        let rec = roster.get(&actor_id).await.unwrap();
        assert_eq!(rec.parent.as_ref().map(|id| id.as_str()), Some("Parent"));
    }

    // ─── P3.5: lifecycle event emission tests ────────────────────────────────

    fn lifecycle_hook() -> (
        Arc<std::sync::Mutex<Vec<ActorLifecycleEvent>>>,
        impl Fn(String, ActorLifecycleEvent) + Send + Sync + 'static,
    ) {
        let captured: Arc<std::sync::Mutex<Vec<ActorLifecycleEvent>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured2 = Arc::clone(&captured);
        let hook = move |_session_id: String, event: ActorLifecycleEvent| {
            captured2.lock().unwrap().push(event);
        };
        (captured, hook)
    }

    struct EchoExec;
    #[async_trait::async_trait]
    impl crate::executor::ActorExecutor for EchoExec {
        async fn run_to_digest(
            &self,
            spec: crate::actor::ActorSpec,
        ) -> std::result::Result<crate::actor::ActorDigest, crate::executor::ActorError> {
            Ok(crate::actor::ActorDigest {
                actor_id: spec.id,
                summary: format!("echo: {}", spec.task),
                artifact_uri: None,
                history_uri: None,
                history_content: None,
            })
        }
    }

    #[tokio::test]
    async fn agents_run_emits_spawned_and_completed_lifecycle_events() {
        let roster = Arc::new(MailboxRegistry::new());
        roster.set_executor(Arc::new(EchoExec));
        let (events, hook) = lifecycle_hook();
        roster.set_lifecycle_hook(hook);

        let f = AgentsRunFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_RUN);
        f.call(json!({"role": "worker", "task": "do it"}), &ctx)
            .await
            .unwrap();

        let evs = events.lock().unwrap();
        assert_eq!(evs.len(), 2, "expected Spawned + Completed, got {evs:?}");
        assert!(matches!(&evs[0], ActorLifecycleEvent::Spawned { role, .. } if role == "worker"));
        assert!(
            matches!(&evs[1], ActorLifecycleEvent::Completed { summary, .. } if summary.as_deref() == Some("echo: do it"))
        );
    }

    #[tokio::test]
    async fn agents_run_emits_failed_lifecycle_event_on_error() {
        use crate::executor::ActorError;
        struct FailExecutor;
        #[async_trait::async_trait]
        impl crate::executor::ActorExecutor for FailExecutor {
            async fn run_to_digest(
                &self,
                _spec: crate::actor::ActorSpec,
            ) -> std::result::Result<crate::actor::ActorDigest, ActorError> {
                Err(ActorError::Execution("boom".to_string()))
            }
        }

        let roster = Arc::new(MailboxRegistry::new());
        roster.set_executor(Arc::new(FailExecutor));
        let (events, hook) = lifecycle_hook();
        roster.set_lifecycle_hook(hook);

        let f = AgentsRunFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_RUN);
        let _ = f
            .call(json!({"role": "worker", "task": "fail"}), &ctx)
            .await;

        let evs = events.lock().unwrap();
        assert_eq!(evs.len(), 2, "expected Spawned + Failed, got {evs:?}");
        assert!(matches!(&evs[0], ActorLifecycleEvent::Spawned { .. }));
        assert!(matches!(&evs[1], ActorLifecycleEvent::Failed { .. }));
    }

    #[tokio::test]
    async fn agents_parallel_emits_spawned_and_completed_for_each_actor() {
        let roster = Arc::new(MailboxRegistry::new());
        roster.set_executor(Arc::new(EchoExec));
        let (events, hook) = lifecycle_hook();
        roster.set_lifecycle_hook(hook);

        let f = AgentsParallelFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_PARALLEL);
        f.call(
            json!({"tasks": [{"role": "worker", "task": "A"}, {"role": "worker", "task": "B"}]}),
            &ctx,
        )
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        // 2 Spawned + 2 Completed = 4 total (order: Spawned A, Spawned B, then Completed in any order)
        let spawned = evs
            .iter()
            .filter(|e| matches!(e, ActorLifecycleEvent::Spawned { .. }))
            .count();
        let completed = evs
            .iter()
            .filter(|e| matches!(e, ActorLifecycleEvent::Completed { .. }))
            .count();
        assert_eq!(spawned, 2, "expected 2 Spawned events");
        assert_eq!(completed, 2, "expected 2 Completed events");
    }

    #[tokio::test]
    async fn agents_spawn_emits_spawned_and_eventually_completed() {
        let roster = Arc::new(MailboxRegistry::new());
        roster.set_executor(Arc::new(EchoExec));
        let (events, hook) = lifecycle_hook();
        roster.set_lifecycle_hook(hook);

        let f = AgentsSpawnFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_SPAWN);
        f.call(json!({"role": "worker", "task": "bg"}), &ctx)
            .await
            .unwrap();

        // Spawned event fires synchronously before returning the handle
        {
            let evs = events.lock().unwrap();
            assert!(
                matches!(evs.first(), Some(ActorLifecycleEvent::Spawned { .. })),
                "first event should be Spawned immediately"
            );
        }

        // Background thread completes quickly (EchoExec is trivial); wait for Completed
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let evs = events.lock().unwrap();
        assert_eq!(
            evs.len(),
            2,
            "expected Spawned + Completed after background completion, got {evs:?}"
        );
        assert!(matches!(&evs[1], ActorLifecycleEvent::Completed { .. }));
    }

    #[tokio::test]
    async fn lifecycle_hook_no_op_when_not_set() {
        // Verify emit_lifecycle is safe with no hook installed (no panic, no error)
        let roster = Arc::new(MailboxRegistry::new());
        roster.set_executor(Arc::new(EchoExec));
        let f = AgentsRunFn::new(Arc::clone(&roster));
        let ctx = ctx_with(caps::AGENTS_RUN);
        // Should not panic even though no lifecycle hook is set
        f.call(json!({"role": "worker", "task": "t"}), &ctx)
            .await
            .unwrap();
    }
}

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};
use tm_host::{
    CapabilityGrants, GrantDoc, HostError, HostFn, HostRegistry, InvocationCtx, ResourceRegistry,
    Result, ToolDocs, ToolErrorDoc, ToolExample,
};

use crate::actor::{ActorBudget, ActorId, ActorRecord, ActorSpec, ActorStatus};
use crate::executor::ActorError;
use crate::mailbox::{ActorMessage, MailboxRegistry};
use crate::supervise::FailureReason;

/// Capability names for the P3 MVP `agents.*` calls (§23.3, ROADMAP authority).
///
/// The §23 full surface (`pipeline`, `broadcast`, `send`, `wait`, `inbox`, `list`) is deferred
/// to P3 stretch / P3-plus and excluded from grants until the MVP gate passes.
pub mod caps {
    pub const AGENTS_RUN: &str = "agents.run";
    pub const AGENTS_SPAWN: &str = "agents.spawn";
    pub const AGENTS_PARALLEL: &str = "agents.parallel";
    pub const AGENTS_MSG: &str = "agents.msg";
}

/// Register `agents.*` HostFns and `agent://`/`history://` ResourceHandlers.
///
/// Called from `tm-server/src/main.rs` at startup. `agents.run` executes when the registry
/// holds an injected executor (set via `MailboxRegistry::set_executor` after this call).
/// `agents.spawn` and `agents.parallel` also execute with the injected executor; `agents.msg`
/// is registered and grant-checked, but returns `NotImplemented` until mailbox request/reply lands.
pub fn register(
    host_registry: &mut HostRegistry,
    resource_registry: &mut ResourceRegistry,
    roster: Arc<MailboxRegistry>,
) {
    host_registry.register(Arc::new(AgentsRunFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsSpawnFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsParallelFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsMsgFn::new(Arc::clone(&roster))));
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
        ActorError::Execution(msg) => FailureReason::Crash { message: msg.clone() },
        ActorError::InvalidSpec(msg) => FailureReason::Crash { message: msg.clone() },
        ActorError::DepthExceeded(_) => FailureReason::DepthExceeded,
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

        let executor = self
            .roster
            .executor()
            .ok_or_else(|| HostError::NotImplemented("agents.run — executor not configured".to_string()))?;

        let actor_id = self.roster.next_actor_id(&role);
        let budget = ActorBudget::default();

        let record = ActorRecord {
            id: actor_id.clone(),
            parent: None,
            status: ActorStatus::Running,
            mode: Some(role.clone()),
            budget: budget.clone(),
            spawned_at: Utc::now(),
            completed_at: None,
            cancelled: false,
            failure_reason: None,
            last_summary: None,
        };
        self.roster.track(record).await;

        tracing::debug!(actor_id = %actor_id, role = %role, "actor spawned");

        let spec = ActorSpec {
            id: actor_id.clone(),
            role,
            task,
            mode: None,
            grants: CapabilityGrants::default(),
            budget,
            parent: None,
            depth: 0,
        };

        match executor.run_to_digest(spec).await {
            Ok(digest) => {
                self.roster
                    .mark_complete_with_summary(&actor_id, digest.summary.clone())
                    .await;
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

        let executor = self
            .roster
            .executor()
            .ok_or_else(|| HostError::NotImplemented("agents.spawn — executor not configured".to_string()))?;

        let actor_id = self.roster.next_actor_id(&role);
        let budget = ActorBudget::default();

        let record = ActorRecord {
            id: actor_id.clone(),
            parent: None,
            status: ActorStatus::Running,
            mode: Some(role.clone()),
            budget: budget.clone(),
            spawned_at: Utc::now(),
            completed_at: None,
            cancelled: false,
            failure_reason: None,
            last_summary: None,
        };
        self.roster.track(record).await;

        let spec = ActorSpec {
            id: actor_id.clone(),
            role,
            task,
            mode: None,
            grants: CapabilityGrants::default(),
            budget,
            parent: None,
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
                    rt.block_on(roster.mark_complete_with_summary(&actor_id_bg, digest.summary));
                    tracing::debug!(actor_id = %actor_id_bg, "spawned actor completed");
                }
                Err(err) => {
                    let reason = map_actor_error(&err);
                    tracing::warn!(actor_id = %actor_id_bg, error = %err, "spawned actor failed");
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
                    .ok_or_else(|| HostError::InvalidArgs(format!("tasks[{i}].role must be a string")))?
                    .to_string();
                let task_str = item["task"]
                    .as_str()
                    .ok_or_else(|| HostError::InvalidArgs(format!("tasks[{i}].task must be a string")))?
                    .to_string();
                Ok((role, task_str))
            })
            .collect::<Result<_>>()?;

        let executor = self
            .roster
            .executor()
            .ok_or_else(|| HostError::NotImplemented("agents.parallel — executor not configured".to_string()))?;

        // Register each actor as Running before spawning.
        let mut actor_specs: Vec<(ActorId, ActorSpec)> = Vec::with_capacity(parsed.len());
        for (role, task_str) in parsed {
            let actor_id = self.roster.next_actor_id(&role);
            let budget = ActorBudget::default();
            self.roster
                .track(ActorRecord {
                    id: actor_id.clone(),
                    parent: None,
                    status: ActorStatus::Running,
                    mode: Some(role.clone()),
                    budget: budget.clone(),
                    spawned_at: Utc::now(),
                    completed_at: None,
                    cancelled: false,
                    failure_reason: None,
                    last_summary: None,
                })
                .await;
            actor_specs.push((
                actor_id.clone(),
                ActorSpec {
                    id: actor_id,
                    role,
                    task: task_str,
                    mode: None,
                    grants: CapabilityGrants::default(),
                    budget,
                    parent: None,
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
                tokio::spawn(async move {
                    match executor.run_to_digest(spec).await {
                        Ok(digest) => {
                            roster
                                .mark_complete_with_summary(&actor_id, digest.summary.clone())
                                .await;
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
                     Fire-and-forget (default): records the message and returns null immediately. \
                     Request/reply (opts.await = true): runs a one-shot seeded continuation from \
                     the target's last digest summary + the new text, and returns the reply string. \
                     Each opts.await call is stateless — repeated calls re-seed from the original \
                     summary, not the previous reply. \
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

        // Synthetic "Root" id for top-level orchestrator sends (§23.3).
        // Real caller-id threading through InvocationCtx is deferred to P3-plus.
        let from_id = ActorId::new("Root").expect("Root is always valid");

        // ── Fire-and-forget ───────────────────────────────────────────────────
        if !do_await {
            self.roster
                .record_message(ActorMessage {
                    from: from_id,
                    to: target_id,
                    text,
                    reply_to: None,
                    sent_at: Utc::now(),
                })
                .await;
            return Ok(Value::Null);
        }

        // ── Request/reply ─────────────────────────────────────────────────────
        // Requires executor — mirrors the pattern in agents.run / agents.spawn.
        let executor = self.roster.executor().ok_or_else(|| {
            HostError::NotImplemented("agents.msg await — executor not configured".to_string())
        })?;

        // Record the outgoing message before running the continuation.
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
        let seeded_task = format!(
            "Prior context: {prior_context}\n\nNew message: {text}"
        );

        let continuation_id = self.roster.next_actor_id(&continuation_role);
        let budget = ActorBudget::default();
        let spec = ActorSpec {
            id: continuation_id.clone(),
            role: continuation_role,
            task: seeded_task,
            mode: None,
            grants: CapabilityGrants::default(),
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

    #[tokio::test]
    async fn agents_run_denied_without_grant() {
        let f = AgentsRunFn::new(make_roster());
        let ctx = ctx_without_agents();
        let err = f.call(json!({"role": "r", "task": "t"}), &ctx).await.unwrap_err();
        assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_RUN));
    }

    #[tokio::test]
    async fn agents_run_executor_not_configured_returns_not_implemented() {
        let f = AgentsRunFn::new(make_roster());
        let ctx = ctx_with(caps::AGENTS_RUN);
        let err = f.call(json!({"role": "r", "task": "t"}), &ctx).await.unwrap_err();
        assert!(matches!(err, HostError::NotImplemented(_)));
    }

    #[tokio::test]
    async fn agents_spawn_denied_without_grant() {
        let f = AgentsSpawnFn::new(make_roster());
        let ctx = ctx_without_agents();
        let err = f.call(json!({"role": "r", "task": "t"}), &ctx).await.unwrap_err();
        assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_SPAWN));
    }

    #[tokio::test]
    async fn agents_spawn_executor_not_configured_returns_not_implemented() {
        let f = AgentsSpawnFn::new(make_roster());
        let ctx = ctx_with(caps::AGENTS_SPAWN);
        let err = f.call(json!({"role": "r", "task": "t"}), &ctx).await.unwrap_err();
        assert!(matches!(err, HostError::NotImplemented(_)));
    }

    #[tokio::test]
    async fn agents_spawn_invalid_args_returns_invalid_args_error() {
        let roster = make_roster();
        let f = AgentsSpawnFn::new(roster);
        let ctx = ctx_with(caps::AGENTS_SPAWN);
        let err = f.call(json!({"role": 42, "task": "t"}), &ctx).await.unwrap_err();
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
        assert!(
            matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_PARALLEL)
        );
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
        assert!(matches!(err, HostError::InvalidArgs(_)), "expected InvalidArgs, got {err:?}");

        // Missing handle.id
        let err = f
            .call(json!({"handle": {}, "text": "hello"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::InvalidArgs(_)), "expected InvalidArgs, got {err:?}");

        // handle.id is not valid CamelCase (lowercase start)
        let err = f
            .call(json!({"handle": {"id": "worker"}, "text": "hello"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::InvalidArgs(_)), "expected InvalidArgs, got {err:?}");
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
        use crate::actor::{ActorBudget, ActorDigest, ActorId, ActorRecord, ActorSpec, ActorStatus};
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
        assert!(reply.contains("I found the answer."), "reply should contain prior context");
        assert!(reply.contains("elaborate please"), "reply should contain new message");
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
        assert!(matches!(err, HostError::NotImplemented(_)), "expected NotImplemented, got {err:?}");
    }

    #[test]
    fn caps_module_defines_mvp_set() {
        assert_eq!(caps::AGENTS_RUN, "agents.run");
        assert_eq!(caps::AGENTS_SPAWN, "agents.spawn");
        assert_eq!(caps::AGENTS_PARALLEL, "agents.parallel");
        assert_eq!(caps::AGENTS_MSG, "agents.msg");
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
        use crate::actor::{ActorDigest, ActorId};
        use crate::actor::ActorSpec;
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
}

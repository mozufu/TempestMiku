use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};
use tm_host::{
    CapabilityGrants, GrantDoc, HostError, HostFn, HostRegistry, InvocationCtx, ResourceRegistry,
    Result, ToolDocs, ToolErrorDoc, ToolExample,
};

use crate::actor::{ActorBudget, ActorRecord, ActorSpec, ActorStatus};
use crate::executor::ActorError;
use crate::mailbox::MailboxRegistry;
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
/// `agents.spawn`, `agents.parallel`, and `agents.msg` return `NotImplemented` until P3.2.
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
                self.roster.mark_complete(&actor_id).await;
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
    #[allow(dead_code)]
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
                     Requires agents.spawn grant. Implementation deferred to P3.2."
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

    async fn call(&self, _args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_SPAWN);
        Err(HostError::NotImplemented("agents.spawn — P3.2".to_string()))
    }
}

// ─── agents.parallel ──────────────────────────────────────────────────────────

pub struct AgentsParallelFn {
    docs: ToolDocs,
    #[allow(dead_code)]
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
                     Requires agents.parallel grant. Implementation deferred to P3.2."
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

    async fn call(&self, _args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_PARALLEL);
        Err(HostError::NotImplemented(
            "agents.parallel — P3.2".to_string(),
        ))
    }
}

// ─── agents.msg ───────────────────────────────────────────────────────────────

pub struct AgentsMsgFn {
    docs: ToolDocs,
    #[allow(dead_code)]
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
                    "Sends a plain-prose message to a spawned actor handle. Fire-and-forget by \
                     default; set opts.await = true for request/reply. Messages are plain prose — \
                     never control-payload blobs. Pass large payloads by reference \
                     (artifact://, memory://). A null reply means unreachable — do not retry-loop. \
                     Requires agents.msg grant. Implementation deferred to P3.2."
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

    async fn call(&self, _args: Value, ctx: &InvocationCtx) -> Result<Value> {
        check_grant!(ctx, caps::AGENTS_MSG);
        Err(HostError::NotImplemented("agents.msg — P3.2".to_string()))
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
    async fn agents_msg_denied_without_grant() {
        let f = AgentsMsgFn::new(make_roster());
        let ctx = ctx_without_agents();
        let err = f
            .call(json!({"handle": {"id": "W"}, "text": "hello"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::CapabilityDenied(ref s) if s == caps::AGENTS_MSG));
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

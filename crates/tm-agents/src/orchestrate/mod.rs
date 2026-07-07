use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
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
    ActorBudget, ActorDigest, ActorId, ActorLifecycleEvent, ActorRecord, ActorSpec, ActorStatus,
};
use crate::executor::{ActorError, failure_reason_for_error, run_to_digest_with_budget};
use crate::mailbox::{ActorMessage, MailboxRegistry, Receipt};
use crate::supervise::{
    FailureReason, RestartStrategy, SupervisionAction, SupervisionDecision, SupervisionPolicy,
};

/// Capability names for the P3/P3-plus `agents.*` calls (§23.3, ROADMAP authority).
///
/// Active supervision, wall-clock budgets, subtree cancellation, and fuller provenance remain later
/// P3-plus work.
pub mod caps {
    pub const AGENTS_RUN: &str = "agents.run";
    pub const AGENTS_SPAWN: &str = "agents.spawn";
    pub const AGENTS_PARALLEL: &str = "agents.parallel";
    pub const AGENTS_PIPELINE: &str = "agents.pipeline";
    pub const AGENTS_MSG: &str = "agents.msg";
    pub const AGENTS_SEND: &str = "agents.send";
    pub const AGENTS_BROADCAST: &str = "agents.broadcast";
    pub const AGENTS_CANCEL: &str = "agents.cancel";
    pub const AGENTS_WAIT: &str = "agents.wait";
    pub const AGENTS_INBOX: &str = "agents.inbox";
    pub const AGENTS_LIST: &str = "agents.list";
}

macro_rules! check_grant {
    ($ctx:expr, $cap:expr) => {
        if !$ctx.grants.permits($cap) {
            return Err(HostError::CapabilityDenied($cap.to_string()));
        }
    };
}

mod mailbox_fns;
mod msg;
mod parallel;
mod pipeline;
mod run;
mod spawn;

#[cfg(test)]
mod tests;

use mailbox_fns::{
    AgentsBroadcastFn, AgentsCancelFn, AgentsInboxFn, AgentsListFn, AgentsSendFn, AgentsWaitFn,
};
use msg::AgentsMsgFn;
use parallel::AgentsParallelFn;
use pipeline::AgentsPipelineFn;
use run::AgentsRunFn;
use spawn::AgentsSpawnFn;

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
    host_registry.register(Arc::new(AgentsPipelineFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsMsgFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsSendFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsBroadcastFn::new(Arc::clone(&roster))));
    host_registry.register(Arc::new(AgentsCancelFn::new(Arc::clone(&roster))));
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

async fn run_actor_to_digest(
    executor: Arc<dyn crate::executor::ActorExecutor>,
    spec: ActorSpec,
) -> std::result::Result<crate::actor::ActorDigest, ActorError> {
    run_to_digest_with_budget(executor, spec).await
}

async fn mark_actor_error(
    roster: &Arc<MailboxRegistry>,
    session_id: &str,
    actor_id: &ActorId,
    reason: FailureReason,
) {
    let Some(decision) = roster
        .record_actor_error(session_id, actor_id, reason)
        .await
    else {
        return;
    };
    spawn_restart_actions(Arc::clone(roster), decision).await;
}

async fn spawn_restart_actions(roster: Arc<MailboxRegistry>, decision: SupervisionDecision) {
    for action in decision.actions {
        if let SupervisionAction::Restart { actor_id } = action {
            spawn_restarted_actor(Arc::clone(&roster), actor_id).await;
        }
    }
}

async fn spawn_restarted_actor(roster: Arc<MailboxRegistry>, actor_id: ActorId) {
    let Some(executor) = roster.executor() else {
        return;
    };
    let Some(spec) = roster.prepare_actor_restart(&actor_id).await else {
        return;
    };
    let session_id = spec.session_id.clone();
    let actor_id_bg = actor_id.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("restart worker runtime");
        rt.block_on(async move {
            match run_actor_to_digest(executor, spec).await {
                Ok(digest) => {
                    mark_actor_completed(&roster, &session_id, &actor_id_bg, digest).await;
                    tracing::debug!(actor_id = %actor_id_bg, "restarted actor completed");
                }
                Err(err) => {
                    let reason = failure_reason_for_error(&err);
                    tracing::warn!(
                        actor_id = %actor_id_bg,
                        error = %err,
                        "restarted actor failed"
                    );
                    mark_actor_error(&roster, &session_id, &actor_id_bg, reason).await;
                }
            }
        });
    });
}

async fn mark_actor_completed(
    roster: &MailboxRegistry,
    session_id: &str,
    actor_id: &ActorId,
    digest: ActorDigest,
) -> bool {
    if let Some(content) = digest.history_content.clone() {
        roster.store_transcript(actor_id, content).await;
    }
    let completed = roster
        .mark_complete_with_digest(
            actor_id,
            digest.summary.clone(),
            digest.artifact_uri.clone(),
            digest.history_uri.clone(),
        )
        .await;
    if completed {
        roster.emit_lifecycle(
            session_id,
            ActorLifecycleEvent::StatusChanged {
                actor_id: actor_id.clone(),
                status: ActorStatus::Terminated,
                at: Utc::now(),
            },
        );
        roster.emit_lifecycle(
            session_id,
            ActorLifecycleEvent::Completed {
                actor_id: actor_id.clone(),
                completed_at: Utc::now(),
                summary: Some(digest.summary),
                artifact_uri: digest.artifact_uri,
                history_uri: digest.history_uri,
            },
        );
    }
    completed
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

fn parse_plain_prose_text(args: &Value, field: &str) -> Result<String> {
    let text = args
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::InvalidArgs(format!("{field} must be a string")))?;
    validate_plain_prose_text(text, field)?;
    Ok(text.to_string())
}

fn validate_plain_prose_text(text: &str, field: &str) -> Result<()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(HostError::InvalidArgs(format!(
            "{field} must be non-empty plain prose"
        )));
    }

    if matches!(
        serde_json::from_str::<Value>(trimmed),
        Ok(Value::Object(_)) | Ok(Value::Array(_))
    ) {
        return Err(HostError::InvalidArgs(format!(
            "{field} must be plain prose, not a JSON control payload"
        )));
    }

    Ok(())
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

async fn reject_descendant_wait(
    roster: &MailboxRegistry,
    waiter: &ActorId,
    target: &ActorId,
    field: &str,
) -> Result<()> {
    if roster.would_wait_on_descendant(waiter, target).await {
        return Err(HostError::InvalidArgs(format!(
            "{field} would make actor {waiter} wait on itself or its own descendant {target}"
        )));
    }
    Ok(())
}

async fn configure_supervision_from_opts(
    roster: &MailboxRegistry,
    parent_id: Option<&ActorId>,
    opts: Option<&Value>,
) -> Result<Option<ActorId>> {
    let Some(supervision) = opts.and_then(|opts| opts.get("supervision")) else {
        return Ok(None);
    };
    let object = supervision
        .as_object()
        .ok_or_else(|| HostError::InvalidArgs("opts.supervision must be an object".to_string()))?;

    let group = object.get("group").and_then(Value::as_str);
    let strategy = object
        .get("strategy")
        .and_then(Value::as_str)
        .map(parse_restart_strategy)
        .transpose()?
        .unwrap_or_else(|| {
            if group.is_some() {
                RestartStrategy::OneForAll
            } else {
                RestartStrategy::OneForOne
            }
        });
    let default_max_restarts = if group.is_some() { 0 } else { 3 };
    let max_restarts = object
        .get("maxRestarts")
        .map(parse_u32_value)
        .transpose()?
        .unwrap_or(default_max_restarts);

    let supervisor_id = match group {
        Some(group) => spawn_group_supervisor_id(parent_id, group)?,
        None => parent_id.cloned().unwrap_or_else(root_actor_id),
    };
    roster
        .set_supervision_policy(
            supervisor_id.clone(),
            SupervisionPolicy {
                strategy,
                max_restarts,
            },
        )
        .await;
    Ok(Some(supervisor_id))
}

fn parse_restart_strategy(value: &str) -> Result<RestartStrategy> {
    match value {
        "one_for_one" => Ok(RestartStrategy::OneForOne),
        "one_for_all" => Ok(RestartStrategy::OneForAll),
        "rest_for_one" => Ok(RestartStrategy::RestForOne),
        _ => Err(HostError::InvalidArgs(format!(
            "unknown supervision strategy: {value}"
        ))),
    }
}

fn parse_u32_value(value: &Value) -> Result<u32> {
    let raw = value.as_u64().ok_or_else(|| {
        HostError::InvalidArgs("maxRestarts must be a non-negative integer".to_string())
    })?;
    u32::try_from(raw).map_err(|_| HostError::InvalidArgs("maxRestarts is too large".to_string()))
}

fn spawn_group_supervisor_id(parent_id: Option<&ActorId>, group: &str) -> Result<ActorId> {
    if group.trim().is_empty() {
        return Err(HostError::InvalidArgs(
            "opts.supervision.group must not be empty".to_string(),
        ));
    }
    let mut hasher = DefaultHasher::new();
    parent_id.map(ActorId::as_str).hash(&mut hasher);
    group.hash(&mut hasher);
    ActorId::new(format!("Supervisor{:016x}", hasher.finish()))
        .map_err(|err| HostError::InvalidArgs(format!("invalid supervision group: {err}")))
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
    if receipt.is_delivered() {
        json!({ "status": "delivered" })
    } else {
        json!({
            "status": "failed",
            "reason": receipt.failure_reason().unwrap_or("unknown"),
        })
    }
}

async fn wait_for_actor_message_or_cancel(
    roster: &MailboxRegistry,
    actor_id: &ActorId,
    from: Option<&ActorId>,
    timeout: Duration,
) -> Result<Option<ActorMessage>> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if roster.is_cancelled(actor_id).await {
            return Err(HostError::HostCall("actor cancelled".to_string()));
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        let remaining = deadline.saturating_duration_since(now);
        let slice = remaining.min(Duration::from_millis(50));
        if let Some(message) = roster.wait_for_message(actor_id, from, slice).await {
            return Ok(Some(message));
        }
    }
}

fn maybe_emit_message(
    roster: &MailboxRegistry,
    session_id: &str,
    message: &ActorMessage,
    receipt: Receipt,
) {
    if receipt.is_delivered() {
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

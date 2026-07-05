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
mod run;
mod spawn;

#[cfg(test)]
mod tests;

use mailbox_fns::{AgentsInboxFn, AgentsListFn, AgentsSendFn, AgentsWaitFn};
use msg::AgentsMsgFn;
use parallel::AgentsParallelFn;
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

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tm_host::{ApprovalDecision as HostApprovalDecision, ApprovalPolicy, HostError};
use uuid::Uuid;

use crate::{
    ApprovalBroker, ApprovalOption, ApprovalPrompt, ApprovalStatus, CodingEventSink,
    DetailedApprovalOutcome, Result, ServerError,
};

const NATIVE_TM_BACKEND: &str = "native-tm";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeApprovalMode {
    Deny,
    Manual,
}

impl NativeApprovalMode {
    pub fn parse(mode: &str) -> Result<Self> {
        match mode {
            "" | "deny" => Ok(Self::Deny),
            "manual" => Ok(Self::Manual),
            other => Err(ServerError::InvalidRequest(format!(
                "unsupported approval mode {other}"
            ))),
        }
    }
}
pub struct HttpApprovalPolicy {
    broker: Arc<ApprovalBroker>,
    session_id: Uuid,
    sink: Arc<dyn CodingEventSink>,
    actor_id: Option<String>,
}

impl HttpApprovalPolicy {
    pub fn new(
        broker: Arc<ApprovalBroker>,
        session_id: Uuid,
        sink: Arc<dyn CodingEventSink>,
    ) -> Self {
        Self {
            broker,
            session_id,
            sink,
            actor_id: None,
        }
    }

    pub fn with_actor_id(mut self, actor_id: Option<impl Into<String>>) -> Self {
        self.actor_id = actor_id.map(Into::into);
        self
    }
}

#[async_trait]
impl ApprovalPolicy for HttpApprovalPolicy {
    async fn request(
        &self,
        action: &str,
        timeout: std::time::Duration,
    ) -> tm_host::Result<HostApprovalDecision> {
        let action = action.to_string();
        let detailed = self
            .broker
            .request_permission_detailed_for_backend(
                self.session_id,
                NATIVE_TM_BACKEND,
                approval_prompt(&action, self.actor_id.as_deref()),
                timeout,
                Arc::clone(&self.sink),
            )
            .await
            .map_err(|err| HostError::HostCall(err.to_string()))?;
        host_decision(&action, detailed)
    }
}

fn approval_prompt(action: &str, actor_id: Option<&str>) -> ApprovalPrompt {
    let mut scope = serde_json::Map::new();
    scope.insert("action".to_string(), json!(action));
    scope.insert(
        "capability".to_string(),
        json!(action.split_whitespace().next().unwrap_or(action)),
    );
    if let Some(actor_id) = actor_id {
        scope.insert("actorId".to_string(), json!(actor_id));
    }
    ApprovalPrompt {
        action: action.to_string(),
        scope: Value::Object(scope),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Allow once".to_string(),
                kind: "allow_once".to_string(),
            },
            ApprovalOption {
                option_id: "reject".to_string(),
                name: "Reject once".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}

fn host_decision(
    action: &str,
    detailed: DetailedApprovalOutcome,
) -> tm_host::Result<HostApprovalDecision> {
    match detailed.status {
        ApprovalStatus::Approved => Ok(HostApprovalDecision::Approved),
        ApprovalStatus::Denied | ApprovalStatus::Cancelled => Ok(HostApprovalDecision::Denied),
        ApprovalStatus::TimedOut => Err(HostError::ApprovalTimeout(action.to_string())),
    }
}

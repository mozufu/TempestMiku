use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot};
use uuid::Uuid;

use crate::{
    ApprovalRequestRecord, NewApprovalRequest, NewApprovalResolution, Result, ServerError,
    SessionEvent, Store,
};

#[async_trait]
pub trait CodingBackend: Send + Sync + 'static {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<CodingTurnResult>;
}

#[derive(Debug, Clone)]
pub struct CodingTurn {
    pub session_id: Uuid,
    pub user_prompt: String,
    pub system_prompt: String,
    pub mode: tm_modes::ModeId,
    pub scope: String,
    /// Exact capabilities declared for this turn (e.g. `["agents.*", "backend.coding"]`).
    /// The sandbox replaces its turn grants with this set plus the fixed core grants; `.*`
    /// capability patterns remain supported.
    pub capabilities: Vec<String>,
    /// Caller-bounded persisted conversation history, ordered oldest to newest.
    pub prior_messages: Vec<tm_core::Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodingTurnResult {
    pub final_text: String,
    pub transcript_artifact: Option<tm_artifacts::ArtifactRef>,
}

#[async_trait]
pub trait CodingEventSink: Send + Sync + 'static {
    async fn emit(&self, event_type: &str, payload_json: Value) -> Result<SessionEvent>;

    async fn publish_persisted(&self, _event: SessionEvent) -> Result<()> {
        Ok(())
    }

    fn turn_id(&self) -> Option<Uuid> {
        None
    }
}

pub struct StoreCodingEventSink<S> {
    session_id: Uuid,
    turn_id: Option<Uuid>,
    store: Arc<S>,
    sender: broadcast::Sender<SessionEvent>,
}

impl<S> StoreCodingEventSink<S> {
    pub fn new(session_id: Uuid, store: Arc<S>, sender: broadcast::Sender<SessionEvent>) -> Self {
        Self {
            session_id,
            turn_id: None,
            store,
            sender,
        }
    }

    pub fn for_turn(
        session_id: Uuid,
        turn_id: Uuid,
        store: Arc<S>,
        sender: broadcast::Sender<SessionEvent>,
    ) -> Self {
        Self {
            session_id,
            turn_id: Some(turn_id),
            store,
            sender,
        }
    }
}

#[async_trait]
impl<S> CodingEventSink for StoreCodingEventSink<S>
where
    S: Store,
{
    async fn emit(&self, event_type: &str, payload_json: Value) -> Result<SessionEvent> {
        let event = self
            .store
            .append_event_for_turn(self.session_id, event_type, payload_json, self.turn_id)
            .await?;
        let _ = self.sender.send(event.clone());
        Ok(event)
    }

    async fn publish_persisted(&self, event: SessionEvent) -> Result<()> {
        let _ = self.sender.send(event);
        Ok(())
    }

    fn turn_id(&self) -> Option<Uuid> {
        self.turn_id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPrompt {
    pub action: String,
    pub scope: Value,
    pub options: Vec<ApprovalOption>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResolveDecision {
    Approve,
    Deny,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveApprovalRequest {
    pub decision: ApprovalResolveDecision,
    #[serde(default)]
    pub option_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Selected { option_id: String },
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Approved,
    Denied,
    TimedOut,
    Cancelled,
}

impl ApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailedApprovalOutcome {
    pub outcome: ApprovalOutcome,
    pub status: ApprovalStatus,
}

pub struct DurableApprovalSpec {
    pub session_id: Uuid,
    pub origin: String,
    pub prompt: ApprovalPrompt,
    pub timeout: Duration,
    pub effect_type: String,
    pub effect_payload_json: Value,
    pub resumable: bool,
    pub sink: Arc<dyn CodingEventSink>,
}

pub struct ApprovalBroker {
    pending: Mutex<BTreeMap<Uuid, PendingApproval>>,
    store: RwLock<Option<Arc<dyn Store>>>,
    instance_id: Uuid,
}

struct PendingApproval {
    session_id: Uuid,
    sender: oneshot::Sender<ResolveApprovalRequest>,
}

impl Default for ApprovalBroker {
    fn default() -> Self {
        Self {
            pending: Mutex::new(BTreeMap::new()),
            store: RwLock::new(None),
            instance_id: Uuid::new_v4(),
        }
    }
}

impl ApprovalBroker {
    pub fn bind_store<S>(&self, store: Arc<S>)
    where
        S: Store,
    {
        let store: Arc<dyn Store> = store;
        *self.store.write() = Some(Arc::clone(&store));
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            // Snapshot startup before spawning. A starved cleanup task must never classify
            // approvals created after binding as stale or expired when it eventually runs.
            let startup_at = Utc::now();
            runtime.spawn(async move {
                let stale_before = startup_at - chrono::Duration::seconds(30);
                if let Err(error) = store
                    .cancel_stale_non_resumable_approvals(stale_before, startup_at)
                    .await
                {
                    let error = tm_memory::redact_dream_text(&error.to_string()).text;
                    tracing::warn!(%error, "failed to cancel stale durable approvals");
                }
                if let Err(error) = store.expire_pending_approvals(startup_at).await {
                    let error = tm_memory::redact_dream_text(&error.to_string()).text;
                    tracing::warn!(%error, "failed to expire durable approvals");
                }
            });
        }
    }

    pub async fn request_permission(
        &self,
        session_id: Uuid,
        prompt: ApprovalPrompt,
        timeout: Duration,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<ApprovalOutcome> {
        self.request_permission_for_backend(session_id, "omp-acp", prompt, timeout, sink)
            .await
    }

    pub async fn request_permission_for_backend(
        &self,
        session_id: Uuid,
        backend: &str,
        prompt: ApprovalPrompt,
        timeout: Duration,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<ApprovalOutcome> {
        Ok(self
            .request_permission_detailed_for_backend(session_id, backend, prompt, timeout, sink)
            .await?
            .outcome)
    }

    pub async fn enqueue_permission_for_backend(&self, spec: DurableApprovalSpec) -> Result<Uuid> {
        let DurableApprovalSpec {
            session_id,
            origin,
            prompt,
            timeout,
            effect_type,
            effect_payload_json,
            resumable,
            sink,
        } = spec;
        let durable_store = { self.store.read().clone() };
        let store = durable_store
            .ok_or_else(|| ServerError::Store("durable approval store is not bound".to_string()))?;
        let approval_id = Uuid::new_v4();
        let created_at = Utc::now();
        let expires_at = created_at
            + chrono::Duration::from_std(timeout)
                .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        store
            .create_approval_request(NewApprovalRequest {
                id: approval_id,
                session_id,
                turn_id: sink.turn_id(),
                requester_id: self.instance_id,
                origin: origin.clone(),
                action: prompt.action.clone(),
                scope_json: prompt.scope.clone(),
                options_json: serde_json::to_value(&prompt.options)?,
                effect_type,
                effect_payload_json,
                resumable,
                created_at,
                expires_at,
            })
            .await?;
        let event = sink
            .emit(
                "approval",
                json!({
                    "approvalId": approval_id,
                    "backend": origin,
                    "action": prompt.action,
                    "scope": prompt.scope,
                    "options": prompt.options,
                    "timeoutMs": timeout.as_millis(),
                    "expiresAt": expires_at,
                    "resumable": resumable,
                }),
            )
            .await?;
        store
            .link_approval_event(session_id, approval_id, "approval", event.seq)
            .await?;
        Ok(approval_id)
    }

    pub async fn request_permission_detailed_for_backend(
        &self,
        session_id: Uuid,
        backend: &str,
        prompt: ApprovalPrompt,
        timeout: Duration,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<DetailedApprovalOutcome> {
        let durable_store = { self.store.read().clone() };
        if let Some(store) = durable_store {
            let effect_payload_json = json!({
                "origin": backend,
                "action": prompt.action.clone(),
                "scope": prompt.scope.clone(),
            });
            return self
                .request_permission_durable(
                    store,
                    DurableApprovalSpec {
                        session_id,
                        origin: backend.to_string(),
                        prompt,
                        timeout,
                        effect_type: "approval_continuation".to_string(),
                        effect_payload_json,
                        resumable: false,
                        sink,
                    },
                )
                .await
                .map(|(_, outcome)| outcome);
        }
        let approval_id = Uuid::new_v4();
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .insert(approval_id, PendingApproval { session_id, sender });

        let approval_payload = json!({
            "approvalId": approval_id,
            "backend": backend,
            "action": prompt.action,
            "scope": prompt.scope,
            "options": prompt.options,
            "timeoutMs": timeout.as_millis(),
        });
        if let Err(err) = sink.emit("approval", approval_payload).await {
            self.pending.lock().remove(&approval_id);
            return Err(err);
        }

        enum RequestState {
            Resolved(ResolveApprovalRequest),
            Closed,
            TimedOut,
        }

        let request = match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(request)) => RequestState::Resolved(request),
            Ok(Err(_closed)) => RequestState::Closed,
            Err(_elapsed) => {
                self.pending.lock().remove(&approval_id);
                RequestState::TimedOut
            }
        };

        let detailed = match request {
            RequestState::Resolved(request) => {
                let outcome = self.resolve_outcome(&prompt, request);
                DetailedApprovalOutcome {
                    status: status_for_outcome(&prompt, &outcome),
                    outcome,
                }
            }
            RequestState::Closed => {
                let outcome = self.reject_outcome(&prompt);
                DetailedApprovalOutcome {
                    status: ApprovalStatus::Cancelled,
                    outcome,
                }
            }
            RequestState::TimedOut => {
                let outcome = self.reject_outcome(&prompt);
                DetailedApprovalOutcome {
                    status: ApprovalStatus::TimedOut,
                    outcome,
                }
            }
        };
        self.emit_resolution(approval_id, backend, &detailed, sink)
            .await?;
        Ok(detailed)
    }

    pub async fn request_permission_detailed_with_effect_for_backend(
        &self,
        spec: DurableApprovalSpec,
    ) -> Result<(Uuid, DetailedApprovalOutcome)> {
        let durable_store = { self.store.read().clone() };
        let store = durable_store
            .ok_or_else(|| ServerError::Store("durable approval store is not bound".to_string()))?;
        self.request_permission_durable(store, spec).await
    }

    async fn request_permission_durable(
        &self,
        store: Arc<dyn Store>,
        spec: DurableApprovalSpec,
    ) -> Result<(Uuid, DetailedApprovalOutcome)> {
        let DurableApprovalSpec {
            session_id,
            origin,
            prompt,
            timeout,
            effect_type,
            effect_payload_json,
            resumable,
            sink,
        } = spec;
        let approval_id = Uuid::new_v4();
        let created_at = Utc::now();
        let expires_at = created_at
            + chrono::Duration::from_std(timeout)
                .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        store
            .create_approval_request(NewApprovalRequest {
                id: approval_id,
                session_id,
                turn_id: sink.turn_id(),
                requester_id: self.instance_id,
                origin: origin.clone(),
                action: prompt.action.clone(),
                scope_json: prompt.scope.clone(),
                options_json: serde_json::to_value(&prompt.options)?,
                effect_type,
                effect_payload_json,
                resumable,
                created_at,
                expires_at,
            })
            .await?;

        let approval_payload = json!({
            "approvalId": approval_id,
            "backend": origin,
            "action": prompt.action,
            "scope": prompt.scope,
            "options": prompt.options,
            "timeoutMs": timeout.as_millis(),
            "expiresAt": expires_at,
            "resumable": resumable,
        });
        let approval_event = match sink.emit("approval", approval_payload).await {
            Ok(event) => event,
            Err(error) => {
                let resolution_json = json!({
                    "approvalId": approval_id,
                    "backend": origin,
                    "status": "cancelled",
                    "outcome": "cancelled",
                    "reason": "event_emit_failed",
                });
                if let Ok((_, event)) = store
                    .resolve_approval_request_with_event(
                        session_id,
                        approval_id,
                        NewApprovalResolution {
                            status: "cancelled".to_string(),
                            selected_option_id: None,
                            resolution_json,
                            resolved_at: Utc::now(),
                        },
                    )
                    .await
                {
                    let _ = sink.publish_persisted(event).await;
                }
                return Err(error);
            }
        };
        store
            .link_approval_event(session_id, approval_id, "approval", approval_event.seq)
            .await?;

        let mut last_heartbeat = created_at;
        loop {
            let record = store.approval_request(session_id, approval_id).await?;
            if record.status != "pending" {
                let detailed = detailed_from_record(&record)?;
                return Ok((approval_id, detailed));
            }

            let now = Utc::now();
            if now >= record.expires_at {
                let outcome = self.reject_outcome(&prompt);
                let detailed = DetailedApprovalOutcome {
                    outcome,
                    status: ApprovalStatus::TimedOut,
                };
                let resolution_json = resolution_payload(approval_id, &origin, &detailed);
                let selected_option_id = selected_option_id(&detailed.outcome);
                match store
                    .resolve_approval_request_with_event(
                        session_id,
                        approval_id,
                        NewApprovalResolution {
                            status: detailed.status.as_str().to_string(),
                            selected_option_id,
                            resolution_json,
                            resolved_at: now,
                        },
                    )
                    .await
                {
                    Ok((_, event)) => {
                        sink.publish_persisted(event).await?;
                        return Ok((approval_id, detailed));
                    }
                    Err(ServerError::Conflict(_)) => continue,
                    Err(error) => return Err(error),
                }
            }

            if now - last_heartbeat >= chrono::Duration::seconds(1) {
                store
                    .heartbeat_approval_request(approval_id, self.instance_id, now)
                    .await?;
                last_heartbeat = now;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub async fn resolve_persisted(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        request: ResolveApprovalRequest,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<()> {
        let durable_store = { self.store.read().clone() };
        let Some(store) = durable_store else {
            return self.resolve(session_id, approval_id, request);
        };
        let record = store.approval_request(session_id, approval_id).await?;
        if record.status != "pending" {
            return Err(ServerError::Conflict(format!(
                "approval {approval_id} is already resolved"
            )));
        }
        let prompt = prompt_from_record(&record)?;
        let detailed = if Utc::now() >= record.expires_at {
            DetailedApprovalOutcome {
                outcome: self.reject_outcome(&prompt),
                status: ApprovalStatus::TimedOut,
            }
        } else {
            let outcome = self.resolve_outcome(&prompt, request);
            DetailedApprovalOutcome {
                status: status_for_outcome(&prompt, &outcome),
                outcome,
            }
        };
        let resolved_at = Utc::now();
        let (_, event) = store
            .resolve_approval_request_with_event(
                session_id,
                approval_id,
                NewApprovalResolution {
                    status: detailed.status.as_str().to_string(),
                    selected_option_id: selected_option_id(&detailed.outcome),
                    resolution_json: resolution_payload(approval_id, &record.origin, &detailed),
                    resolved_at,
                },
            )
            .await?;
        sink.publish_persisted(event).await?;
        Ok(())
    }

    pub fn resolve(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        request: ResolveApprovalRequest,
    ) -> Result<()> {
        let pending = {
            let mut pending = self.pending.lock();
            match pending.get(&approval_id) {
                Some(pending) if pending.session_id != session_id => {
                    return Err(ServerError::InvalidRequest(format!(
                        "approval {approval_id} does not belong to session {session_id}"
                    )));
                }
                Some(_) => pending.remove(&approval_id),
                None => None,
            }
        }
        .ok_or_else(|| ServerError::InvalidRequest(format!("unknown approval {approval_id}")))?;

        pending.sender.send(request).map_err(|_| {
            ServerError::InvalidRequest(format!("approval {approval_id} is no longer pending"))
        })
    }

    fn resolve_outcome(
        &self,
        prompt: &ApprovalPrompt,
        request: ResolveApprovalRequest,
    ) -> ApprovalOutcome {
        match request.decision {
            ApprovalResolveDecision::Approve => {
                if let Some(option_id) = request.option_id {
                    if prompt
                        .options
                        .iter()
                        .any(|option| option.option_id == option_id)
                    {
                        return ApprovalOutcome::Selected { option_id };
                    }
                    return ApprovalOutcome::Cancelled;
                }
                self.allow_outcome(prompt)
            }
            ApprovalResolveDecision::Deny => self.reject_outcome(prompt),
        }
    }

    fn allow_outcome(&self, prompt: &ApprovalPrompt) -> ApprovalOutcome {
        select_option(prompt, |kind| kind == "allow_once")
            .or_else(|| select_option(prompt, |kind| kind.starts_with("allow_")))
            .map_or(ApprovalOutcome::Cancelled, |option_id| {
                ApprovalOutcome::Selected { option_id }
            })
    }

    fn reject_outcome(&self, prompt: &ApprovalPrompt) -> ApprovalOutcome {
        select_option(prompt, |kind| kind == "reject_once")
            .or_else(|| select_option(prompt, |kind| kind.starts_with("reject_")))
            .map_or(ApprovalOutcome::Cancelled, |option_id| {
                ApprovalOutcome::Selected { option_id }
            })
    }

    async fn emit_resolution(
        &self,
        approval_id: Uuid,
        backend: &str,
        detailed: &DetailedApprovalOutcome,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<SessionEvent> {
        let payload = resolution_payload(approval_id, backend, detailed);
        sink.emit("approval_resolved", payload).await
    }
}

fn resolution_payload(
    approval_id: Uuid,
    origin: &str,
    detailed: &DetailedApprovalOutcome,
) -> Value {
    match &detailed.outcome {
        ApprovalOutcome::Selected { option_id } => json!({
            "approvalId": approval_id,
            "backend": origin,
            "status": detailed.status.as_str(),
            "outcome": "selected",
            "optionId": option_id,
        }),
        ApprovalOutcome::Cancelled => json!({
            "approvalId": approval_id,
            "backend": origin,
            "status": detailed.status.as_str(),
            "outcome": "cancelled",
        }),
    }
}

fn selected_option_id(outcome: &ApprovalOutcome) -> Option<String> {
    match outcome {
        ApprovalOutcome::Selected { option_id } => Some(option_id.clone()),
        ApprovalOutcome::Cancelled => None,
    }
}

fn prompt_from_record(record: &ApprovalRequestRecord) -> Result<ApprovalPrompt> {
    Ok(ApprovalPrompt {
        action: record.action.clone(),
        scope: record.scope_json.clone(),
        options: serde_json::from_value(record.options_json.clone())?,
    })
}

fn detailed_from_record(record: &ApprovalRequestRecord) -> Result<DetailedApprovalOutcome> {
    let status = match record.status.as_str() {
        "approved" => ApprovalStatus::Approved,
        "denied" => ApprovalStatus::Denied,
        "timed_out" => ApprovalStatus::TimedOut,
        "cancelled" => ApprovalStatus::Cancelled,
        other => {
            return Err(ServerError::Store(format!(
                "approval {} has non-terminal status {other}",
                record.id
            )));
        }
    };
    let outcome = record
        .selected_option_id
        .clone()
        .map_or(ApprovalOutcome::Cancelled, |option_id| {
            ApprovalOutcome::Selected { option_id }
        });
    Ok(DetailedApprovalOutcome { outcome, status })
}

fn select_option(prompt: &ApprovalPrompt, predicate: impl Fn(&str) -> bool) -> Option<String> {
    prompt
        .options
        .iter()
        .find(|option| predicate(&option.kind))
        .map(|option| option.option_id.clone())
}

fn status_for_outcome(prompt: &ApprovalPrompt, outcome: &ApprovalOutcome) -> ApprovalStatus {
    match outcome {
        ApprovalOutcome::Selected { option_id } => prompt
            .options
            .iter()
            .find(|option| option.option_id == *option_id)
            .map(|option| {
                if option.kind.starts_with("allow_") {
                    ApprovalStatus::Approved
                } else if option.kind.starts_with("reject_") {
                    ApprovalStatus::Denied
                } else {
                    ApprovalStatus::Cancelled
                }
            })
            .unwrap_or(ApprovalStatus::Cancelled),
        ApprovalOutcome::Cancelled => ApprovalStatus::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<(String, Value)>>,
    }

    #[async_trait]
    impl CodingEventSink for RecordingSink {
        async fn emit(&self, event_type: &str, payload_json: Value) -> Result<SessionEvent> {
            self.events
                .lock()
                .push((event_type.to_string(), payload_json.clone()));
            Ok(SessionEvent::new(
                Uuid::nil(),
                self.events.lock().len() as i64,
                event_type,
                payload_json,
                chrono::Utc::now(),
            ))
        }
    }

    fn prompt(options: Vec<(&str, &str)>) -> ApprovalPrompt {
        ApprovalPrompt {
            action: "edit file".to_string(),
            scope: json!({ "path": "src/lib.rs" }),
            options: options
                .into_iter()
                .map(|(option_id, kind)| ApprovalOption {
                    option_id: option_id.to_string(),
                    name: option_id.to_string(),
                    kind: kind.to_string(),
                })
                .collect(),
        }
    }

    async fn wait_for_approval_id(sink: &RecordingSink) -> Uuid {
        for _ in 0..100 {
            if let Some((_, payload)) = sink
                .events
                .lock()
                .iter()
                .find(|(event_type, _)| event_type == "approval")
                .cloned()
            {
                return serde_json::from_value(payload["approvalId"].clone()).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("approval event was not emitted")
    }

    #[tokio::test]
    async fn approve_without_option_selects_allow_once_before_allow_always() {
        let broker = Arc::new(ApprovalBroker::default());
        let sink = Arc::new(RecordingSink::default());
        let session_id = Uuid::new_v4();
        let request = {
            let broker = Arc::clone(&broker);
            let sink = Arc::clone(&sink);
            tokio::spawn(async move {
                broker
                    .request_permission(
                        session_id,
                        prompt(vec![
                            ("always", "allow_always"),
                            ("once", "allow_once"),
                            ("reject", "reject_once"),
                        ]),
                        Duration::from_secs(5),
                        sink,
                    )
                    .await
                    .unwrap()
            })
        };
        let approval_id = wait_for_approval_id(&sink).await;
        broker
            .resolve(
                session_id,
                approval_id,
                ResolveApprovalRequest {
                    decision: ApprovalResolveDecision::Approve,
                    option_id: None,
                },
            )
            .unwrap();
        assert_eq!(
            request.await.unwrap(),
            ApprovalOutcome::Selected {
                option_id: "once".to_string()
            }
        );
    }

    #[tokio::test]
    async fn deny_and_timeout_select_reject_once_or_cancel() {
        let broker = Arc::new(ApprovalBroker::default());
        let sink = Arc::new(RecordingSink::default());
        let session_id = Uuid::new_v4();
        let denied = {
            let broker = Arc::clone(&broker);
            let sink = Arc::clone(&sink);
            tokio::spawn(async move {
                broker
                    .request_permission(
                        session_id,
                        prompt(vec![("reject", "reject_once"), ("always", "allow_always")]),
                        Duration::from_secs(5),
                        sink,
                    )
                    .await
                    .unwrap()
            })
        };
        let approval_id = wait_for_approval_id(&sink).await;
        broker
            .resolve(
                session_id,
                approval_id,
                ResolveApprovalRequest {
                    decision: ApprovalResolveDecision::Deny,
                    option_id: None,
                },
            )
            .unwrap();
        assert_eq!(
            denied.await.unwrap(),
            ApprovalOutcome::Selected {
                option_id: "reject".to_string()
            }
        );

        let timeout_sink = Arc::new(RecordingSink::default());
        let timed_out = broker
            .request_permission(
                session_id,
                prompt(vec![("reject", "reject_once"), ("always", "allow_always")]),
                Duration::from_millis(1),
                timeout_sink,
            )
            .await
            .unwrap();
        assert_eq!(
            timed_out,
            ApprovalOutcome::Selected {
                option_id: "reject".to_string()
            }
        );

        let cancel_sink = Arc::new(RecordingSink::default());
        let cancelled = broker
            .request_permission(
                session_id,
                prompt(vec![("always", "allow_always")]),
                Duration::from_millis(1),
                cancel_sink,
            )
            .await
            .unwrap();
        assert_eq!(cancelled, ApprovalOutcome::Cancelled);
    }

    #[tokio::test]
    async fn detailed_permission_preserves_timeout_status() {
        let broker = Arc::new(ApprovalBroker::default());
        let sink = Arc::new(RecordingSink::default());
        let session_id = Uuid::new_v4();
        let timed_out = broker
            .request_permission_detailed_for_backend(
                session_id,
                "native-tm",
                prompt(vec![("reject", "reject_once"), ("allow", "allow_once")]),
                Duration::from_millis(1),
                sink,
            )
            .await
            .unwrap();
        assert_eq!(timed_out.status, ApprovalStatus::TimedOut);
        assert_eq!(
            timed_out.outcome,
            ApprovalOutcome::Selected {
                option_id: "reject".to_string()
            }
        );
    }

    #[tokio::test]
    async fn unknown_and_repeated_resolution_return_invalid_request() {
        let broker = Arc::new(ApprovalBroker::default());
        let session_id = Uuid::new_v4();
        let unknown = broker.resolve(
            session_id,
            Uuid::new_v4(),
            ResolveApprovalRequest {
                decision: ApprovalResolveDecision::Approve,
                option_id: None,
            },
        );
        assert!(matches!(unknown, Err(ServerError::InvalidRequest(_))));

        let sink = Arc::new(RecordingSink::default());
        let pending = {
            let broker = Arc::clone(&broker);
            let sink = Arc::clone(&sink);
            tokio::spawn(async move {
                broker
                    .request_permission(
                        session_id,
                        prompt(vec![("once", "allow_once"), ("reject", "reject_once")]),
                        Duration::from_secs(5),
                        sink,
                    )
                    .await
                    .unwrap()
            })
        };
        let approval_id = wait_for_approval_id(&sink).await;
        broker
            .resolve(
                session_id,
                approval_id,
                ResolveApprovalRequest {
                    decision: ApprovalResolveDecision::Approve,
                    option_id: None,
                },
            )
            .unwrap();
        assert!(matches!(
            broker.resolve(
                session_id,
                approval_id,
                ResolveApprovalRequest {
                    decision: ApprovalResolveDecision::Approve,
                    option_id: None,
                },
            ),
            Err(ServerError::InvalidRequest(_))
        ));
        assert_eq!(
            pending.await.unwrap(),
            ApprovalOutcome::Selected {
                option_id: "once".to_string()
            }
        );
    }

    #[tokio::test]
    async fn durable_permission_observes_resolution_from_another_broker_instance() {
        let store = Arc::new(crate::InMemoryStore::default());
        let session = store
            .create_session(crate::NewSession {
                mode: tm_modes::ModeId::from("general"),
                persona_status: tm_modes::AssetStatus::Degraded {
                    warning: "test".to_string(),
                },
            })
            .await
            .unwrap();
        let requester = Arc::new(ApprovalBroker::default());
        let resolver = Arc::new(ApprovalBroker::default());
        requester.bind_store(Arc::clone(&store));
        resolver.bind_store(Arc::clone(&store));
        let (sender, _) = tokio::sync::broadcast::channel(32);
        let request_sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
            session.id,
            Arc::clone(&store),
            sender.clone(),
        ));
        let pending = {
            let requester = Arc::clone(&requester);
            tokio::spawn(async move {
                requester
                    .request_permission(
                        session.id,
                        prompt(vec![("allow", "allow_once"), ("reject", "reject_once")]),
                        Duration::from_secs(5),
                        request_sink,
                    )
                    .await
                    .unwrap()
            })
        };
        let approval_id = loop {
            if let Some(event) = store
                .events_after(session.id, None)
                .await
                .unwrap()
                .into_iter()
                .find(|event| event.event_type == "approval")
            {
                break event.payload_json["approvalId"]
                    .as_str()
                    .unwrap()
                    .parse::<Uuid>()
                    .unwrap();
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        };
        let resolution_sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
            session.id,
            Arc::clone(&store),
            sender,
        ));
        resolver
            .resolve_persisted(
                session.id,
                approval_id,
                ResolveApprovalRequest {
                    decision: ApprovalResolveDecision::Approve,
                    option_id: Some("allow".to_string()),
                },
                resolution_sink,
            )
            .await
            .unwrap();
        assert_eq!(
            pending.await.unwrap(),
            ApprovalOutcome::Selected {
                option_id: "allow".to_string()
            }
        );
        let approval = store
            .approval_request(session.id, approval_id)
            .await
            .unwrap();
        assert_eq!(approval.status, "approved");
        assert!(approval.request_event_seq.is_some());
        assert!(approval.resolution_event_seq.is_some());
        let lease = store
            .claim_next_approval_effect(Uuid::new_v4(), Utc::now(), chrono::Duration::seconds(30))
            .await
            .unwrap()
            .expect("a restarted worker can claim the durable continuation");
        assert_eq!(lease.effect.approval_id, approval_id);
        store
            .complete_approval_effect(&lease, Utc::now())
            .await
            .unwrap();
    }
}

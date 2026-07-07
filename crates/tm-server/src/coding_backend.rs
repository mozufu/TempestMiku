use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot};
use uuid::Uuid;

use crate::{Result, ServerError, SessionEvent, Store};

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
    /// Declared capabilities from the mode profile (e.g. `["agents.*", "backend.coding"]`).
    /// Merged into sandbox grants for this turn; supports `.*` glob patterns.
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodingTurnResult {
    pub final_text: String,
    pub transcript_artifact: Option<tm_artifacts::ArtifactRef>,
}

#[async_trait]
pub trait CodingEventSink: Send + Sync + 'static {
    async fn emit(&self, event_type: &str, payload_json: Value) -> Result<SessionEvent>;
}

pub struct StoreCodingEventSink<S> {
    session_id: Uuid,
    store: Arc<S>,
    sender: broadcast::Sender<SessionEvent>,
}

impl<S> StoreCodingEventSink<S> {
    pub fn new(session_id: Uuid, store: Arc<S>, sender: broadcast::Sender<SessionEvent>) -> Self {
        Self {
            session_id,
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
            .append_event(self.session_id, event_type, payload_json)
            .await?;
        let _ = self.sender.send(event.clone());
        Ok(event)
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

#[derive(Default)]
pub struct ApprovalBroker {
    pending: Mutex<BTreeMap<Uuid, PendingApproval>>,
}

struct PendingApproval {
    session_id: Uuid,
    sender: oneshot::Sender<ResolveApprovalRequest>,
}

impl ApprovalBroker {
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

    pub async fn request_permission_detailed_for_backend(
        &self,
        session_id: Uuid,
        backend: &str,
        prompt: ApprovalPrompt,
        timeout: Duration,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<DetailedApprovalOutcome> {
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
        let payload = match &detailed.outcome {
            ApprovalOutcome::Selected { option_id } => json!({
                "approvalId": approval_id,
                "backend": backend,
                "status": detailed.status.as_str(),
                "outcome": "selected",
                "optionId": option_id,
            }),
            ApprovalOutcome::Cancelled => json!({
                "approvalId": approval_id,
                "backend": backend,
                "status": detailed.status.as_str(),
                "outcome": "cancelled",
            }),
        };
        sink.emit("approval_resolved", payload).await
    }
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
                "native-deno",
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
}

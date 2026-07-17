use std::{sync::Arc, time::Duration};

use serde_json::json;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::coding_backend::CodingEventSink;
use crate::{Result, ServerError, SessionEvent};

use super::outcome::{resolution_payload, select_option, status_for_outcome};
use super::{
    ApprovalBroker, ApprovalOutcome, ApprovalPrompt, ApprovalResolveDecision, ApprovalStatus,
    DetailedApprovalOutcome, DurableApprovalSpec, PendingApproval, ResolveApprovalRequest,
};

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

    pub(super) fn resolve_outcome(
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

    pub(super) fn reject_outcome(&self, prompt: &ApprovalPrompt) -> ApprovalOutcome {
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

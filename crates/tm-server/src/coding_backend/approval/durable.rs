use std::{sync::Arc, time::Duration};

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::coding_backend::CodingEventSink;
use crate::{NewApprovalRequest, NewApprovalResolution, Result, ServerError, Store};

use super::outcome::{
    detailed_from_record, prompt_from_record, resolution_payload, selected_option_id,
    status_for_outcome,
};
use super::{
    ApprovalBroker, ApprovalStatus, DetailedApprovalOutcome, DurableApprovalSpec,
    ResolveApprovalRequest,
};

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

    pub async fn request_permission_detailed_with_effect_for_backend(
        &self,
        spec: DurableApprovalSpec,
    ) -> Result<(Uuid, DetailedApprovalOutcome)> {
        let durable_store = { self.store.read().clone() };
        let store = durable_store
            .ok_or_else(|| ServerError::Store("durable approval store is not bound".to_string()))?;
        self.request_permission_durable(store, spec).await
    }

    pub(super) async fn request_permission_durable(
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
}

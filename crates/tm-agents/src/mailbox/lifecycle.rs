use chrono::Utc;

use crate::actor::{ActorId, ActorLifecycleEvent, ActorStatus};
use crate::supervise::{
    FailureReason, SupervisionAction, SupervisionDecision, SupervisionPolicy, Supervisor,
};

use super::{ActorKey, MailboxRegistry};

impl MailboxRegistry {
    pub async fn set_supervision_policy_for_session(
        &self,
        session_id: &str,
        supervisor_id: ActorId,
        policy: SupervisionPolicy,
    ) {
        let mut supervisors = self.supervisors.write().await;
        let supervisor = supervisors
            .entry(ActorKey::new(session_id, &supervisor_id))
            .or_insert_with(|| Supervisor::new(policy.clone()));
        supervisor.policy = policy;
    }

    /// Mark an actor as successfully terminated.
    pub async fn mark_complete_for_session(&self, session_id: &str, id: &ActorId) -> bool {
        let Some(supervisor_id) = self.mark_complete_record(session_id, id, None).await else {
            return false;
        };
        self.record_supervised_success(session_id, &supervisor_id, id)
            .await;
        true
    }

    /// Mark an actor as successfully terminated and store its output/history URIs (P3.3).
    ///
    /// Stores artifact URI and history URI. Digest summaries are returned by the orchestration
    /// calls, not cached in the roster.
    pub async fn mark_complete_with_resources_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
        artifact_uri: Option<String>,
        history_uri: Option<String>,
    ) -> bool {
        let Some(supervisor_id) = self
            .mark_complete_record(session_id, id, Some((artifact_uri, history_uri)))
            .await
        else {
            return false;
        };
        self.record_supervised_success(session_id, &supervisor_id, id)
            .await;
        true
    }

    /// Mark an actor as terminated due to a failure.
    pub async fn mark_failed_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
        reason: FailureReason,
    ) -> bool {
        self.mark_failed_record(session_id, id, reason)
            .await
            .is_some()
    }

    pub async fn mark_failed_with_supervision_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
        reason: FailureReason,
    ) -> Option<(ActorId, SupervisionDecision)> {
        let supervisor_id = self
            .mark_failed_record(session_id, id, reason.clone())
            .await?;
        let decision = {
            let mut supervisors = self.supervisors.write().await;
            supervisors
                .entry(ActorKey::new(session_id, &supervisor_id))
                .or_default()
                .record_failure(id, reason)
        };

        Some((supervisor_id, decision))
    }

    pub async fn record_actor_error(
        &self,
        session_id: &str,
        actor_id: &ActorId,
        reason: FailureReason,
    ) -> Option<SupervisionDecision> {
        let (supervisor_id, decision) = self
            .mark_failed_with_supervision_for_session(session_id, actor_id, reason.clone())
            .await?;

        self.emit_lifecycle(
            session_id,
            ActorLifecycleEvent::StatusChanged {
                actor_id: actor_id.clone(),
                status: ActorStatus::Terminated,
                at: Utc::now(),
            },
        );

        match reason {
            FailureReason::Cancelled => self.emit_lifecycle(
                session_id,
                ActorLifecycleEvent::Cancelled {
                    actor_id: actor_id.clone(),
                    cancelled_at: Utc::now(),
                },
            ),
            reason => self.emit_lifecycle(
                session_id,
                ActorLifecycleEvent::Failed {
                    actor_id: actor_id.clone(),
                    failed_at: Utc::now(),
                    reason,
                },
            ),
        }

        self.emit_and_apply_supervision_decision(session_id, supervisor_id, decision.clone())
            .await;
        Some(decision)
    }

    pub async fn emit_and_apply_supervision_decision(
        &self,
        session_id: &str,
        supervisor_id: ActorId,
        decision: SupervisionDecision,
    ) {
        if !decision.actions.is_empty() {
            self.emit_lifecycle(
                session_id,
                ActorLifecycleEvent::Supervision {
                    supervisor_id,
                    failed_actor_id: decision.failed_actor_id.clone(),
                    decision: decision.clone(),
                    decided_at: Utc::now(),
                },
            );
            self.apply_supervision_actions(session_id, &decision).await;
        }
    }

    async fn mark_complete_record(
        &self,
        session_id: &str,
        id: &ActorId,
        resources: Option<(Option<String>, Option<String>)>,
    ) -> Option<ActorId> {
        let mut actors = self.actors.write().await;
        let rec = actors.get_mut(&ActorKey::new(session_id, id))?;
        if rec.cancelled || rec.failure_reason.is_some() {
            return None;
        }
        rec.status = ActorStatus::Terminated;
        rec.completed_at = Some(Utc::now());
        if let Some((artifact_uri, history_uri)) = resources {
            rec.artifact_uri = artifact_uri;
            rec.history_uri = history_uri;
        }
        let fallback = rec.parent.clone();
        drop(actors);
        Some(self.supervisor_id_for_actor(session_id, id, fallback).await)
    }

    async fn mark_failed_record(
        &self,
        session_id: &str,
        id: &ActorId,
        reason: FailureReason,
    ) -> Option<ActorId> {
        let (fallback_supervisor, should_cancel_token) = {
            let mut actors = self.actors.write().await;
            let rec = actors.get_mut(&ActorKey::new(session_id, id))?;
            if rec.cancelled {
                return None;
            }
            rec.status = ActorStatus::Terminated;
            rec.completed_at = Some(Utc::now());
            rec.cancelled = matches!(reason, FailureReason::Cancelled);
            rec.failure_reason = Some(reason);
            (rec.parent.clone(), rec.cancelled)
        };
        if should_cancel_token {
            self.cancel_token_for_session(session_id, id).await.cancel();
        }
        Some(
            self.supervisor_id_for_actor(session_id, id, fallback_supervisor)
                .await,
        )
    }

    async fn record_supervised_success(
        &self,
        session_id: &str,
        supervisor_id: &ActorId,
        id: &ActorId,
    ) {
        if let Some(supervisor) = self
            .supervisors
            .write()
            .await
            .get_mut(&ActorKey::new(session_id, supervisor_id))
        {
            supervisor.record_success(id);
        }
    }

    async fn apply_supervision_actions(&self, session_id: &str, decision: &SupervisionDecision) {
        for action in &decision.actions {
            if let SupervisionAction::Cancel { actor_id } = action {
                let _ = self.cancel_actor_subtree(session_id, actor_id).await;
            }
        }
    }
}

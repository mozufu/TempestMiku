use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::actor::{ActorCancelToken, ActorId, ActorRecord, ActorSpec, ActorStatus};
use crate::supervise::{FailureReason, SupervisionDecision, SupervisionPolicy};

use super::{ActorMessage, MailboxRegistry, Receipt};

impl MailboxRegistry {
    // Unit tests written before session ownership use one isolated synthetic session. Keep these
    // compatibility helpers test-only so production callers must always supply exact ownership.
    #[cfg(test)]
    pub async fn track(&self, record: ActorRecord) {
        self.track_for_session("", record).await.unwrap();
    }

    #[cfg(test)]
    pub async fn track_with_supervisor(&self, record: ActorRecord, supervisor_id: ActorId) {
        self.track_with_supervisor_for_session("", record, supervisor_id)
            .await
            .unwrap();
    }

    #[cfg(test)]
    pub async fn get(&self, id: &ActorId) -> Option<ActorRecord> {
        self.get_for_session("", id).await
    }

    #[cfg(test)]
    pub async fn update_status(&self, id: &ActorId, status: ActorStatus) {
        self.update_status_for_session("", id, status).await;
    }

    #[cfg(test)]
    pub async fn set_supervision_policy(&self, supervisor_id: ActorId, policy: SupervisionPolicy) {
        self.set_supervision_policy_for_session("", supervisor_id, policy)
            .await;
    }

    #[cfg(test)]
    pub async fn mark_complete(&self, id: &ActorId) -> bool {
        self.mark_complete_for_session("", id).await
    }

    #[cfg(test)]
    pub async fn mark_complete_with_resources(
        &self,
        id: &ActorId,
        artifact_uri: Option<String>,
        history_uri: Option<String>,
    ) -> bool {
        self.mark_complete_with_resources_for_session("", id, artifact_uri, history_uri)
            .await
    }

    #[cfg(test)]
    pub async fn store_transcript(&self, id: &ActorId, content: String) {
        self.store_transcript_for_session("", id, content).await;
    }

    #[cfg(test)]
    pub async fn get_transcript(&self, id: &ActorId) -> Option<String> {
        self.get_transcript_for_session("", id).await
    }

    #[cfg(test)]
    pub async fn record_message(&self, message: ActorMessage) {
        self.record_message_for_session("", message).await.unwrap();
    }

    #[cfg(test)]
    pub async fn send_message(&self, message: ActorMessage) -> Receipt {
        self.send_message_for_session("", message).await.unwrap()
    }

    #[cfg(test)]
    pub async fn drain_inbox(
        &self,
        actor_id: &ActorId,
        from: Option<&ActorId>,
    ) -> Vec<ActorMessage> {
        self.drain_inbox_for_session("", actor_id, from).await
    }

    #[cfg(test)]
    pub async fn wait_for_message(
        &self,
        actor_id: &ActorId,
        from: Option<&ActorId>,
        timeout: Duration,
    ) -> Option<ActorMessage> {
        self.wait_for_message_for_session("", actor_id, from, timeout)
            .await
    }

    #[cfg(test)]
    pub async fn unread_count(&self, actor_id: &ActorId) -> usize {
        self.unread_count_for_session("", actor_id).await
    }

    #[cfg(test)]
    pub async fn last_activity(&self, actor_id: &ActorId) -> Option<DateTime<Utc>> {
        self.last_activity_for_session("", actor_id).await
    }

    #[cfg(test)]
    pub async fn messages(&self) -> Vec<ActorMessage> {
        self.messages_for_session("").await
    }

    #[cfg(test)]
    pub async fn mark_failed(&self, id: &ActorId, reason: FailureReason) -> bool {
        self.mark_failed_for_session("", id, reason).await
    }

    #[cfg(test)]
    pub async fn mark_failed_with_supervision(
        &self,
        id: &ActorId,
        reason: FailureReason,
    ) -> Option<(ActorId, SupervisionDecision)> {
        self.mark_failed_with_supervision_for_session("", id, reason)
            .await
    }

    #[cfg(test)]
    pub async fn prepare_actor_restart(&self, actor_id: &ActorId) -> Option<ActorSpec> {
        self.prepare_actor_restart_for_session("", actor_id).await
    }

    #[cfg(test)]
    pub async fn cancel_token(&self, actor_id: &ActorId) -> ActorCancelToken {
        self.cancel_token_for_session("", actor_id).await
    }

    #[cfg(test)]
    pub async fn is_cancelled(&self, actor_id: &ActorId) -> bool {
        self.is_cancelled_for_session("", actor_id).await
    }

    #[cfg(test)]
    pub async fn would_wait_on_descendant(&self, waiter: &ActorId, target: &ActorId) -> bool {
        self.would_wait_on_descendant_for_session("", waiter, target)
            .await
    }

    #[cfg(test)]
    pub async fn is_descendant_of(&self, actor_id: &ActorId, ancestor_id: &ActorId) -> bool {
        self.is_descendant_of_for_session("", actor_id, ancestor_id)
            .await
    }

    #[cfg(test)]
    pub async fn list(&self) -> Vec<ActorRecord> {
        self.list_for_session("").await
    }

    #[cfg(test)]
    pub async fn live_direct_children(&self, parent_id: &ActorId) -> Vec<ActorRecord> {
        self.live_direct_children_for_session("", parent_id).await
    }
}

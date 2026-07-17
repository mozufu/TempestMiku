use std::collections::{HashSet, VecDeque};

use chrono::Utc;

use crate::actor::{ActorCancelToken, ActorId, ActorLifecycleEvent, ActorStatus};
use crate::supervise::FailureReason;

use super::{ActorKey, CancelActorResult, MailboxRegistry};

impl MailboxRegistry {
    pub async fn cancel_actor(&self, session_id: &str, id: &ActorId) -> CancelActorResult {
        let cancelled_at = Utc::now();
        let result = {
            let mut actors = self.actors.write().await;
            match actors.get_mut(&ActorKey::new(session_id, id)) {
                Some(rec) if rec.cancelled => CancelActorResult::AlreadyCancelled,
                Some(rec) if rec.status == ActorStatus::Terminated => {
                    CancelActorResult::AlreadyTerminated
                }
                Some(rec) => {
                    rec.status = ActorStatus::Terminated;
                    rec.completed_at = Some(cancelled_at);
                    rec.cancelled = true;
                    rec.failure_reason = Some(FailureReason::Cancelled);
                    CancelActorResult::Cancelled
                }
                None => CancelActorResult::NotFound,
            }
        };

        if matches!(
            result,
            CancelActorResult::Cancelled | CancelActorResult::AlreadyCancelled
        ) {
            self.cancel_token_for_session(session_id, id).await.cancel();
        }
        if result == CancelActorResult::Cancelled {
            self.emit_lifecycle(
                session_id,
                ActorLifecycleEvent::StatusChanged {
                    actor_id: id.clone(),
                    status: ActorStatus::Terminated,
                    at: cancelled_at,
                },
            );
            self.emit_lifecycle(
                session_id,
                ActorLifecycleEvent::Cancelled {
                    actor_id: id.clone(),
                    cancelled_at,
                },
            );
        }
        result
    }

    pub async fn cancel_actor_subtree(
        &self,
        session_id: &str,
        root: &ActorId,
    ) -> Vec<(ActorId, CancelActorResult)> {
        let actor_ids = self.actor_subtree_ids(session_id, root).await;
        let mut results = Vec::with_capacity(actor_ids.len());
        for actor_id in actor_ids {
            let result = self.cancel_actor(session_id, &actor_id).await;
            results.push((actor_id, result));
        }
        results
    }

    pub async fn cancel_token_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
    ) -> ActorCancelToken {
        self.ensure_cancel_token(session_id, actor_id).await
    }

    pub async fn is_cancelled_for_session(&self, session_id: &str, actor_id: &ActorId) -> bool {
        self.cancel_tokens
            .read()
            .await
            .get(&ActorKey::new(session_id, actor_id))
            .is_some_and(ActorCancelToken::is_cancelled)
    }

    /// Return true when `waiter` would explicitly wait on itself or a descendant.
    ///
    /// The synthetic top-level Root actor is intentionally exempt: top-level
    /// orchestrator code must be able to await root-level workers. Real actor
    /// descendants are checked through the tracked parent chain.
    pub async fn would_wait_on_descendant_for_session(
        &self,
        session_id: &str,
        waiter: &ActorId,
        target: &ActorId,
    ) -> bool {
        if waiter.as_str() == "Root" {
            return false;
        }
        if waiter == target {
            return true;
        }
        self.is_descendant_of_for_session(session_id, target, waiter)
            .await
    }

    /// Return true when `actor_id` has `ancestor_id` in its tracked parent chain.
    pub async fn is_descendant_of_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
        ancestor_id: &ActorId,
    ) -> bool {
        let actors = self.actors.read().await;
        let mut current = actor_id.clone();
        let mut seen = HashSet::new();

        while seen.insert(current.clone()) {
            let Some(record) = actors.get(&ActorKey::new(session_id, &current)) else {
                return false;
            };
            let Some(parent) = record.parent.as_ref() else {
                return false;
            };
            if parent == ancestor_id {
                return true;
            }
            current = parent.clone();
        }

        false
    }

    async fn ensure_cancel_token(&self, session_id: &str, actor_id: &ActorId) -> ActorCancelToken {
        let key = ActorKey::new(session_id, actor_id);
        if let Some(token) = self.cancel_tokens.read().await.get(&key).cloned() {
            return token;
        }
        let mut tokens = self.cancel_tokens.write().await;
        tokens.entry(key).or_default().clone()
    }

    async fn actor_subtree_ids(&self, session_id: &str, root: &ActorId) -> Vec<ActorId> {
        let actors = self.actors.read().await;
        if !actors.contains_key(&ActorKey::new(session_id, root)) {
            return vec![root.clone()];
        }

        let mut ordered = Vec::new();
        let mut pending = VecDeque::new();
        let mut seen = HashSet::new();
        pending.push_back(root.clone());

        while let Some(current) = pending.pop_front() {
            if !seen.insert(current.clone()) {
                continue;
            }
            ordered.push(current.clone());

            let mut children = actors
                .iter()
                .filter(|(key, _)| key.session_id == session_id)
                .map(|(_, record)| record)
                .filter(|record| record.parent.as_ref() == Some(&current))
                .map(|record| (record.spawned_at, record.id.clone()))
                .collect::<Vec<_>>();
            children.sort_by(|(left_at, left_id), (right_at, right_id)| {
                left_at.cmp(right_at).then_with(|| left_id.cmp(right_id))
            });
            pending.extend(children.into_iter().map(|(_, child)| child));
        }

        ordered
    }
}

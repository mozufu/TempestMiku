use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::actor::ActorId;

/// Erlang/OTP-inspired restart strategies (§23.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartStrategy {
    /// Restart only the dead child; siblings continue (default — §23.4 "one_for_one").
    #[default]
    OneForOne,
    /// Restart the dead child and all siblings in the group ("one_for_all").
    OneForAll,
    /// Restart the dead child and those started after it ("rest_for_one").
    RestForOne,
}

/// Reason a child actor stopped abnormally (§23.4, §23.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FailureReason {
    Crash { message: String },
    Timeout,
    ApprovalDenied,
    QuotaExhausted,
    Cancelled,
    DepthExceeded,
}

impl FailureReason {
    pub fn is_restartable(&self) -> bool {
        matches!(self, Self::Crash { .. } | Self::Timeout)
    }
}

/// Supervision policy applied by a parent actor to its children (§23.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisionPolicy {
    pub strategy: RestartStrategy,
    /// Maximum restart attempts before escalating to the grandparent supervisor.
    pub max_restarts: u32,
}

impl Default for SupervisionPolicy {
    fn default() -> Self {
        Self {
            strategy: RestartStrategy::OneForOne,
            max_restarts: 3,
        }
    }
}

/// A replayable instruction produced by supervision policy evaluation (§23.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SupervisionAction {
    Cancel {
        actor_id: ActorId,
    },
    Restart {
        actor_id: ActorId,
    },
    Escalate {
        actor_id: ActorId,
        reason: FailureReason,
    },
}

/// The policy decision for one child failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisionDecision {
    pub failed_actor_id: ActorId,
    pub reason: FailureReason,
    pub strategy: RestartStrategy,
    /// One-based restart attempt for restart decisions; `None` means no restart was attempted.
    pub restart_attempt: Option<u32>,
    pub actions: Vec<SupervisionAction>,
}

/// Tracks a supervisor's live children and applies the restart strategy on failure (§23.4).
#[derive(Debug, Default)]
pub struct Supervisor {
    pub policy: SupervisionPolicy,
    children: Vec<ActorId>,
    restart_attempts: HashMap<ActorId, u32>,
}

impl Supervisor {
    pub fn new(policy: SupervisionPolicy) -> Self {
        Self {
            policy,
            children: Vec::new(),
            restart_attempts: HashMap::new(),
        }
    }

    pub fn children(&self) -> &[ActorId] {
        &self.children
    }

    pub fn track(&mut self, id: ActorId) {
        if !self.children.contains(&id) {
            self.children.push(id);
        }
    }

    pub fn restart_attempts(&self, id: &ActorId) -> u32 {
        self.restart_attempts.get(id).copied().unwrap_or(0)
    }

    pub fn record_success(&mut self, id: &ActorId) {
        self.restart_attempts.remove(id);
    }

    pub fn record_failure(
        &mut self,
        failed_actor_id: &ActorId,
        reason: FailureReason,
    ) -> SupervisionDecision {
        let mut decision = SupervisionDecision {
            failed_actor_id: failed_actor_id.clone(),
            reason: reason.clone(),
            strategy: self.policy.strategy,
            restart_attempt: None,
            actions: Vec::new(),
        };

        let Some(failed_index) = self.children.iter().position(|id| id == failed_actor_id) else {
            decision.actions.push(SupervisionAction::Escalate {
                actor_id: failed_actor_id.clone(),
                reason,
            });
            return decision;
        };

        if matches!(reason, FailureReason::Cancelled) {
            return decision;
        }

        if !reason.is_restartable() {
            decision.actions.push(SupervisionAction::Escalate {
                actor_id: failed_actor_id.clone(),
                reason,
            });
            return decision;
        }

        let cancel_targets = self.cancel_targets(failed_index);
        if self.restart_attempts(failed_actor_id) >= self.policy.max_restarts {
            decision.actions.extend(
                cancel_targets
                    .into_iter()
                    .map(|actor_id| SupervisionAction::Cancel { actor_id }),
            );
            decision.actions.push(SupervisionAction::Escalate {
                actor_id: failed_actor_id.clone(),
                reason,
            });
            return decision;
        }

        let attempts = self
            .restart_attempts
            .entry(failed_actor_id.clone())
            .or_default();
        *attempts += 1;
        decision.restart_attempt = Some(*attempts);

        let restart_targets = self.restart_targets(failed_index);
        decision.actions.extend(
            cancel_targets
                .into_iter()
                .map(|actor_id| SupervisionAction::Cancel { actor_id }),
        );
        decision.actions.extend(
            restart_targets
                .into_iter()
                .map(|actor_id| SupervisionAction::Restart { actor_id }),
        );
        decision
    }

    fn cancel_targets(&self, failed_index: usize) -> Vec<ActorId> {
        match self.policy.strategy {
            RestartStrategy::OneForOne => Vec::new(),
            RestartStrategy::OneForAll => self
                .children
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != failed_index)
                .map(|(_, id)| id.clone())
                .collect(),
            RestartStrategy::RestForOne => self.children[failed_index + 1..].to_vec(),
        }
    }

    fn restart_targets(&self, failed_index: usize) -> Vec<ActorId> {
        match self.policy.strategy {
            RestartStrategy::OneForOne => vec![self.children[failed_index].clone()],
            RestartStrategy::OneForAll => self.children.clone(),
            RestartStrategy::RestForOne => self.children[failed_index..].to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn id(value: &str) -> ActorId {
        ActorId::new(value).expect("valid actor id")
    }

    fn action_restart(value: &str) -> SupervisionAction {
        SupervisionAction::Restart {
            actor_id: id(value),
        }
    }

    fn action_cancel(value: &str) -> SupervisionAction {
        SupervisionAction::Cancel {
            actor_id: id(value),
        }
    }

    #[test]
    fn supervision_policy_default_is_one_for_one_with_three_restarts() {
        let policy = SupervisionPolicy::default();

        assert_eq!(policy.strategy, RestartStrategy::OneForOne);
        assert_eq!(policy.max_restarts, 3);
    }

    #[test]
    fn supervisor_tracks_children_in_spawn_order() {
        let mut supervisor = Supervisor::new(SupervisionPolicy {
            strategy: RestartStrategy::RestForOne,
            max_restarts: 2,
        });
        let child_a = ActorId::new("ChildA").expect("valid actor id");
        let child_b = ActorId::new("ChildB").expect("valid actor id");

        supervisor.track(child_a.clone());
        supervisor.track(child_b.clone());

        let expected = vec![child_a, child_b];
        assert_eq!(supervisor.policy.strategy, RestartStrategy::RestForOne);
        assert_eq!(supervisor.policy.max_restarts, 2);
        assert_eq!(supervisor.children(), expected.as_slice());
    }

    #[test]
    fn supervisor_tracks_actor_ids_once() {
        let mut supervisor = Supervisor::new(SupervisionPolicy::default());
        let child = id("ChildA");

        supervisor.track(child.clone());
        supervisor.track(child.clone());

        assert_eq!(supervisor.children(), &[child]);
    }

    #[test]
    fn supervision_wire_values_are_snake_case_and_tagged() {
        assert_eq!(
            serde_json::to_value(RestartStrategy::OneForAll).expect("serialize strategy"),
            json!("one_for_all")
        );
        assert_eq!(
            serde_json::to_value(FailureReason::Crash {
                message: "boom".to_string(),
            })
            .expect("serialize failure reason"),
            json!({"kind": "crash", "message": "boom"})
        );

        let reason: FailureReason =
            serde_json::from_value(json!({"kind": "approval_denied"})).expect("parse reason");
        assert!(matches!(reason, FailureReason::ApprovalDenied));
    }

    #[test]
    fn one_for_one_restarts_only_failed_child() {
        let mut supervisor = Supervisor::new(SupervisionPolicy {
            strategy: RestartStrategy::OneForOne,
            max_restarts: 2,
        });
        supervisor.track(id("ChildA"));
        supervisor.track(id("ChildB"));

        let decision = supervisor.record_failure(
            &id("ChildA"),
            FailureReason::Crash {
                message: "boom".to_string(),
            },
        );

        assert_eq!(decision.strategy, RestartStrategy::OneForOne);
        assert_eq!(decision.restart_attempt, Some(1));
        assert_eq!(decision.actions, vec![action_restart("ChildA")]);
        assert_eq!(supervisor.restart_attempts(&id("ChildA")), 1);
        assert_eq!(supervisor.restart_attempts(&id("ChildB")), 0);
    }

    #[test]
    fn one_for_all_cancels_siblings_then_restarts_every_child_in_spawn_order() {
        let mut supervisor = Supervisor::new(SupervisionPolicy {
            strategy: RestartStrategy::OneForAll,
            max_restarts: 1,
        });
        supervisor.track(id("ChildA"));
        supervisor.track(id("ChildB"));
        supervisor.track(id("ChildC"));

        let decision = supervisor.record_failure(&id("ChildB"), FailureReason::Timeout);

        assert_eq!(decision.restart_attempt, Some(1));
        assert_eq!(
            decision.actions,
            vec![
                action_cancel("ChildA"),
                action_cancel("ChildC"),
                action_restart("ChildA"),
                action_restart("ChildB"),
                action_restart("ChildC"),
            ]
        );
    }

    #[test]
    fn rest_for_one_cancels_and_restarts_failed_child_and_later_siblings() {
        let mut supervisor = Supervisor::new(SupervisionPolicy {
            strategy: RestartStrategy::RestForOne,
            max_restarts: 1,
        });
        supervisor.track(id("ChildA"));
        supervisor.track(id("ChildB"));
        supervisor.track(id("ChildC"));

        let decision = supervisor.record_failure(
            &id("ChildB"),
            FailureReason::Crash {
                message: "boom".to_string(),
            },
        );

        assert_eq!(decision.restart_attempt, Some(1));
        assert_eq!(
            decision.actions,
            vec![
                action_cancel("ChildC"),
                action_restart("ChildB"),
                action_restart("ChildC"),
            ]
        );
    }

    #[test]
    fn max_restarts_escalates_after_allowed_attempts() {
        let mut supervisor = Supervisor::new(SupervisionPolicy {
            strategy: RestartStrategy::OneForOne,
            max_restarts: 1,
        });
        supervisor.track(id("ChildA"));

        let first = supervisor.record_failure(&id("ChildA"), FailureReason::Timeout);
        let second = supervisor.record_failure(&id("ChildA"), FailureReason::Timeout);

        assert_eq!(first.restart_attempt, Some(1));
        assert_eq!(first.actions, vec![action_restart("ChildA")]);
        assert_eq!(second.restart_attempt, None);
        assert_eq!(
            second.actions,
            vec![SupervisionAction::Escalate {
                actor_id: id("ChildA"),
                reason: FailureReason::Timeout,
            }]
        );
    }

    #[test]
    fn zero_restart_one_for_all_cancels_siblings_and_escalates_failed_child() {
        let mut supervisor = Supervisor::new(SupervisionPolicy {
            strategy: RestartStrategy::OneForAll,
            max_restarts: 0,
        });
        supervisor.track(id("ChildA"));
        supervisor.track(id("ChildB"));
        supervisor.track(id("ChildC"));

        let decision = supervisor.record_failure(&id("ChildB"), FailureReason::Timeout);

        assert_eq!(decision.restart_attempt, None);
        assert_eq!(
            decision.actions,
            vec![
                action_cancel("ChildA"),
                action_cancel("ChildC"),
                SupervisionAction::Escalate {
                    actor_id: id("ChildB"),
                    reason: FailureReason::Timeout,
                },
            ]
        );
    }

    #[test]
    fn non_restartable_failure_escalates_without_counting_attempts() {
        let mut supervisor = Supervisor::new(SupervisionPolicy::default());
        supervisor.track(id("ChildA"));

        let decision = supervisor.record_failure(&id("ChildA"), FailureReason::ApprovalDenied);

        assert_eq!(decision.restart_attempt, None);
        assert_eq!(supervisor.restart_attempts(&id("ChildA")), 0);
        assert_eq!(
            decision.actions,
            vec![SupervisionAction::Escalate {
                actor_id: id("ChildA"),
                reason: FailureReason::ApprovalDenied,
            }]
        );
    }

    #[test]
    fn cancellation_is_terminal_without_restart_or_escalation() {
        let mut supervisor = Supervisor::new(SupervisionPolicy::default());
        supervisor.track(id("ChildA"));

        let decision = supervisor.record_failure(&id("ChildA"), FailureReason::Cancelled);

        assert_eq!(decision.restart_attempt, None);
        assert!(decision.actions.is_empty());
        assert_eq!(supervisor.restart_attempts(&id("ChildA")), 0);
    }

    #[test]
    fn successful_child_run_clears_restart_attempts() {
        let mut supervisor = Supervisor::new(SupervisionPolicy::default());
        let failed = id("ChildA");
        supervisor.track(failed.clone());

        supervisor.record_failure(&failed, FailureReason::Timeout);
        supervisor.record_success(&failed);

        assert_eq!(supervisor.restart_attempts(&failed), 0);
    }
}

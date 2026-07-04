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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FailureReason {
    Crash { message: String },
    Timeout,
    ApprovalDenied,
    QuotaExhausted,
    Cancelled,
    DepthExceeded,
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

/// Tracks a supervisor's live children and applies the restart strategy on failure (§23.4).
///
/// Supervision tree logic is deferred to P3.1.
#[derive(Debug, Default)]
pub struct Supervisor {
    pub policy: SupervisionPolicy,
    children: Vec<ActorId>,
}

impl Supervisor {
    pub fn new(policy: SupervisionPolicy) -> Self {
        Self {
            policy,
            children: Vec::new(),
        }
    }

    pub fn children(&self) -> &[ActorId] {
        &self.children
    }

    pub fn track(&mut self, id: ActorId) {
        self.children.push(id);
    }
}

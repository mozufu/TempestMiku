use tm_host::CapabilityGrants;

use crate::actor::{ActorBudget, ActorCancelToken, ActorId, ActorSpec};

#[derive(Debug, Clone)]
pub(super) struct ActorRestartTemplate {
    pub(super) id: ActorId,
    pub(super) session_id: String,
    session_scope: Option<String>,
    pub(super) role: String,
    pub(super) task: String,
    mode: Option<String>,
    grants: CapabilityGrants,
    pub(super) budget: ActorBudget,
    pub(super) parent: Option<ActorId>,
    pub(super) supervisor_id: ActorId,
    pub(super) depth: u32,
}

impl ActorRestartTemplate {
    pub(super) fn from_spec(spec: &ActorSpec, supervisor_id: ActorId) -> Self {
        Self {
            id: spec.id.clone(),
            session_id: spec.session_id.clone(),
            session_scope: spec.session_scope.clone(),
            role: spec.role.clone(),
            task: spec.task.clone(),
            mode: spec.mode.clone(),
            grants: spec.grants.clone(),
            budget: spec.budget.clone(),
            parent: spec.parent.clone(),
            supervisor_id,
            depth: spec.depth,
        }
    }

    pub(super) fn to_spec(&self, cancellation: ActorCancelToken) -> ActorSpec {
        ActorSpec {
            id: self.id.clone(),
            session_id: self.session_id.clone(),
            session_scope: self.session_scope.clone(),
            role: self.role.clone(),
            task: self.task.clone(),
            mode: self.mode.clone(),
            grants: self.grants.clone(),
            budget: self.budget.clone(),
            parent: self.parent.clone(),
            depth: self.depth,
            cancellation,
        }
    }
}

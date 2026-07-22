use chrono::Utc;
use tm_host::CapabilityGrants;

use crate::actor::{
    ActorBudget, ActorCancelToken, ActorId, ActorLifecycleEvent, ActorRecord, ActorSpec,
    ActorStatus,
};

use super::inbox::ActorInbox;
use super::{ActorKey, MailboxRegistry};

#[derive(Debug, Clone)]
pub(super) struct ActorRestartTemplate {
    pub(super) id: ActorId,
    pub(super) session_id: String,
    project_id: Option<String>,
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
            project_id: spec.project_id.clone(),
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
            project_id: self.project_id.clone(),
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

impl MailboxRegistry {
    pub async fn remember_restart_spec(&self, spec: &ActorSpec) {
        let supervisor_id = self
            .supervisor_id_for_actor(&spec.session_id, &spec.id, spec.parent.clone())
            .await;
        self.restart_templates.write().await.insert(
            ActorKey::new(&spec.session_id, &spec.id),
            ActorRestartTemplate::from_spec(spec, supervisor_id),
        );
    }

    pub async fn prepare_actor_restart_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
    ) -> Option<ActorSpec> {
        let key = ActorKey::new(session_id, actor_id);
        let template = self.restart_templates.read().await.get(&key).cloned()?;
        let cancellation = ActorCancelToken::default();
        {
            self.cancel_tokens
                .write()
                .await
                .insert(key.clone(), cancellation.clone());
            self.actor_supervisors
                .write()
                .await
                .insert(key.clone(), template.supervisor_id.clone());
            self.inboxes
                .write()
                .await
                .insert(key.clone(), ActorInbox::new());
        }

        let spawned_at = Utc::now();
        {
            let mut actors = self.actors.write().await;
            let record = actors.entry(key).or_insert_with(|| ActorRecord {
                id: actor_id.clone(),
                parent: template.parent.clone(),
                status: ActorStatus::Running,
                mode: Some(template.role.clone()),
                budget: template.budget.clone(),
                spawned_at,
                completed_at: None,
                cancelled: false,
                failure_reason: None,
                artifact_uri: None,
                history_uri: None,
            });
            record.parent = template.parent.clone();
            record.status = ActorStatus::Running;
            record.mode = Some(template.role.clone());
            record.budget = template.budget.clone();
            record.spawned_at = spawned_at;
            record.completed_at = None;
            record.cancelled = false;
            record.failure_reason = None;
            record.artifact_uri = None;
            record.history_uri = None;
        }

        self.supervisors
            .write()
            .await
            .entry(ActorKey::new(session_id, &template.supervisor_id))
            .or_default()
            .track(actor_id.clone());

        self.emit_lifecycle(
            &template.session_id,
            ActorLifecycleEvent::StatusChanged {
                actor_id: actor_id.clone(),
                status: ActorStatus::Running,
                at: spawned_at,
            },
        );
        self.emit_lifecycle(
            &template.session_id,
            ActorLifecycleEvent::Spawned {
                actor_id: actor_id.clone(),
                parent_id: template.parent.clone(),
                role: template.role.clone(),
                task: template.task.clone(),
                depth: template.depth,
                budget: template.budget.clone(),
                spawned_at,
            },
        );

        Some(template.to_spec(cancellation))
    }
}

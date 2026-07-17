use super::super::*;
use tm_host::{CapabilityGrants, InvocationCtx};

const TEST_SESSION: &str = "";

pub(super) fn make_roster() -> Arc<MailboxRegistry> {
    Arc::new(MailboxRegistry::new())
}

pub(super) fn ctx_without_agents() -> InvocationCtx {
    InvocationCtx::new(CapabilityGrants::default())
}

pub(super) fn ctx_with(cap: &str) -> InvocationCtx {
    InvocationCtx::new(CapabilityGrants::default().allow(cap))
}

pub(super) fn ctx_with_actor(cap: &str, actor_id: &str) -> InvocationCtx {
    InvocationCtx::new(CapabilityGrants::default().allow(cap))
        .with_actor_id(Some(actor_id.to_string()))
}

pub(super) async fn track_running(roster: &MailboxRegistry, id: &str) {
    track_actor(roster, id, None, ActorStatus::Running).await;
}

pub(super) async fn track_actor(
    roster: &MailboxRegistry,
    id: &str,
    parent: Option<&str>,
    status: ActorStatus,
) {
    roster
        .track_for_session(
            TEST_SESSION,
            crate::actor::ActorRecord {
                id: ActorId::new(id).unwrap(),
                parent: parent.map(|id| ActorId::new(id).unwrap()),
                status,
                mode: Some("worker".to_string()),
                budget: ActorBudget::default(),
                spawned_at: chrono::Utc::now(),
                completed_at: matches!(status, ActorStatus::Terminated).then(chrono::Utc::now),
                cancelled: false,
                failure_reason: None,
                last_summary: None,
                artifact_uri: None,
                history_uri: None,
            },
        )
        .await
        .unwrap();
}

// ─── P3.5: lifecycle event emission tests ────────────────────────────────

pub(super) fn lifecycle_hook() -> (
    Arc<std::sync::Mutex<Vec<ActorLifecycleEvent>>>,
    impl Fn(String, ActorLifecycleEvent) + Send + Sync + 'static,
) {
    let captured: Arc<std::sync::Mutex<Vec<ActorLifecycleEvent>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured2 = Arc::clone(&captured);
    let hook = move |_session_id: String, event: ActorLifecycleEvent| {
        captured2.lock().unwrap().push(event);
    };
    (captured, hook)
}

pub(super) struct EchoExec;
#[async_trait::async_trait]
impl crate::executor::ActorExecutor for EchoExec {
    async fn run_to_digest(
        &self,
        spec: crate::actor::ActorSpec,
    ) -> std::result::Result<crate::actor::ActorDigest, crate::executor::ActorError> {
        Ok(crate::actor::ActorDigest {
            actor_id: spec.id,
            summary: format!("echo: {}", spec.task),
            artifact_uri: None,
            history_uri: None,
            history_content: None,
        })
    }
}

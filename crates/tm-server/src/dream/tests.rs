use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use tm_memory::{
    DreamInputBudget, DreamLease, DreamQueueRecord, DreamReason, DreamStatus, DreamWorker,
    DreamWorkerReport, MemorySummaryKind, MemorySummaryRecord, NewDreamQueueRecord,
    NewMemorySummaryRecord, NewSkillProposalRecord, SkillProposalRecord, SkillProposalStatus,
};
use tm_modes::{AssetStatus, ModeId, ModesConfig};
use tokio::sync::broadcast;
use uuid::Uuid;

use super::worker::SenderFactory;
use super::*;
use crate::store::StoreRuntimeMetrics;
use crate::{
    ApprovalBroker, ApprovalEffectLease, ApprovalEffectRecord, ApprovalRequestRecord,
    ApprovalResolveDecision, CronJobRecord, CronLease, CronRunRecord, EndSessionDreamResult,
    InMemoryStore, MemoryWriteStatus, MessageRecord, NewApprovalRequest, NewApprovalResolution,
    NewCronJobRecord, NewCronRunRecord, NewProjectItem, NewSession, ProfileFactRecord,
    ProjectItemKind, ProjectItemRecord, RecallChunkRecord, ResolveApprovalRequest, Result,
    ServerError, SessionEvent, SessionRecord, SessionSummaryRecord, SessionTurnRecord, Store,
    StoreCodingEventSink,
};

struct ClaimFailureStore;

#[async_trait]
impl Store for ClaimFailureStore {
    async fn create_session(&self, _new: NewSession) -> Result<SessionRecord> {
        panic!("unused store method create_session")
    }

    async fn configure_owner_subject(&self, _owner_subject: &str) -> Result<usize> {
        panic!("unused store method configure_owner_subject")
    }

    async fn set_session_memory_scope(
        &self,
        _session_id: Uuid,
        _memory_scope: &str,
    ) -> Result<SessionRecord> {
        panic!("unused store method set_session_memory_scope")
    }

    async fn end_session(&self, _session_id: Uuid) -> Result<SessionRecord> {
        panic!("unused store method end_session")
    }

    async fn end_session_and_enqueue_dream(
        &self,
        _session_id: Uuid,
        _subject: String,
        _scope: String,
    ) -> Result<EndSessionDreamResult> {
        panic!("unused store method end_session_and_enqueue_dream")
    }

    async fn list_sessions(&self, _limit: usize) -> Result<Vec<SessionSummaryRecord>> {
        panic!("unused store method list_sessions")
    }

    async fn get_session(&self, _session_id: Uuid) -> Result<SessionRecord> {
        panic!("unused store method get_session")
    }

    async fn session_messages(&self, _session_id: Uuid) -> Result<Vec<MessageRecord>> {
        panic!("unused store method session_messages")
    }

    async fn set_mode_state(
        &self,
        _session_id: Uuid,
        _mode_state: crate::ModeState,
    ) -> Result<SessionRecord> {
        panic!("unused store method set_mode_state")
    }

    async fn append_message(
        &self,
        _session_id: Uuid,
        _role: &str,
        _content: &str,
    ) -> Result<MessageRecord> {
        panic!("unused store method append_message")
    }

    async fn append_event(
        &self,
        _session_id: Uuid,
        _event_type: &str,
        _payload_json: Value,
    ) -> Result<SessionEvent> {
        panic!("unused store method append_event")
    }

    async fn events_after(
        &self,
        _session_id: Uuid,
        _last_event_id: Option<i64>,
    ) -> Result<Vec<SessionEvent>> {
        panic!("unused store method events_after")
    }

    async fn enqueue_turn(
        &self,
        _session_id: Uuid,
        _client_message_id: &str,
        _content: &str,
    ) -> Result<SessionTurnRecord> {
        panic!("unused store method enqueue_turn")
    }

    async fn turn(&self, _turn_id: Uuid) -> Result<SessionTurnRecord> {
        panic!("unused store method turn")
    }

    async fn claim_next_turn(
        &self,
        _worker_id: Uuid,
        _now: chrono::DateTime<Utc>,
    ) -> Result<Option<SessionTurnRecord>> {
        panic!("unused store method claim_next_turn")
    }

    async fn heartbeat_turn(
        &self,
        _turn_id: Uuid,
        _worker_id: Uuid,
        _now: chrono::DateTime<Utc>,
    ) -> Result<SessionTurnRecord> {
        panic!("unused store method heartbeat_turn")
    }

    async fn fail_stale_running_turns(
        &self,
        _stale_before: chrono::DateTime<Utc>,
        _failed_at: chrono::DateTime<Utc>,
        _error: &str,
    ) -> Result<usize> {
        panic!("unused store method fail_stale_running_turns")
    }

    async fn complete_turn(
        &self,
        _turn_id: Uuid,
        _worker_id: Uuid,
        _assistant_content: &str,
        _completed_at: chrono::DateTime<Utc>,
    ) -> Result<SessionTurnRecord> {
        panic!("unused store method complete_turn")
    }

    async fn fail_turn(
        &self,
        _turn_id: Uuid,
        _worker_id: Uuid,
        _error: &str,
        _failed_at: chrono::DateTime<Utc>,
    ) -> Result<SessionTurnRecord> {
        panic!("unused store method fail_turn")
    }

    async fn create_approval_request(
        &self,
        _request: NewApprovalRequest,
    ) -> Result<ApprovalRequestRecord> {
        panic!("unused store method create_approval_request")
    }

    async fn approval_request(
        &self,
        _session_id: Uuid,
        _approval_id: Uuid,
    ) -> Result<ApprovalRequestRecord> {
        panic!("unused store method approval_request")
    }

    async fn heartbeat_approval_request(
        &self,
        _approval_id: Uuid,
        _requester_id: Uuid,
        _now: chrono::DateTime<Utc>,
    ) -> Result<ApprovalRequestRecord> {
        panic!("unused store method heartbeat_approval_request")
    }

    async fn resolve_approval_request(
        &self,
        _session_id: Uuid,
        _approval_id: Uuid,
        _resolution: NewApprovalResolution,
    ) -> Result<ApprovalRequestRecord> {
        panic!("unused store method resolve_approval_request")
    }

    async fn resolve_approval_request_with_event(
        &self,
        _session_id: Uuid,
        _approval_id: Uuid,
        _resolution: NewApprovalResolution,
    ) -> Result<(ApprovalRequestRecord, SessionEvent)> {
        panic!("unused store method resolve_approval_request_with_event")
    }

    async fn link_approval_event(
        &self,
        _session_id: Uuid,
        _approval_id: Uuid,
        _event_type: &str,
        _event_seq: i64,
    ) -> Result<ApprovalRequestRecord> {
        panic!("unused store method link_approval_event")
    }

    async fn cancel_stale_non_resumable_approvals(
        &self,
        _stale_before: chrono::DateTime<Utc>,
        _cancelled_at: chrono::DateTime<Utc>,
    ) -> Result<Vec<SessionEvent>> {
        panic!("unused store method cancel_stale_non_resumable_approvals")
    }

    async fn expire_pending_approvals(
        &self,
        _now: chrono::DateTime<Utc>,
    ) -> Result<Vec<SessionEvent>> {
        panic!("unused store method expire_pending_approvals")
    }

    async fn claim_approval_effect(
        &self,
        _approval_id: Uuid,
        _owner_id: Uuid,
        _now: chrono::DateTime<Utc>,
        _lease_timeout: Duration,
    ) -> Result<Option<ApprovalEffectLease>> {
        panic!("unused store method claim_approval_effect")
    }

    async fn claim_next_approval_effect(
        &self,
        _owner_id: Uuid,
        _now: chrono::DateTime<Utc>,
        _lease_timeout: Duration,
    ) -> Result<Option<ApprovalEffectLease>> {
        panic!("unused store method claim_next_approval_effect")
    }

    async fn complete_approval_effect(
        &self,
        _lease: &ApprovalEffectLease,
        _applied_at: chrono::DateTime<Utc>,
    ) -> Result<ApprovalEffectRecord> {
        panic!("unused store method complete_approval_effect")
    }

    async fn complete_approval_effect_with_event(
        &self,
        _lease: &ApprovalEffectLease,
        _proposal_payload_json: Value,
        _turn_id: Option<Uuid>,
        _applied_at: chrono::DateTime<Utc>,
    ) -> Result<(ApprovalEffectRecord, SessionEvent)> {
        panic!("unused store method complete_approval_effect_with_event")
    }

    async fn fail_approval_effect(
        &self,
        _lease: &ApprovalEffectLease,
        _error: &str,
        _next_available_at: chrono::DateTime<Utc>,
        _max_attempts: i32,
    ) -> Result<ApprovalEffectRecord> {
        panic!("unused store method fail_approval_effect")
    }

    async fn add_profile_fact(&self, _fact: ProfileFactRecord) -> Result<()> {
        panic!("unused store method add_profile_fact")
    }

    async fn add_recall_chunk(&self, _chunk: RecallChunkRecord) -> Result<()> {
        panic!("unused store method add_recall_chunk")
    }

    async fn upsert_profile_fact(&self, _fact: ProfileFactRecord) -> Result<ProfileFactRecord> {
        panic!("unused store method upsert_profile_fact")
    }

    async fn upsert_recall_chunk(&self, _chunk: RecallChunkRecord) -> Result<RecallChunkRecord> {
        panic!("unused store method upsert_recall_chunk")
    }

    async fn profile_facts(&self, _subject: &str) -> Result<Vec<ProfileFactRecord>> {
        panic!("unused store method profile_facts")
    }

    async fn profile_fact(&self, _subject: &str, _id: Uuid) -> Result<ProfileFactRecord> {
        panic!("unused store method profile_fact")
    }

    async fn recall_chunks(
        &self,
        _scope: &str,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<RecallChunkRecord>> {
        panic!("unused store method recall_chunks")
    }

    async fn recall_chunk(&self, _scope: &str, _id: Uuid) -> Result<RecallChunkRecord> {
        panic!("unused store method recall_chunk")
    }

    async fn upsert_project_item(&self, _item: NewProjectItem) -> Result<ProjectItemRecord> {
        panic!("unused store method upsert_project_item")
    }

    async fn project_items(
        &self,
        _project_id: &str,
        _kind: Option<ProjectItemKind>,
    ) -> Result<Vec<ProjectItemRecord>> {
        panic!("unused store method project_items")
    }

    async fn enqueue_dream(&self, _new: NewDreamQueueRecord) -> Result<DreamQueueRecord> {
        panic!("unused store method enqueue_dream")
    }

    async fn dream_queue_for_session(&self, _session_id: Uuid) -> Result<Vec<DreamQueueRecord>> {
        panic!("unused store method dream_queue_for_session")
    }

    async fn dream_queue(&self, _scope: &str, _limit: usize) -> Result<Vec<DreamQueueRecord>> {
        panic!("unused store method dream_queue")
    }

    async fn dream(&self, _dream_id: Uuid) -> Result<DreamQueueRecord> {
        panic!("unused store method dream")
    }

    async fn claim_ready_dream(
        &self,
        _now: chrono::DateTime<Utc>,
        _lease_timeout: Duration,
        _owner_id: Uuid,
    ) -> Result<Option<DreamLease>> {
        Err(ServerError::Store("claim failed".to_string()))
    }

    async fn heartbeat_dream(
        &self,
        _lease: &DreamLease,
        _now: chrono::DateTime<Utc>,
    ) -> Result<DreamLease> {
        panic!("unused store method heartbeat_dream")
    }

    async fn complete_dream(
        &self,
        _lease: &DreamLease,
        _now: chrono::DateTime<Utc>,
    ) -> Result<DreamQueueRecord> {
        panic!("unused store method complete_dream")
    }

    async fn fail_dream(
        &self,
        _lease: &DreamLease,
        _error: String,
        _next_available_at: chrono::DateTime<Utc>,
        _max_attempts: i32,
    ) -> Result<DreamQueueRecord> {
        panic!("unused store method fail_dream")
    }

    async fn upsert_memory_summary(
        &self,
        _summary: NewMemorySummaryRecord,
    ) -> Result<MemorySummaryRecord> {
        panic!("unused store method upsert_memory_summary")
    }

    async fn memory_summary(&self, _id: Uuid) -> Result<MemorySummaryRecord> {
        panic!("unused store method memory_summary")
    }

    async fn memory_summaries(
        &self,
        _scope: &str,
        _limit: usize,
    ) -> Result<Vec<MemorySummaryRecord>> {
        panic!("unused store method memory_summaries")
    }

    async fn upsert_skill_proposal(
        &self,
        _proposal: NewSkillProposalRecord,
    ) -> Result<SkillProposalRecord> {
        panic!("unused store method upsert_skill_proposal")
    }

    async fn update_skill_proposal_status(
        &self,
        _id: Uuid,
        _status: SkillProposalStatus,
    ) -> Result<SkillProposalRecord> {
        panic!("unused store method update_skill_proposal_status")
    }

    async fn skill_proposal(&self, _id: Uuid) -> Result<SkillProposalRecord> {
        panic!("unused store method skill_proposal")
    }

    async fn skill_proposals_for_session(
        &self,
        _session_id: Uuid,
    ) -> Result<Vec<SkillProposalRecord>> {
        panic!("unused store method skill_proposals_for_session")
    }

    async fn upsert_cron_job(&self, _job: NewCronJobRecord) -> Result<CronJobRecord> {
        panic!("unused store method upsert_cron_job")
    }

    async fn cron_job(&self, _id: &str) -> Result<CronJobRecord> {
        panic!("unused store method cron_job")
    }

    async fn cron_jobs(&self) -> Result<Vec<CronJobRecord>> {
        panic!("unused store method cron_jobs")
    }

    async fn materialize_cron_run(
        &self,
        _run: NewCronRunRecord,
        _expected_next_run_at: chrono::DateTime<Utc>,
        _next_run_at: chrono::DateTime<Utc>,
    ) -> Result<Option<CronRunRecord>> {
        panic!("unused store method materialize_cron_run")
    }

    async fn claim_ready_cron_run(
        &self,
        _owner_id: Uuid,
        _now: chrono::DateTime<Utc>,
        _lease_timeout: Duration,
        _max_attempts: i32,
    ) -> Result<Option<CronLease>> {
        panic!("unused store method claim_ready_cron_run")
    }

    async fn claim_cron_run(
        &self,
        _run: NewCronRunRecord,
        _owner_id: Uuid,
        _now: chrono::DateTime<Utc>,
        _lease_timeout: Duration,
    ) -> Result<(CronLease, bool)> {
        panic!("unused store method claim_cron_run")
    }

    async fn record_cron_run(&self, _run: NewCronRunRecord) -> Result<CronRunRecord> {
        panic!("unused store method record_cron_run")
    }

    async fn heartbeat_cron_run(
        &self,
        _lease: &CronLease,
        _now: chrono::DateTime<Utc>,
    ) -> Result<CronLease> {
        panic!("unused store method heartbeat_cron_run")
    }

    async fn complete_cron_run(
        &self,
        _lease: &CronLease,
        _status: &str,
        _session_id: Option<Uuid>,
        _result_json: Value,
    ) -> Result<CronRunRecord> {
        panic!("unused store method complete_cron_run")
    }

    async fn fail_cron_run(
        &self,
        _lease: &CronLease,
        _error: String,
        _next_available_at: chrono::DateTime<Utc>,
        _max_attempts: i32,
    ) -> Result<CronRunRecord> {
        panic!("unused store method fail_cron_run")
    }

    async fn cron_runs(&self, _job_id: &str, _limit: usize) -> Result<Vec<CronRunRecord>> {
        panic!("unused store method cron_runs")
    }

    async fn runtime_metrics(&self, _now: chrono::DateTime<Utc>) -> Result<StoreRuntimeMetrics> {
        panic!("unused store method runtime_metrics")
    }
}

fn test_sender_factory() -> SenderFactory {
    let senders = Arc::new(Mutex::new(
        BTreeMap::<Uuid, broadcast::Sender<SessionEvent>>::new(),
    ));
    Arc::new(move |session_id| {
        let mut senders = senders.lock().expect("sender map lock");
        senders
            .entry(session_id)
            .or_insert_with(|| broadcast::channel(64).0)
            .clone()
    })
}

struct FailingPublishSink;

#[async_trait]
impl crate::CodingEventSink for FailingPublishSink {
    async fn emit(&self, _event_type: &str, _payload_json: Value) -> Result<SessionEvent> {
        panic!("approval proposal finalization must publish an already-persisted event")
    }

    async fn publish_persisted(&self, _event: SessionEvent) -> Result<()> {
        Err(ServerError::Store(
            "simulated post-commit broadcast failure".to_string(),
        ))
    }
}

struct PersistedOnlySink;

#[async_trait]
impl crate::CodingEventSink for PersistedOnlySink {
    async fn emit(&self, _event_type: &str, _payload_json: Value) -> Result<SessionEvent> {
        panic!("durable approval test helper must not append a second event")
    }
}

async fn resolve_and_apply_durable_approval(
    store: &Arc<InMemoryStore>,
    broker: &Arc<ApprovalBroker>,
    sender_for: &SenderFactory,
    session_id: Uuid,
    approval_id: Uuid,
    request: ResolveApprovalRequest,
) {
    let sink: Arc<dyn crate::CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session_id,
        Arc::clone(store),
        sender_for(session_id),
    ));
    broker
        .resolve_persisted(session_id, approval_id, request, Arc::clone(&sink))
        .await
        .unwrap();
    let lease = store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            Utc::now(),
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("resolved approval effect");
    let approval = store
        .approval_request(session_id, approval_id)
        .await
        .unwrap();
    crate::api::approvals::apply_approval_effect_lease(store.as_ref(), &approval, &lease, sink)
        .await
        .unwrap();
}

#[tokio::test]
async fn approval_effect_publish_failure_does_not_requeue_committed_mutation() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let now = Utc::now();
    let proposal = crate::memory::MemoryWriteProposal::recall_chunk(
        "brian".to_string(),
        "global".to_string(),
        "persist this exactly once".to_string(),
        "fault-test".to_string(),
        "fault-test".to_string(),
        json!({"source": "fault-test"}),
        now,
    )
    .unwrap();
    let proposal_id = proposal.proposal_id;
    let approval_id = Uuid::new_v4();
    store
        .create_approval_request(NewApprovalRequest {
            id: approval_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "fault-test".to_string(),
            action: "memory.write".to_string(),
            scope_json: json!({"scope": "global"}),
            options_json: json!([]),
            effect_type: "memory_write".to_string(),
            effect_payload_json: json!({"proposal": proposal}),
            resumable: true,
            created_at: now,
            expires_at: now + Duration::minutes(5),
        })
        .await
        .unwrap();
    store
        .resolve_approval_request_with_event(
            session.id,
            approval_id,
            NewApprovalResolution {
                status: "approved".to_string(),
                selected_option_id: Some("allow".to_string()),
                resolution_json: json!({
                    "approvalId": approval_id,
                    "status": "approved",
                    "outcome": "approved",
                }),
                resolved_at: now + Duration::seconds(1),
            },
        )
        .await
        .unwrap();
    let lease = store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            now + Duration::seconds(1),
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("resolved memory effect");
    let approval = store
        .approval_request(session.id, approval_id)
        .await
        .unwrap();

    let error = crate::api::approvals::apply_approval_effect_lease(
        store.as_ref(),
        &approval,
        &lease,
        Arc::new(FailingPublishSink),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("post-commit broadcast failure"));
    assert!(
        store
            .claim_approval_effect(
                approval_id,
                Uuid::new_v4(),
                now + Duration::minutes(2),
                Duration::seconds(30),
            )
            .await
            .unwrap()
            .is_none(),
        "a broadcast failure must not requeue an applied effect"
    );
    assert_eq!(
        store.recall_chunks("global", "", 10).await.unwrap().len(),
        1
    );
    assert_eq!(
        store
            .events_after(session.id, None)
            .await
            .unwrap()
            .iter()
            .filter(|event| {
                event.event_type == "write_proposal"
                    && event.payload_json["proposalId"] == json!(proposal_id)
                    && event.payload_json["status"] == json!("approved")
            })
            .count(),
        1
    );
}

async fn wait_for_event_count(
    store: &InMemoryStore,
    session_id: Uuid,
    event_type: &str,
    count: usize,
) -> Vec<SessionEvent> {
    for _ in 0..100 {
        if event_type == "approval_resolved" {
            store.expire_pending_approvals(Utc::now()).await.unwrap();
            loop {
                let Some(lease) = store
                    .claim_next_approval_effect(Uuid::new_v4(), Utc::now(), Duration::seconds(30))
                    .await
                    .unwrap()
                else {
                    break;
                };
                let approval = store
                    .approval_request(lease.effect.session_id, lease.effect.approval_id)
                    .await
                    .unwrap();
                crate::api::approvals::apply_approval_effect_lease(
                    store,
                    &approval,
                    &lease,
                    Arc::new(PersistedOnlySink),
                )
                .await
                .unwrap();
            }
        }
        let events = store.events_after(session_id, None).await.unwrap();
        if events
            .iter()
            .filter(|event| event.event_type == event_type)
            .count()
            >= count
        {
            return events;
        }
        tokio::time::sleep(StdDuration::from_millis(5)).await;
    }
    panic!("timed out waiting for {count} {event_type} events");
}

async fn wait_for_skill_status(
    store: &InMemoryStore,
    session_id: Uuid,
    status: SkillProposalStatus,
) -> SkillProposalRecord {
    for _ in 0..100 {
        let proposals = store.skill_proposals_for_session(session_id).await.unwrap();
        if let Some(proposal) = proposals
            .into_iter()
            .find(|proposal| proposal.status == status)
        {
            return proposal;
        }
        tokio::time::sleep(StdDuration::from_millis(5)).await;
    }
    panic!("timed out waiting for skill proposal status {status}");
}

async fn wait_for_memory_write_status(
    store: &InMemoryStore,
    session_id: Uuid,
    status: MemoryWriteStatus,
) -> Value {
    for _ in 0..100 {
        let events = store.events_after(session_id, None).await.unwrap();
        if let Some(event) = events.into_iter().find(|event| {
            event.event_type == "write_proposal"
                && event.payload_json["kind"] == json!("memory")
                && event.payload_json["status"] == json!(status)
        }) {
            return event.payload_json;
        }
        tokio::time::sleep(StdDuration::from_millis(5)).await;
    }
    panic!("timed out waiting for memory proposal status {status:?}");
}

#[test]
fn dream_worker_config_defaults_cover_p4_knobs() {
    let config = DreamWorkerConfig::default();

    assert!(config.enabled);
    assert!(config.redaction.enabled);
    assert!(config.summary_cadence.session_every_dream);
    assert!(config.summary_cadence.rollup_every_dream);
    assert_eq!(config.retry_backoff, Duration::seconds(30));
    assert_eq!(config.input_budget, DreamInputBudget::default());
    assert_eq!(config.reflect_importance_threshold, 1.5);
    assert_eq!(config.model_roles.reflection, "cheap");
}

#[tokio::test]
async fn dream_worker_store_claim_failure_is_reported_and_trait_degrades() {
    let store = Arc::new(ClaimFailureStore);
    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig::default(),
    );

    let err = worker.run_once_result().await.unwrap_err();
    assert_eq!(err.to_string(), "store error: claim failed");
    assert_eq!(worker.run_once().await, DreamWorkerReport::default());
}

#[tokio::test]
async fn dream_worker_writes_summary_and_defers_proposals() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "Workflow: when I ask for release notes, gather commits then draft concise notes. token=sk-testsecret123456",
        )
        .await
        .unwrap();
    store
        .append_message(session.id, "user", "I prefer small, reviewable patches.")
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "assistant",
            "Captured the release-note workflow and left artifact://0 for review.",
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(
            session.id,
            "session_end",
            json!({"status": "ended", "reason": "test"}),
        )
        .await
        .unwrap();
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:test:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            proposal_timeout: StdDuration::from_millis(20),
            ..DreamWorkerConfig::default()
        },
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 2);
    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[0].id, dream.id);
    assert_eq!(dreams[0].status, DreamStatus::Completed);

    let summaries = store.memory_summaries("global", 10).await.unwrap();
    let session_summary = summaries
        .iter()
        .find(|summary| summary.kind == MemorySummaryKind::Session)
        .expect("session summary");
    assert!(session_summary.body.contains("[REDACTED_SECRET]"));
    assert!(!session_summary.body.contains("sk-testsecret123456"));
    assert_eq!(session_summary.source_dream_id, dream.id);
    assert!(
        session_summary
            .evidence
            .iter()
            .any(|evidence| evidence.event_seq == Some(ended.seq))
    );

    let proposals = store.skill_proposals_for_session(session.id).await.unwrap();
    assert_eq!(proposals.len(), 1);
    assert!(proposals[0].verification.passed);
    assert!(matches!(
        proposals[0].status,
        SkillProposalStatus::Pending | SkillProposalStatus::TimedOut
    ));

    let events = wait_for_event_count(&store, session.id, "approval_resolved", 2).await;
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_started")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_completed")
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "write_proposal")
            .count(),
        4
    );
    assert_eq!(
        store.profile_facts("brian").await.unwrap(),
        Vec::new(),
        "timed-out dream memory approval must not write profile facts"
    );
    assert_eq!(
        store.skill_proposal(proposals[0].id).await.unwrap().status,
        SkillProposalStatus::TimedOut
    );
}

#[tokio::test]
async fn concurrent_dream_workers_do_not_complete_same_dream_twice() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(session.id, "assistant", "Done and ready for review.")
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:race:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let broker = Arc::new(ApprovalBroker::default());
    let worker_a = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::clone(&broker),
        test_sender_factory(),
        DreamWorkerConfig::default(),
    );
    let worker_b = ServerDreamWorker::new(
        Arc::clone(&store),
        broker,
        test_sender_factory(),
        DreamWorkerConfig::default(),
    );

    let (first, second) = tokio::join!(worker_a.run_once_result(), worker_b.run_once_result());
    let reports = [first.unwrap(), second.unwrap()];
    assert_eq!(
        reports.iter().map(|report| report.attempted).sum::<usize>(),
        1
    );
    assert_eq!(
        reports.iter().map(|report| report.completed).sum::<usize>(),
        1
    );
    assert_eq!(
        reports.iter().map(|report| report.proposals).sum::<usize>(),
        0
    );

    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams.len(), 1);
    assert_eq!(dreams[0].status, DreamStatus::Completed);
    let summaries = store.memory_summaries("global", 10).await.unwrap();
    assert_eq!(
        summaries
            .iter()
            .filter(|summary| summary.kind == MemorySummaryKind::Session)
            .count(),
        1
    );
    assert_eq!(
        summaries
            .iter()
            .filter(|summary| summary.kind == MemorySummaryKind::TopicProject)
            .count(),
        1
    );

    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "dream_started")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "dream_completed")
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "dream_failed")
    );
}

#[tokio::test]
async fn completed_dream_rerun_does_not_duplicate_memory_or_approvals() {
    let store = Arc::new(InMemoryStore::default());
    let broker = Arc::new(ApprovalBroker::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "Remember that I prefer deterministic dream reruns.",
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    let new_dream = NewDreamQueueRecord {
        session_id: session.id,
        subject: "brian".to_string(),
        scope: "global".to_string(),
        reason: DreamReason::SessionEnded,
        dedupe_key: format!("dream:rerun:{}", session.id),
        source_event_seq: Some(ended.seq),
        available_at: Utc::now(),
    };
    let dream = store.enqueue_dream(new_dream.clone()).await.unwrap();

    let sender_for = test_sender_factory();
    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::clone(&broker),
        Arc::clone(&sender_for),
        DreamWorkerConfig {
            proposal_timeout: StdDuration::from_secs(5),
            ..DreamWorkerConfig::default()
        },
    );
    let first = worker.run_once_result().await.unwrap();
    assert_eq!(first.attempted, 1);
    assert_eq!(first.completed, 1);
    assert_eq!(first.proposals, 1);

    let events = wait_for_event_count(&store, session.id, "approval", 1).await;
    let approval_id = events
        .iter()
        .find(|event| event.event_type == "approval")
        .unwrap()
        .payload_json["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_and_apply_durable_approval(
        &store,
        &broker,
        &sender_for,
        session.id,
        approval_id,
        ResolveApprovalRequest {
            decision: ApprovalResolveDecision::Approve,
            option_id: Some("allow".to_string()),
        },
    )
    .await;
    wait_for_memory_write_status(&store, session.id, MemoryWriteStatus::Approved).await;

    let duplicate = store.enqueue_dream(new_dream).await.unwrap();
    assert_eq!(duplicate.id, dream.id);
    assert_eq!(duplicate.status, DreamStatus::Completed);
    let second = worker.run_once_result().await.unwrap();
    assert_eq!(second, DreamWorkerReport::default());

    let facts = store.profile_facts("brian").await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].object, "deterministic dream reruns");
    let summaries = store.memory_summaries("global", 10).await.unwrap();
    assert_eq!(
        summaries
            .iter()
            .filter(|summary| summary.kind == MemorySummaryKind::Session)
            .count(),
        1
    );
    assert_eq!(
        summaries
            .iter()
            .filter(|summary| summary.kind == MemorySummaryKind::TopicProject)
            .count(),
        1
    );
    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "approval")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.event_type == "write_proposal"
                    && event.payload_json["status"] == json!("approved")
            })
            .count(),
        1
    );
}

#[tokio::test]
async fn dream_worker_timeout_failure_is_replayable_and_bounded() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(session.id, "user", "Summarize this before the timeout.")
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:timeout:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            per_dream_timeout: StdDuration::ZERO,
            retry_backoff: Duration::zero(),
            max_attempts: 2,
            ..DreamWorkerConfig::default()
        },
    );

    let first = worker.run_once_result().await.unwrap();
    assert_eq!(first.attempted, 1);
    assert_eq!(first.completed, 0);
    let retryable = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(retryable[0].status, DreamStatus::Queued);
    assert_eq!(retryable[0].attempts, 1);
    assert_eq!(
        retryable[0].last_error.as_deref(),
        Some("store error: dream timed out")
    );

    let second = worker.run_once_result().await.unwrap();
    assert_eq!(second.attempted, 1);
    assert_eq!(second.completed, 0);
    let terminal = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(terminal[0].status, DreamStatus::Failed);
    assert_eq!(terminal[0].attempts, 2);
    assert_eq!(
        terminal[0].last_error.as_deref(),
        Some("store error: dream timed out")
    );

    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "dream_started")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "dream_failed")
            .count(),
        2
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "dream_completed")
    );
}

#[tokio::test]
async fn dream_worker_redaction_disabled_fails_visibly_without_writes() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "Remember that I prefer this should not be written when redaction is disabled.",
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:redaction-disabled:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            redaction: DreamRedactionConfig { enabled: false },
            retry_backoff: Duration::zero(),
            max_attempts: 1,
            ..DreamWorkerConfig::default()
        },
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 0);
    assert_eq!(report.proposals, 0);
    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[0].status, DreamStatus::Failed);
    assert_eq!(
        dreams[0].last_error.as_deref(),
        Some("policy error: dream redaction is disabled by config")
    );
    assert!(
        store
            .memory_summaries("global", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.profile_facts("brian").await.unwrap().is_empty());
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_failed")
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "write_proposal")
    );
}

#[tokio::test]
async fn dream_worker_missing_model_role_fails_visibly_without_writes() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(session.id, "user", "Decision: keep model config explicit.")
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:missing-model-role:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();
    let mut model_roles = DreamModelRoles::default();
    model_roles.extraction.clear();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            model_roles,
            retry_backoff: Duration::zero(),
            max_attempts: 1,
            ..DreamWorkerConfig::default()
        },
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 0);
    assert_eq!(report.proposals, 0);
    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[0].status, DreamStatus::Failed);
    assert_eq!(
        dreams[0].last_error.as_deref(),
        Some("policy error: dream model role extraction is not configured")
    );
    assert!(
        store
            .memory_summaries("global", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .recall_chunks("global", "model config", 10)
            .await
            .unwrap()
            .is_empty()
    );
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_failed")
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "write_proposal")
    );
}

#[tokio::test]
async fn dream_worker_daemon_processes_queue_and_stops_cleanly() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "Close this session with a compact summary.",
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:daemon:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let daemon = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            poll_interval: Duration::milliseconds(5),
            ..DreamWorkerConfig::default()
        },
    )
    .into_daemon();
    let handle = daemon.spawn();
    wait_for_event_count(&store, session.id, "dream_completed", 1).await;
    handle.request_shutdown();
    handle.shutdown().await.unwrap();

    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[0].status, DreamStatus::Completed);
    assert_eq!(dreams[0].locked_at, None);
}

#[tokio::test]
async fn dream_worker_budgets_redacted_input_before_summary_and_proposals() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "Remember that I prefer compact dream summaries.",
        )
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "token=sk-testsecret123456 and this long note should be trimmed before it can sprawl through the dream input collector.",
        )
        .await
        .unwrap();
    for index in 0..12 {
        store
            .append_message(
                session.id,
                "user",
                &format!(
                    "Remember that I prefer omitted preference {index}. {}",
                    "filler ".repeat(20)
                ),
            )
            .await
            .unwrap();
    }
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:budget:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            proposal_timeout: StdDuration::from_millis(20),
            input_budget: DreamInputBudget {
                max_chunks: 1,
                max_chunk_chars: 220,
                max_message_chars: 80,
            },
            ..DreamWorkerConfig::default()
        },
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 1);
    let summaries = store.memory_summaries("global", 10).await.unwrap();
    let session_summary = summaries
        .iter()
        .find(|summary| summary.kind == MemorySummaryKind::Session)
        .expect("session summary");
    assert_eq!(session_summary.source_dream_id, dream.id);
    assert!(
        session_summary
            .evidence
            .iter()
            .any(|evidence| evidence.event_seq == Some(ended.seq))
    );
    assert!(session_summary.body.contains("Input budget: included"));
    assert!(!session_summary.body.contains("sk-testsecret123456"));

    let events = wait_for_event_count(&store, session.id, "approval_resolved", 1).await;
    let input = events
        .iter()
        .find(|event| {
            event.event_type == "dream_progress"
                && event.payload_json["phase"] == json!("input_collected")
        })
        .unwrap();
    assert_eq!(input.payload_json["totalMessages"], json!(14));
    assert!(input.payload_json["omittedMessages"].as_u64().unwrap() > 0);
    assert!(input.payload_json["truncatedMessages"].as_u64().unwrap() > 0);
    assert_eq!(input.payload_json["inputChunks"], json!(1));
    assert_eq!(input.payload_json["inputTruncated"], json!(true));
    assert_eq!(input.payload_json["redactedMessages"], json!(1));
    assert_eq!(
        store.profile_facts("brian").await.unwrap(),
        Vec::new(),
        "timed-out budgeted proposal must not write profile facts"
    );
    let proposal_events = events
        .iter()
        .filter(|event| event.event_type == "write_proposal")
        .collect::<Vec<_>>();
    assert_eq!(proposal_events.len(), 2);
    assert!(proposal_events.iter().all(|event| {
        !event
            .payload_json
            .to_string()
            .contains("omitted preference")
    }));
}

#[tokio::test]
async fn dream_worker_writes_reflection_when_importance_crosses_threshold() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "Deadline: send the P4 notes by Friday.\nDecision: keep reflection summaries evidence-cited.",
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:reflection:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            proposal_timeout: StdDuration::from_millis(20),
            reflect_importance_threshold: 1.5,
            ..DreamWorkerConfig::default()
        },
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 2);
    let summaries = store.memory_summaries("global", 10).await.unwrap();
    let reflection = summaries
        .iter()
        .find(|summary| summary.kind == MemorySummaryKind::Reflection)
        .expect("reflection summary");
    assert!(reflection.body.contains("Cumulative importance"));
    assert!(reflection.body.contains("Deadline: send the P4 notes"));
    assert!(
        reflection
            .body
            .contains("Decision: keep reflection summaries evidence-cited")
    );
    assert!(reflection.body.contains("Evidence citations"));
    assert_eq!(reflection.source_session_id, Some(session.id));
    assert!(
        reflection
            .evidence
            .iter()
            .any(|evidence| evidence.event_seq == Some(ended.seq))
    );

    let events = wait_for_event_count(&store, session.id, "approval_resolved", 2).await;
    assert!(events.iter().any(|event| {
        event.event_type == "dream_progress"
            && event.payload_json["phase"] == json!("reflection_written")
    }));
}

#[tokio::test]
async fn dream_worker_updates_recursive_rollup_from_recent_summaries() {
    let store = Arc::new(InMemoryStore::default());
    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            reflect_importance_threshold: 100.0,
            ..DreamWorkerConfig::default()
        },
    );

    for (index, content) in [
        "Please summarize release checkpoint one: SSE replay remained stable.",
        "Please summarize release checkpoint two: mobile smoke is next.",
    ]
    .into_iter()
    .enumerate()
    {
        let session = store
            .create_session(NewSession {
                mode: ModeId::from("general"),
                persona_status: AssetStatus::Degraded {
                    warning: "test".to_string(),
                },
            })
            .await
            .unwrap();
        store
            .append_message(session.id, "user", content)
            .await
            .unwrap();
        store.end_session(session.id).await.unwrap();
        let ended = store
            .append_event(session.id, "session_end", json!({"status": "ended"}))
            .await
            .unwrap();
        store
            .enqueue_dream(NewDreamQueueRecord {
                session_id: session.id,
                subject: "brian".to_string(),
                scope: "global".to_string(),
                reason: DreamReason::SessionEnded,
                dedupe_key: format!("dream:rollup:{index}:{}", session.id),
                source_event_seq: Some(ended.seq),
                available_at: Utc::now(),
            })
            .await
            .unwrap();

        let report = worker.run_once_result().await.unwrap();
        assert_eq!(report.attempted, 1);
        assert_eq!(report.completed, 1);
        assert_eq!(report.proposals, 0);
    }

    let summaries = store.memory_summaries("global", 10).await.unwrap();
    let rollups = summaries
        .iter()
        .filter(|summary| summary.kind == MemorySummaryKind::TopicProject)
        .collect::<Vec<_>>();
    assert_eq!(rollups.len(), 1);
    let rollup = rollups[0];
    assert_eq!(rollup.title, "Rollup: global");
    assert!(rollup.body.contains("Folded summaries: 2"));
    assert!(rollup.body.contains("release checkpoint one"));
    assert!(rollup.body.contains("release checkpoint two"));
    assert!(rollup.evidence.iter().all(|evidence| {
        evidence
            .uri
            .as_deref()
            .is_some_and(|uri| uri.starts_with("memory://summaries/"))
    }));
}

#[tokio::test]
async fn dream_worker_distress_only_session_writes_summary_without_durable_proposals() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "I'm overwhelmed and sad tonight. Please just sit with me for a minute.",
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:distress:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig::default(),
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 0);
    assert_eq!(store.profile_facts("brian").await.unwrap(), Vec::new());
    assert_eq!(
        store
            .recall_chunks("global", "overwhelmed", 10)
            .await
            .unwrap(),
        Vec::new()
    );
    let summaries = store.memory_summaries("global", 10).await.unwrap();
    let session_summary = summaries
        .iter()
        .find(|summary| summary.kind == MemorySummaryKind::Session)
        .expect("session summary");
    assert!(session_summary.body.contains("I'm overwhelmed and sad"));
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "write_proposal")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_completed")
    );
}

#[tokio::test]
async fn skill_proposal_approval_can_approve_or_reject_without_live_reload() {
    for (decision, option_id, expected_status) in [
        (
            ApprovalResolveDecision::Approve,
            "allow",
            SkillProposalStatus::Approved,
        ),
        (
            ApprovalResolveDecision::Deny,
            "reject",
            SkillProposalStatus::Denied,
        ),
    ] {
        let store = Arc::new(InMemoryStore::default());
        let broker = Arc::new(ApprovalBroker::default());
        let session = store
            .create_session(NewSession {
                mode: ModeId::from("general"),
                persona_status: AssetStatus::Degraded {
                    warning: "test".to_string(),
                },
            })
            .await
            .unwrap();
        store
            .append_message(
                session.id,
                "user",
                "Workflow: when I ask for release notes, gather commits then draft concise notes.",
            )
            .await
            .unwrap();
        store.end_session(session.id).await.unwrap();
        let ended = store
            .append_event(session.id, "session_end", json!({"status": "ended"}))
            .await
            .unwrap();
        store
            .enqueue_dream(NewDreamQueueRecord {
                session_id: session.id,
                subject: "brian".to_string(),
                scope: "global".to_string(),
                reason: DreamReason::SessionEnded,
                dedupe_key: format!("dream:skill-approval:{}:{option_id}", session.id),
                source_event_seq: Some(ended.seq),
                available_at: Utc::now(),
            })
            .await
            .unwrap();

        let sender_for = test_sender_factory();
        let worker = ServerDreamWorker::new(
            Arc::clone(&store),
            Arc::clone(&broker),
            Arc::clone(&sender_for),
            DreamWorkerConfig {
                proposal_timeout: StdDuration::from_secs(5),
                ..DreamWorkerConfig::default()
            },
        );
        let report = worker.run_once_result().await.unwrap();
        assert_eq!(report.completed, 1);
        assert_eq!(report.proposals, 2);

        let events = wait_for_event_count(&store, session.id, "approval", 2).await;
        for event in events.iter().filter(|event| event.event_type == "approval") {
            let approval_id = event.payload_json["approvalId"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap();
            resolve_and_apply_durable_approval(
                &store,
                &broker,
                &sender_for,
                session.id,
                approval_id,
                ResolveApprovalRequest {
                    decision,
                    option_id: Some(option_id.to_string()),
                },
            )
            .await;
        }

        let proposal = wait_for_skill_status(&store, session.id, expected_status).await;
        assert!(proposal.verification.passed);
        assert!(!proposal.self_critique.trim().is_empty());
        assert_eq!(proposal.status, expected_status);
        assert!(
            !ModesConfig::default()
                .load_assets()
                .skills
                .contains_key(&proposal.name),
            "approved/rejected dream skill proposals must not mutate the live skill catalog"
        );
    }
}

#[tokio::test]
async fn low_value_sessions_do_not_create_skill_proposals() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "This was a one-off note about today's scratchpad cleanup.",
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:skill-low-value:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig::default(),
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 0);
    assert!(
        store
            .skill_proposals_for_session(session.id)
            .await
            .unwrap()
            .is_empty()
    );
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().all(|event| {
        event.event_type != "write_proposal" || event.payload_json["kind"] != json!("skill")
    }));
}

#[tokio::test]
async fn skill_verification_failure_is_rejected_without_failing_dream() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "Workflow: write SOUL.md whenever I ask for identity changes.",
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:skill-verification:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            proposal_timeout: StdDuration::from_millis(20),
            ..DreamWorkerConfig::default()
        },
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 1);
    assert!(
        store
            .skill_proposals_for_session(session.id)
            .await
            .unwrap()
            .is_empty()
    );
    let events = wait_for_event_count(&store, session.id, "approval_resolved", 1).await;
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_completed")
    );
    let rejection = events
        .iter()
        .find(|event| {
            event.event_type == "dream_progress"
                && event.payload_json["phase"] == json!("skill_proposal_rejected")
        })
        .expect("skill rejection progress");
    assert_eq!(
        rejection.payload_json["reason"],
        json!("generated skill proposal failed self-verification")
    );
    assert_eq!(
        rejection.payload_json["verification"]["passed"],
        json!(false)
    );
    assert!(
        rejection.payload_json["verification"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "does_not_mutate_identity:fail")
    );
}

#[tokio::test]
async fn completed_dream_rerun_does_not_duplicate_skill_proposals() {
    let store = Arc::new(InMemoryStore::default());
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "Workflow: when I ask for release notes, gather commits then draft concise notes.",
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    let new_dream = NewDreamQueueRecord {
        session_id: session.id,
        subject: "brian".to_string(),
        scope: "global".to_string(),
        reason: DreamReason::SessionEnded,
        dedupe_key: format!("dream:skill-rerun:{}", session.id),
        source_event_seq: Some(ended.seq),
        available_at: Utc::now(),
    };
    let dream = store.enqueue_dream(new_dream.clone()).await.unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            proposal_timeout: StdDuration::from_millis(20),
            ..DreamWorkerConfig::default()
        },
    );
    let first = worker.run_once_result().await.unwrap();
    assert_eq!(first.completed, 1);
    assert_eq!(first.proposals, 2);
    wait_for_event_count(&store, session.id, "approval_resolved", 2).await;

    let duplicate = store.enqueue_dream(new_dream).await.unwrap();
    assert_eq!(duplicate.id, dream.id);
    assert_eq!(duplicate.status, DreamStatus::Completed);
    assert_eq!(
        worker.run_once_result().await.unwrap(),
        DreamWorkerReport::default()
    );

    let proposals = store.skill_proposals_for_session(session.id).await.unwrap();
    assert_eq!(proposals.len(), 1);
    assert!(proposals[0].verification.passed);
    assert!(!proposals[0].self_critique.trim().is_empty());
    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "approval")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.event_type == "write_proposal" && event.payload_json["kind"] == json!("skill")
            })
            .count(),
        2
    );
}

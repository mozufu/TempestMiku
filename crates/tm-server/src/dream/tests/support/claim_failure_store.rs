use super::*;

pub(in crate::dream::tests) struct ClaimFailureStore;

#[async_trait]
impl Store for ClaimFailureStore {
    async fn create_session(&self, _new: NewSession) -> Result<SessionRecord> {
        panic!("unused store method create_session")
    }

    async fn configure_owner_subject(&self, _owner_subject: &str) -> Result<usize> {
        panic!("unused store method configure_owner_subject")
    }

    async fn set_session_memory_context(
        &self,
        _session_id: Uuid,
        _project_id: Option<&str>,
        _memory_policy: crate::MemoryPolicy,
    ) -> Result<SessionRecord> {
        panic!("unused store method set_session_memory_context")
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

    async fn approval_request_for_skill_proposal(
        &self,
        _session_id: Uuid,
        _proposal_id: Uuid,
    ) -> Result<Option<ApprovalRequestRecord>> {
        panic!("unused store method approval_request_for_skill_proposal")
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

    async fn upsert_evolution_episode(
        &self,
        _new: tm_memory::NewEvolutionEpisodeRecord,
    ) -> Result<(tm_memory::EvolutionEpisodeRecord, bool)> {
        panic!("unused store method upsert_evolution_episode")
    }

    async fn evolution_episode_for_turn(
        &self,
        _turn_id: Uuid,
    ) -> Result<Option<tm_memory::EvolutionEpisodeRecord>> {
        panic!("unused store method evolution_episode_for_turn")
    }

    async fn evolution_episodes(
        &self,
        _owner_subject: &str,
        _memory_scope: &str,
        _limit: usize,
    ) -> Result<Vec<tm_memory::EvolutionEpisodeRecord>> {
        panic!("unused store method evolution_episodes")
    }

    async fn evolution_episode(&self, _id: Uuid) -> Result<tm_memory::EvolutionEpisodeRecord> {
        panic!("unused store method evolution_episode")
    }

    async fn replace_experience_traces(
        &self,
        _episode_id: Uuid,
        _traces: Vec<tm_memory::NewExperienceTraceRecord>,
    ) -> Result<Vec<tm_memory::ExperienceTraceRecord>> {
        panic!("unused store method replace_experience_traces")
    }

    async fn experience_traces(
        &self,
        _episode_id: Uuid,
    ) -> Result<Vec<tm_memory::ExperienceTraceRecord>> {
        panic!("unused store method experience_traces")
    }

    async fn set_episode_valuation(
        &self,
        _episode_id: Uuid,
        _terminal_reward: f32,
        _reward_source: tm_memory::RewardSource,
        _feedback_outcome: Option<tm_memory::FeedbackOutcome>,
        _trace_values: &[(Uuid, f32)],
        _skill_outcomes: &[(String, String, bool)],
        _status: tm_memory::EpisodeStatus,
    ) -> Result<tm_memory::EvolutionEpisodeRecord> {
        panic!("unused store method set_episode_valuation")
    }

    async fn upsert_evolution_policy(
        &self,
        _policy: tm_memory::EvolutionPolicyRecord,
    ) -> Result<tm_memory::EvolutionPolicyRecord> {
        panic!("unused store method upsert_evolution_policy")
    }

    async fn evolution_policy(&self, _id: Uuid) -> Result<tm_memory::EvolutionPolicyRecord> {
        panic!("unused store method evolution_policy")
    }

    async fn evolution_policies(
        &self,
        _owner_subject: &str,
        _memory_scope: &str,
        _status: Option<tm_memory::PolicyStatus>,
        _limit: usize,
    ) -> Result<Vec<tm_memory::EvolutionPolicyRecord>> {
        panic!("unused store method evolution_policies")
    }

    async fn link_policy_traces(
        &self,
        _policy_id: Uuid,
        _links: &[(Uuid, Uuid, f32, bool)],
    ) -> Result<()> {
        panic!("unused store method link_policy_traces")
    }

    async fn policy_trace_values(&self, _policy_id: Uuid) -> Result<Vec<(Uuid, Uuid, f32, bool)>> {
        panic!("unused store method policy_trace_values")
    }

    async fn upsert_environment_cognition(
        &self,
        _cognition: tm_memory::EnvironmentCognitionRecord,
    ) -> Result<tm_memory::EnvironmentCognitionRecord> {
        panic!("unused store method upsert_environment_cognition")
    }

    async fn environment_cognition(
        &self,
        _owner_subject: &str,
        _memory_scope: &str,
    ) -> Result<Option<tm_memory::EnvironmentCognitionRecord>> {
        panic!("unused store method environment_cognition")
    }

    async fn record_turn_feedback(
        &self,
        _session_id: Uuid,
        _turn_id: Uuid,
        _outcome: tm_memory::FeedbackOutcome,
        _comment: Option<&str>,
    ) -> Result<bool> {
        panic!("unused store method record_turn_feedback")
    }

    async fn turn_feedback(
        &self,
        _turn_id: Uuid,
    ) -> Result<Option<(tm_memory::FeedbackOutcome, Option<String>)>> {
        panic!("unused store method turn_feedback")
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

    async fn record_skill_exposures_for_turn(
        &self,
        _session_id: Uuid,
        _turn_id: Uuid,
        _skills: &[(String, String)],
    ) -> Result<(SessionEvent, bool)> {
        panic!("unused store method record_skill_exposures_for_turn")
    }

    async fn record_skill_outcome(&self, _name: &str, _digest: &str, _pass: bool) -> Result<()> {
        panic!("unused store method record_skill_outcome")
    }

    async fn skill_runtime_stats(
        &self,
        _names: &[String],
    ) -> Result<Vec<(String, String, u64, u64, u64)>> {
        panic!("unused store method skill_runtime_stats")
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

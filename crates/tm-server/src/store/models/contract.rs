use super::*;
#[async_trait]
pub trait Store: Send + Sync + 'static {
    async fn create_session(&self, new: NewSession) -> Result<SessionRecord>;
    async fn configure_owner_subject(&self, owner_subject: &str) -> Result<usize>;
    /// The server-owned authority subject. Used by non-session-scoped owner actions such as project
    /// archive (§30.4). Defaults to `owner` for stores without a durable authority row.
    async fn owner_subject(&self) -> Result<String> {
        Ok("owner".to_string())
    }
    /// Sets the session's active project and memory read/write policy in one durable update.
    /// `project_id: None` requires `memory_policy: MemoryPolicy::Global` — callers validate this
    /// before calling (see `resources::util::resolve_memory_context`).
    async fn set_session_memory_context(
        &self,
        session_id: Uuid,
        project_id: Option<&str>,
        memory_policy: MemoryPolicy,
    ) -> Result<SessionRecord>;
    async fn end_session(&self, session_id: Uuid) -> Result<SessionRecord>;
    async fn end_session_and_enqueue_dream(
        &self,
        session_id: Uuid,
        subject: String,
        scope: String,
    ) -> Result<EndSessionDreamResult>;
    async fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummaryRecord>>;
    async fn get_session(&self, session_id: Uuid) -> Result<SessionRecord>;
    async fn session_messages(&self, session_id: Uuid) -> Result<Vec<MessageRecord>>;
    async fn set_mode_state(
        &self,
        session_id: Uuid,
        mode_state: ModeState,
    ) -> Result<SessionRecord>;
    async fn append_message(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
    ) -> Result<MessageRecord>;
    async fn append_message_for_turn(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
        turn_id: Option<Uuid>,
    ) -> Result<MessageRecord> {
        if turn_id.is_some() {
            return Err(ServerError::InvalidRequest(
                "store does not support turn-linked messages".to_string(),
            ));
        }
        self.append_message(session_id, role, content).await
    }
    async fn append_event(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
    ) -> Result<SessionEvent>;
    async fn append_event_for_turn(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
        turn_id: Option<Uuid>,
    ) -> Result<SessionEvent> {
        if turn_id.is_some() {
            return Err(ServerError::InvalidRequest(
                "store does not support turn-linked events".to_string(),
            ));
        }
        self.append_event(session_id, event_type, payload_json)
            .await
    }
    async fn append_event_for_turn_once(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
        turn_id: Uuid,
    ) -> Result<(SessionEvent, bool)> {
        if let Some(event) = self.event_for_turn(session_id, turn_id, event_type).await? {
            return Ok((event, false));
        }
        self.append_event_for_turn(session_id, event_type, payload_json, Some(turn_id))
            .await
            .map(|event| (event, true))
    }
    async fn events_after(
        &self,
        session_id: Uuid,
        last_event_id: Option<i64>,
    ) -> Result<Vec<SessionEvent>>;
    async fn event_for_turn(
        &self,
        session_id: Uuid,
        turn_id: Uuid,
        event_type: &str,
    ) -> Result<Option<SessionEvent>> {
        Ok(self
            .events_after(session_id, None)
            .await?
            .into_iter()
            .find(|event| {
                event.turn_id == Some(turn_id) && event.event_type.as_str() == event_type
            }))
    }
    async fn events_by_type(
        &self,
        session_id: Uuid,
        event_type: &str,
        limit: usize,
    ) -> Result<Vec<SessionEvent>> {
        let mut events = self
            .events_after(session_id, None)
            .await?
            .into_iter()
            .filter(|event| event.event_type.as_str() == event_type)
            .collect::<Vec<_>>();
        if events.len() > limit {
            events.drain(..events.len() - limit);
        }
        Ok(events)
    }
    async fn memory_recall_events(
        &self,
        session_id: Uuid,
        owner_subject: &str,
        memory_scope: &str,
        limit: usize,
    ) -> Result<Vec<SessionEvent>> {
        let mut events = self
            .events_after(session_id, None)
            .await?
            .into_iter()
            .filter(|event| {
                event.event_type == "memory_recall"
                    && event
                        .payload_json
                        .pointer("/context/subject")
                        .and_then(Value::as_str)
                        == Some(owner_subject)
                    && event
                        .payload_json
                        .pointer("/context/scope")
                        .and_then(Value::as_str)
                        == Some(memory_scope)
            })
            .collect::<Vec<_>>();
        if events.len() > limit {
            events.drain(..events.len() - limit);
        }
        Ok(events)
    }
    async fn enqueue_turn(
        &self,
        session_id: Uuid,
        client_message_id: &str,
        content: &str,
    ) -> Result<SessionTurnRecord>;
    async fn turn(&self, turn_id: Uuid) -> Result<SessionTurnRecord>;
    async fn claim_next_turn(
        &self,
        worker_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<SessionTurnRecord>>;
    async fn heartbeat_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<SessionTurnRecord>;
    async fn fail_stale_running_turns(
        &self,
        stale_before: DateTime<Utc>,
        failed_at: DateTime<Utc>,
        error: &str,
    ) -> Result<usize>;
    async fn complete_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        assistant_content: &str,
        completed_at: DateTime<Utc>,
    ) -> Result<SessionTurnRecord>;
    async fn fail_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        error: &str,
        failed_at: DateTime<Utc>,
    ) -> Result<SessionTurnRecord>;
    async fn begin_mcp_mutation_effect(
        &self,
        _intent: tm_mcp::McpMutationIntent,
    ) -> Result<tm_mcp::McpMutationEffectClaim> {
        Err(ServerError::Store(
            "store does not support durable MCP mutation effects".to_string(),
        ))
    }
    #[allow(clippy::too_many_arguments)]
    async fn finish_mcp_mutation_effect(
        &self,
        _effect_id: &str,
        _status: tm_mcp::McpMutationEffectStatus,
        _result_digest: Option<&str>,
        _result_bytes: Option<usize>,
        _error_code: Option<&str>,
        _error_digest: Option<&str>,
    ) -> Result<tm_mcp::McpMutationEffectRecord> {
        Err(ServerError::Store(
            "store does not support durable MCP mutation effects".to_string(),
        ))
    }
    async fn begin_egress_mutation_effect(
        &self,
        _intent: tm_egress::EgressMutationIntent,
    ) -> Result<tm_egress::EgressMutationClaim> {
        Err(ServerError::Store(
            "store does not support durable egress mutation effects".to_string(),
        ))
    }
    #[allow(clippy::too_many_arguments)]
    async fn finish_egress_mutation_effect(
        &self,
        _effect_id: &str,
        _status: tm_egress::EgressMutationStatus,
        _result_digest: Option<&str>,
        _result_bytes: Option<usize>,
        _error_code: Option<&str>,
        _error_digest: Option<&str>,
    ) -> Result<tm_egress::EgressMutationRecord> {
        Err(ServerError::Store(
            "store does not support durable egress mutation effects".to_string(),
        ))
    }
    async fn reserve_egress_budget(
        &self,
        _request: tm_egress::EgressBudgetRequest,
    ) -> Result<tm_egress::EgressBudgetReservation> {
        Err(ServerError::Store(
            "store does not support durable egress budgets".to_string(),
        ))
    }
    async fn settle_egress_budget(
        &self,
        _reservation: tm_egress::EgressBudgetReservation,
        _response_bytes: u64,
        _elapsed_ms: u64,
    ) -> Result<()> {
        Err(ServerError::Store(
            "store does not support durable egress budgets".to_string(),
        ))
    }
    async fn clear_egress_session(&self, _session_id: &str) -> Result<()> {
        Err(ServerError::Store(
            "store does not support durable egress session cleanup".to_string(),
        ))
    }
    async fn create_approval_request(
        &self,
        request: NewApprovalRequest,
    ) -> Result<ApprovalRequestRecord>;
    async fn approval_request(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
    ) -> Result<ApprovalRequestRecord>;
    async fn approval_request_for_skill_proposal(
        &self,
        session_id: Uuid,
        proposal_id: Uuid,
    ) -> Result<Option<ApprovalRequestRecord>>;
    async fn create_skill_approval_bundle(
        &self,
        _bundle: NewSkillApprovalBundle,
    ) -> Result<SkillApprovalBundleResult> {
        Err(ServerError::Store(
            "atomic skill approval bundle persistence is not implemented".to_string(),
        ))
    }
    async fn heartbeat_approval_request(
        &self,
        approval_id: Uuid,
        requester_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<ApprovalRequestRecord>;
    async fn resolve_approval_request(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        resolution: NewApprovalResolution,
    ) -> Result<ApprovalRequestRecord>;
    async fn resolve_approval_request_with_event(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        resolution: NewApprovalResolution,
    ) -> Result<(ApprovalRequestRecord, SessionEvent)>;
    async fn link_approval_event(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        event_type: &str,
        event_seq: i64,
    ) -> Result<ApprovalRequestRecord>;
    async fn cancel_stale_non_resumable_approvals(
        &self,
        stale_before: DateTime<Utc>,
        cancelled_at: DateTime<Utc>,
    ) -> Result<Vec<SessionEvent>>;
    async fn expire_pending_approvals(&self, now: DateTime<Utc>) -> Result<Vec<SessionEvent>>;
    async fn claim_approval_effect(
        &self,
        approval_id: Uuid,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<Option<ApprovalEffectLease>>;
    async fn claim_next_approval_effect(
        &self,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<Option<ApprovalEffectLease>>;
    async fn heartbeat_approval_effect(
        &self,
        _lease: &ApprovalEffectLease,
        _now: DateTime<Utc>,
    ) -> Result<ApprovalEffectRecord> {
        Err(ServerError::Store(
            "approval effect lease heartbeat is not implemented".to_string(),
        ))
    }
    async fn complete_approval_effect(
        &self,
        lease: &ApprovalEffectLease,
        applied_at: DateTime<Utc>,
    ) -> Result<ApprovalEffectRecord>;
    async fn complete_approval_effect_with_event(
        &self,
        lease: &ApprovalEffectLease,
        proposal_payload_json: Value,
        turn_id: Option<Uuid>,
        applied_at: DateTime<Utc>,
    ) -> Result<(ApprovalEffectRecord, SessionEvent)>;
    async fn fail_approval_effect(
        &self,
        lease: &ApprovalEffectLease,
        error: &str,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<ApprovalEffectRecord>;
    async fn append_evolution_audit(
        &self,
        _entry: EvolutionAuditEntry,
    ) -> Result<EvolutionAuditRecord> {
        Err(ServerError::Store(
            "evolution audit persistence is not implemented".to_string(),
        ))
    }
    async fn evolution_audits(&self, _session_id: Uuid) -> Result<Vec<EvolutionAuditRecord>> {
        Err(ServerError::Store(
            "evolution audit query is not implemented".to_string(),
        ))
    }
    async fn evolution_memory_proposal(
        &self,
        _proposal_id: Uuid,
    ) -> Result<crate::MemoryWriteProposal> {
        Err(ServerError::Store(
            "evolution proposal query is not implemented".to_string(),
        ))
    }
    async fn create_evolution_review_proposal(
        &self,
        _proposal: NewEvolutionReviewProposal,
    ) -> Result<EvolutionReviewProposalRecord> {
        Err(ServerError::Store(
            "evolution review proposal persistence is not implemented".to_string(),
        ))
    }
    async fn create_auto_evolution_review_proposal(
        &self,
        _proposal: NewEvolutionReviewProposal,
        _cooldown_since: DateTime<Utc>,
    ) -> Result<AutoEvolutionReviewProposalResult> {
        Err(ServerError::Store(
            "atomic auto evolution review proposal persistence is not implemented".to_string(),
        ))
    }
    async fn create_auto_evolution_review_bundle(
        &self,
        _bundle: NewAutoEvolutionReviewBundle,
    ) -> Result<AutoEvolutionReviewBundleResult> {
        Err(ServerError::Store(
            "atomic auto evolution review approval bundle persistence is not implemented"
                .to_string(),
        ))
    }
    async fn evolution_review_proposal(&self, _id: Uuid) -> Result<EvolutionReviewProposalRecord> {
        Err(ServerError::Store(
            "evolution review proposal query is not implemented".to_string(),
        ))
    }
    async fn evolution_review_proposals_for_session(
        &self,
        _session_id: Uuid,
    ) -> Result<Vec<EvolutionReviewProposalRecord>> {
        Err(ServerError::Store(
            "evolution review proposal list is not implemented".to_string(),
        ))
    }
    async fn update_evolution_review_proposal_status(
        &self,
        _id: Uuid,
        _status: ReviewProposalStatus,
    ) -> Result<EvolutionReviewProposalRecord> {
        Err(ServerError::Store(
            "evolution review proposal update is not implemented".to_string(),
        ))
    }
    async fn add_profile_fact(&self, fact: ProfileFactRecord) -> Result<()>;
    async fn add_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<()>;
    async fn upsert_profile_fact(&self, fact: ProfileFactRecord) -> Result<ProfileFactRecord>;
    async fn upsert_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<RecallChunkRecord>;
    async fn apply_approved_memory_proposal(
        &self,
        _proposal: &crate::MemoryWriteProposal,
    ) -> Result<crate::MemoryRecordRef> {
        Err(ServerError::Store(
            "atomic approved memory proposal application is not implemented".to_string(),
        ))
    }
    async fn profile_facts(&self, subject: &str) -> Result<Vec<ProfileFactRecord>>;
    async fn profile_fact(&self, subject: &str, id: Uuid) -> Result<ProfileFactRecord>;
    async fn recall_chunks(
        &self,
        scope: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecallChunkRecord>>;
    async fn recall_chunk(&self, scope: &str, id: Uuid) -> Result<RecallChunkRecord>;
    async fn upsert_memory_record(
        &self,
        _record: StoredMemoryRecord,
    ) -> Result<StoredMemoryRecord> {
        Err(ServerError::Store(
            "durable P8 memory record persistence is not implemented".to_string(),
        ))
    }
    async fn memory_record(
        &self,
        _owner_subject: &str,
        _memory_scope: &str,
        _kind: MemoryRecordKind,
        _id: Uuid,
    ) -> Result<StoredMemoryRecord> {
        Err(ServerError::Store(
            "durable P8 memory record lookup is not implemented".to_string(),
        ))
    }
    async fn active_memory_records(
        &self,
        _owner_subject: &str,
        _memory_scope: &str,
        _limit: usize,
    ) -> Result<Vec<StoredMemoryRecord>> {
        Err(ServerError::Store(
            "durable P8 memory recall is not implemented".to_string(),
        ))
    }
    async fn active_memory_embedding_generation(
        &self,
        _owner_subject: &str,
        _memory_scope: &str,
    ) -> Result<Option<MemoryEmbeddingGeneration>> {
        Ok(None)
    }
    async fn memory_hybrid_candidates(
        &self,
        _request: &HybridRecallRequest,
        _query: &str,
        _dense_query: Option<&DenseRecallQuery>,
    ) -> Result<HybridRecallResult> {
        Err(ServerError::Store(
            "hybrid memory recall is not implemented".to_string(),
        ))
    }
    async fn enqueue_memory_embedding_job(
        &self,
        _job: NewMemoryEmbeddingJob,
    ) -> Result<MemoryEmbeddingJobRecord> {
        Err(ServerError::Store(
            "durable P8 embedding jobs are not implemented".to_string(),
        ))
    }
    async fn memory_embedding_jobs(
        &self,
        _owner_subject: &str,
        _memory_scope: &str,
    ) -> Result<Vec<MemoryEmbeddingJobRecord>> {
        Err(ServerError::Store(
            "durable P8 embedding job lookup is not implemented".to_string(),
        ))
    }
    async fn revoke_memory_scope(
        &self,
        _owner_subject: &str,
        _memory_scope: &str,
        _reason: &str,
    ) -> Result<MemoryScopeTombstone> {
        Err(ServerError::Store(
            "durable P8 memory scope revocation is not implemented".to_string(),
        ))
    }
    async fn memory_scope_tombstone(
        &self,
        _owner_subject: &str,
        _memory_scope: &str,
    ) -> Result<Option<MemoryScopeTombstone>> {
        Err(ServerError::Store(
            "durable P8 memory scope tombstone lookup is not implemented".to_string(),
        ))
    }
    async fn ensure_memory_scope_active(
        &self,
        owner_subject: &str,
        memory_scope: &str,
    ) -> Result<()> {
        if memory_scope == "global" {
            return Ok(());
        }
        if self
            .memory_scope_tombstone(owner_subject, memory_scope)
            .await?
            .is_some()
        {
            return Err(ServerError::NotFound(format!(
                "memory scope {owner_subject}/{memory_scope}"
            )));
        }
        Ok(())
    }
    async fn upsert_project_item(&self, item: NewProjectItem) -> Result<ProjectItemRecord>;
    async fn project_items(
        &self,
        project_id: &str,
        kind: Option<ProjectItemKind>,
    ) -> Result<Vec<ProjectItemRecord>>;
    /// Create or return the project entity for `id` (§30). `id` is the canonical `project:<id>`
    /// slug; `title` is the display name. Idempotent on `id`.
    async fn ensure_project(
        &self,
        id: &str,
        title: &str,
        default_memory_policy: MemoryPolicy,
    ) -> Result<ProjectRecord> {
        let _ = (id, title, default_memory_policy);
        Err(ServerError::Store(
            "project entities are not implemented by this store".to_string(),
        ))
    }
    /// Look up a single project entity by id.
    async fn project(&self, id: &str) -> Result<Option<ProjectRecord>> {
        let _ = id;
        Err(ServerError::Store(
            "project entities are not implemented by this store".to_string(),
        ))
    }
    /// List project entities. When `include_archived` is false, only `active` projects are returned.
    async fn projects(&self, include_archived: bool) -> Result<Vec<ProjectRecord>> {
        let _ = include_archived;
        Err(ServerError::Store(
            "project entities are not implemented by this store".to_string(),
        ))
    }
    /// Archive a project entity and tombstone its memory scope (§30.4). The archive and the durable
    /// scope revocation commit together so exact recall/replay fails closed after archive.
    async fn archive_project(
        &self,
        owner_subject: &str,
        id: &str,
        reason: &str,
    ) -> Result<ProjectRecord> {
        let _ = (owner_subject, id, reason);
        Err(ServerError::Store(
            "project entities are not implemented by this store".to_string(),
        ))
    }
    /// Create or return the memory pool entity for `id` (§30.7). `title` is the display name.
    /// Idempotent on `id`.
    async fn ensure_memory_pool(&self, id: &str, title: &str) -> Result<MemoryPoolRecord> {
        let _ = (id, title);
        Err(ServerError::Store(
            "memory pool entities are not implemented by this store".to_string(),
        ))
    }
    /// Look up a single memory pool by id.
    async fn memory_pool(&self, id: &str) -> Result<Option<MemoryPoolRecord>> {
        let _ = id;
        Err(ServerError::Store(
            "memory pool entities are not implemented by this store".to_string(),
        ))
    }
    /// List memory pools. When `include_archived` is false, only `active` pools are returned.
    async fn memory_pools(&self, include_archived: bool) -> Result<Vec<MemoryPoolRecord>> {
        let _ = include_archived;
        Err(ServerError::Store(
            "memory pool entities are not implemented by this store".to_string(),
        ))
    }
    /// Archive a memory pool (§30.7). A pure status flip: member projects keep their `pool_id`
    /// unchanged, and fan-out membership is recomputed at read time from the pool's status — there
    /// is nothing to tombstone or clean up here (contrast with §30.4 project-archive revocation).
    async fn archive_memory_pool(&self, id: &str) -> Result<MemoryPoolRecord> {
        let _ = id;
        Err(ServerError::Store(
            "memory pool entities are not implemented by this store".to_string(),
        ))
    }
    /// Raw setter for a project's pool membership (§30.7); no validation beyond existence.
    /// `join_memory_pool` / `leave_memory_pool` are the validated entry points — they enforce the
    /// active-project / active-pool / at-most-one-pool rules once, here, for both stores.
    async fn set_project_pool(
        &self,
        project_id: &str,
        pool_id: Option<&str>,
    ) -> Result<ProjectRecord> {
        let _ = (project_id, pool_id);
        Err(ServerError::Store(
            "memory pool entities are not implemented by this store".to_string(),
        ))
    }
    /// Join `project_id` to `pool_id` (§30.7). Both must be active, and the project must not
    /// already belong to a pool — leave first, no silent moves between pools.
    async fn join_memory_pool(&self, project_id: &str, pool_id: &str) -> Result<ProjectRecord> {
        validate_persistence_identifier("project id", project_id)?;
        validate_persistence_identifier("memory pool id", pool_id)?;
        let project = self
            .project(project_id)
            .await?
            .ok_or_else(|| ServerError::NotFound(format!("project {project_id}")))?;
        if project.status != ProjectStatus::Active {
            return Err(ServerError::Conflict(format!(
                "project {project_id} is archived"
            )));
        }
        if project.pool_id.is_some() {
            return Err(ServerError::Conflict(format!(
                "project {project_id} already belongs to a pool; leave it first"
            )));
        }
        let pool = self
            .memory_pool(pool_id)
            .await?
            .ok_or_else(|| ServerError::NotFound(format!("memory pool {pool_id}")))?;
        if pool.status != MemoryPoolStatus::Active {
            return Err(ServerError::Conflict(format!(
                "memory pool {pool_id} is archived"
            )));
        }
        self.set_project_pool(project_id, Some(pool_id)).await
    }
    /// Remove `project_id` from whichever pool it belongs to (§30.7). Idempotent: a project with no
    /// pool is returned unchanged.
    async fn leave_memory_pool(&self, project_id: &str) -> Result<ProjectRecord> {
        validate_persistence_identifier("project id", project_id)?;
        let project = self
            .project(project_id)
            .await?
            .ok_or_else(|| ServerError::NotFound(format!("project {project_id}")))?;
        if project.pool_id.is_none() {
            return Ok(project);
        }
        self.set_project_pool(project_id, None).await
    }
    /// Sibling memory scopes for `project_id`'s recall fan-out (§30.7, §22.3): every other
    /// **active** project in the same pool, gated on the pool itself being **active**. Empty when
    /// the project has no pool, its pool is archived, or there are no active siblings. Read-only —
    /// write authority, exact reads, and archive tombstones (§30.4) are entirely unaffected.
    async fn pool_sibling_scopes(&self, project_id: &str) -> Result<Vec<String>> {
        let Some(project) = self.project(project_id).await? else {
            return Ok(Vec::new());
        };
        let Some(pool_id) = project.pool_id.as_deref() else {
            return Ok(Vec::new());
        };
        match self.memory_pool(pool_id).await? {
            Some(pool) if pool.status == MemoryPoolStatus::Active => {}
            _ => return Ok(Vec::new()),
        }
        let siblings = self
            .projects(false)
            .await?
            .into_iter()
            .filter(|sibling| {
                sibling.id != project_id && sibling.pool_id.as_deref() == Some(pool_id)
            })
            .map(|sibling| format!("project:{}", sibling.id))
            .collect();
        Ok(siblings)
    }
    async fn enqueue_dream(&self, new: NewDreamQueueRecord) -> Result<DreamQueueRecord>;
    async fn dream_queue_for_session(&self, session_id: Uuid) -> Result<Vec<DreamQueueRecord>>;
    async fn dream_queue(&self, scope: &str, limit: usize) -> Result<Vec<DreamQueueRecord>>;
    async fn dream(&self, dream_id: Uuid) -> Result<DreamQueueRecord>;
    async fn claim_ready_dream(
        &self,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        owner_id: Uuid,
    ) -> Result<Option<DreamLease>>;
    async fn claim_ready_dream_bounded(
        &self,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        owner_id: Uuid,
        max_attempts: i32,
    ) -> Result<Option<DreamLease>> {
        let _ = max_attempts;
        self.claim_ready_dream(now, lease_timeout, owner_id).await
    }
    async fn heartbeat_dream(&self, lease: &DreamLease, now: DateTime<Utc>) -> Result<DreamLease>;
    async fn complete_dream(
        &self,
        lease: &DreamLease,
        now: DateTime<Utc>,
    ) -> Result<DreamQueueRecord>;
    async fn fail_dream(
        &self,
        lease: &DreamLease,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<DreamQueueRecord>;
    async fn upsert_memory_summary(
        &self,
        summary: NewMemorySummaryRecord,
    ) -> Result<MemorySummaryRecord>;
    async fn memory_summary(&self, id: Uuid) -> Result<MemorySummaryRecord>;
    async fn memory_summaries(&self, scope: &str, limit: usize)
    -> Result<Vec<MemorySummaryRecord>>;
    async fn upsert_evolution_episode(
        &self,
        new: tm_memory::NewEvolutionEpisodeRecord,
    ) -> Result<(tm_memory::EvolutionEpisodeRecord, bool)>;
    async fn evolution_episode_for_turn(
        &self,
        turn_id: Uuid,
    ) -> Result<Option<tm_memory::EvolutionEpisodeRecord>>;
    async fn evolution_episodes(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        limit: usize,
    ) -> Result<Vec<tm_memory::EvolutionEpisodeRecord>>;
    async fn evolution_episode(&self, id: Uuid) -> Result<tm_memory::EvolutionEpisodeRecord>;
    async fn replace_experience_traces(
        &self,
        episode_id: Uuid,
        traces: Vec<tm_memory::NewExperienceTraceRecord>,
    ) -> Result<Vec<tm_memory::ExperienceTraceRecord>>;
    async fn experience_traces(
        &self,
        episode_id: Uuid,
    ) -> Result<Vec<tm_memory::ExperienceTraceRecord>>;
    #[allow(clippy::too_many_arguments)]
    async fn set_episode_valuation(
        &self,
        episode_id: Uuid,
        terminal_reward: f32,
        reward_source: tm_memory::RewardSource,
        feedback_outcome: Option<tm_memory::FeedbackOutcome>,
        trace_values: &[(Uuid, f32)],
        skill_outcomes: &[(String, String, bool)],
        status: tm_memory::EpisodeStatus,
    ) -> Result<tm_memory::EvolutionEpisodeRecord>;
    async fn upsert_evolution_policy(
        &self,
        policy: tm_memory::EvolutionPolicyRecord,
    ) -> Result<tm_memory::EvolutionPolicyRecord>;
    async fn evolution_policy(&self, id: Uuid) -> Result<tm_memory::EvolutionPolicyRecord>;
    async fn evolution_policies(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        status: Option<tm_memory::PolicyStatus>,
        limit: usize,
    ) -> Result<Vec<tm_memory::EvolutionPolicyRecord>>;
    async fn link_policy_traces(
        &self,
        policy_id: Uuid,
        links: &[(Uuid, Uuid, f32, bool)],
    ) -> Result<()>;
    async fn policy_trace_values(&self, policy_id: Uuid) -> Result<Vec<(Uuid, Uuid, f32, bool)>>;
    async fn upsert_environment_cognition(
        &self,
        cognition: tm_memory::EnvironmentCognitionRecord,
    ) -> Result<tm_memory::EnvironmentCognitionRecord>;
    async fn environment_cognition(
        &self,
        owner_subject: &str,
        memory_scope: &str,
    ) -> Result<Option<tm_memory::EnvironmentCognitionRecord>>;
    async fn record_turn_feedback(
        &self,
        session_id: Uuid,
        turn_id: Uuid,
        outcome: tm_memory::FeedbackOutcome,
        comment: Option<&str>,
    ) -> Result<bool>;
    async fn turn_feedback(
        &self,
        turn_id: Uuid,
    ) -> Result<Option<(tm_memory::FeedbackOutcome, Option<String>)>>;
    async fn upsert_skill_proposal(
        &self,
        proposal: NewSkillProposalRecord,
    ) -> Result<SkillProposalRecord>;
    async fn update_skill_proposal_status(
        &self,
        id: Uuid,
        status: SkillProposalStatus,
    ) -> Result<SkillProposalRecord>;
    async fn skill_proposal(&self, id: Uuid) -> Result<SkillProposalRecord>;
    async fn skill_proposals_for_session(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<SkillProposalRecord>>;
    async fn record_skill_exposures_for_turn(
        &self,
        session_id: Uuid,
        turn_id: Uuid,
        skills: &[(String, String)],
    ) -> Result<(SessionEvent, bool)>;
    async fn record_skill_outcome(&self, name: &str, digest: &str, pass: bool) -> Result<()>;
    async fn skill_runtime_stats(
        &self,
        names: &[String],
    ) -> Result<Vec<(String, String, u64, u64, u64)>>;
    async fn upsert_cron_job(&self, job: NewCronJobRecord) -> Result<CronJobRecord>;
    async fn cron_job(&self, id: &str) -> Result<CronJobRecord>;
    async fn cron_jobs(&self) -> Result<Vec<CronJobRecord>>;
    /// Atomically materialize one scheduled fire and advance the job cursor.
    ///
    /// Returns `None` when another scheduler already advanced the expected cursor.
    async fn materialize_cron_run(
        &self,
        run: NewCronRunRecord,
        expected_next_run_at: DateTime<Utc>,
        next_run_at: DateTime<Utc>,
    ) -> Result<Option<CronRunRecord>>;
    /// Claim the oldest ready or stale cron run independently from materialization.
    async fn claim_ready_cron_run(
        &self,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        max_attempts: i32,
    ) -> Result<Option<CronLease>>;
    async fn claim_cron_run(
        &self,
        run: NewCronRunRecord,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<(CronLease, bool)>;
    async fn record_cron_run(&self, run: NewCronRunRecord) -> Result<CronRunRecord>;
    async fn heartbeat_cron_run(&self, lease: &CronLease, now: DateTime<Utc>) -> Result<CronLease>;
    async fn complete_cron_run(
        &self,
        lease: &CronLease,
        status: &str,
        session_id: Option<Uuid>,
        result_json: Value,
    ) -> Result<CronRunRecord>;
    async fn fail_cron_run(
        &self,
        lease: &CronLease,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<CronRunRecord>;
    async fn cron_runs(&self, job_id: &str, limit: usize) -> Result<Vec<CronRunRecord>>;
    async fn runtime_metrics(&self, now: DateTime<Utc>) -> Result<StoreRuntimeMetrics>;
}

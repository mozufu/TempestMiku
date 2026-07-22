use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tm_host::{EvolutionAuditRecord, EvolutionAuditStatus};
use tm_memory::{
    DreamLease, DreamQueueRecord, DreamStatus, EnvironmentCognitionRecord, EpisodeStatus,
    EvolutionEpisodeRecord, EvolutionPolicyRecord, ExperienceTraceRecord, FeedbackOutcome,
    MemoryEmbeddingJobRecord, MemoryEmbeddingJobStatus, MemoryRecordKind, MemoryScopeTombstone,
    MemorySummaryRecord, NewDreamQueueRecord, NewEvolutionEpisodeRecord, NewExperienceTraceRecord,
    NewMemoryEmbeddingJob, NewMemorySummaryRecord, NewSkillProposalRecord, PolicyStatus,
    SkillProposalRecord, SkillProposalStatus, StoredMemoryRecord,
};
use tm_modes::ReviewProposalStatus;
use uuid::Uuid;

use crate::{Result, ServerError};

use super::models::{
    redact_persisted_json, redact_persisted_text, sanitize_approval_request_persistence,
    sanitize_cron_job_persistence, sanitize_cron_run_persistence, sanitize_durable_memory_record,
    sanitize_environment_cognition_persistence, sanitize_evolution_episode_persistence,
    sanitize_evolution_review_proposal_persistence, sanitize_experience_trace_persistence,
    sanitize_memory_summary_persistence, sanitize_new_memory_embedding_job,
    sanitize_project_item_persistence, sanitize_skill_proposal_persistence,
    sanitize_turn_feedback_comment, turn_content_hash, validate_evolution_policy,
    validate_experience_trace_replacement, validate_persistence_identifier,
    validate_profile_fact_persistence, validate_recall_chunk_persistence,
};

use super::{
    ApprovalEffectLease, ApprovalEffectRecord, ApprovalRequestRecord,
    AutoEvolutionReviewBundleResult, AutoEvolutionReviewDisposition,
    AutoEvolutionReviewProposalResult, CronJobRecord, CronLease, CronRunRecord,
    EndSessionDreamResult, EvolutionAuditEntry, EvolutionReviewProposalRecord, MemoryPolicy,
    MessageRecord, ModeState, NewApprovalRequest, NewApprovalResolution,
    NewAutoEvolutionReviewBundle, NewCronJobRecord, NewCronRunRecord, NewEvolutionReviewProposal,
    NewProjectItem, NewSession, NewSkillApprovalBundle, ProfileFactRecord, ProjectItemKind,
    ProjectItemRecord, ProjectRecord, ProjectStatus, RecallChunkRecord, SessionEvent,
    SessionRecord, SessionSummaryRecord, SessionTurnRecord, SkillApprovalBundleResult, Store,
    StoreRuntimeMetrics,
};

mod approval_helpers;
mod memory_helpers;

use approval_helpers::{
    append_evolution_audit_in_memory, approval_effect_is_claimable, claim_approval_effect_at,
    resolve_approval_in_memory, resolve_approval_with_event_in_memory, stale_approval_effect_lease,
};
use memory_helpers::{
    ensure_memory_record_links_are_scoped, ensure_memory_scope_is_readable,
    memory_record_is_retrievable,
};

#[derive(Debug, Default, Clone)]
pub struct InMemoryStore {
    inner: Arc<Mutex<Inner>>,
    egress_state: Arc<tm_egress::VolatileEgressStateStore>,
}

#[derive(Debug, Default, Clone)]
struct Inner {
    configured_owner_subject: Option<String>,
    sessions: BTreeMap<Uuid, SessionRecord>,
    events: BTreeMap<Uuid, Vec<SessionEvent>>,
    messages: BTreeMap<Uuid, Vec<MessageRecord>>,
    turns: Vec<SessionTurnRecord>,
    approval_requests: Vec<ApprovalRequestRecord>,
    approval_effects: Vec<ApprovalEffectRecord>,
    evolution_audits: Vec<EvolutionAuditEntry>,
    evolution_review_proposals: Vec<EvolutionReviewProposalRecord>,
    profile_facts: Vec<ProfileFactRecord>,
    recall_chunks: Vec<RecallChunkRecord>,
    memory_records: Vec<StoredMemoryRecord>,
    memory_embedding_jobs: Vec<MemoryEmbeddingJobRecord>,
    memory_scope_tombstones: Vec<MemoryScopeTombstone>,
    project_items: Vec<ProjectItemRecord>,
    projects: BTreeMap<String, ProjectRecord>,
    dream_queue: Vec<DreamQueueRecord>,
    memory_summaries: Vec<MemorySummaryRecord>,
    evolution_episodes: Vec<EvolutionEpisodeRecord>,
    experience_traces: Vec<ExperienceTraceRecord>,
    turn_feedback: BTreeMap<Uuid, (Uuid, FeedbackOutcome, Option<String>)>,
    evolution_policies: Vec<EvolutionPolicyRecord>,
    policy_trace_links: Vec<(Uuid, Uuid, Uuid, f32, bool)>,
    environment_cognitions: Vec<EnvironmentCognitionRecord>,
    skill_proposals: Vec<SkillProposalRecord>,
    skill_runtime_stats: BTreeMap<(String, String), (u64, u64, u64)>,
    skill_outcomes: BTreeSet<(Uuid, String, String, bool)>,
    cron_jobs: Vec<CronJobRecord>,
    cron_runs: Vec<CronRunRecord>,
    mcp_mutation_effects: BTreeMap<String, tm_mcp::McpMutationEffectRecord>,
    #[cfg(test)]
    fail_auto_evolution_bundle_before_commit_once: bool,
    #[cfg(test)]
    fail_complete_approval_effect_with_event_once: bool,
    #[cfg(test)]
    fail_set_episode_valuation_once: bool,
    #[cfg(test)]
    fail_upsert_environment_cognition_once: bool,
}

#[cfg(test)]
impl InMemoryStore {
    pub(crate) fn fail_next_auto_evolution_bundle_before_commit(&self) {
        self.inner
            .lock()
            .fail_auto_evolution_bundle_before_commit_once = true;
    }

    pub(crate) fn fail_next_complete_approval_effect_with_event(&self) {
        self.inner
            .lock()
            .fail_complete_approval_effect_with_event_once = true;
    }

    pub(crate) fn fail_next_set_episode_valuation(&self) {
        self.inner.lock().fail_set_episode_valuation_once = true;
    }

    pub(crate) fn fail_next_upsert_environment_cognition(&self) {
        self.inner.lock().fail_upsert_environment_cognition_once = true;
    }
}

fn stale_dream_lease(lease: &DreamLease) -> ServerError {
    ServerError::NotFound(format!(
        "active dream lease {} owner {} epoch {}",
        lease.dream.id, lease.owner_id, lease.epoch
    ))
}

fn stale_cron_lease(lease: &CronLease) -> ServerError {
    ServerError::NotFound(format!(
        "active cron lease {} owner {} epoch {}",
        lease.run.id, lease.owner_id, lease.epoch
    ))
}

fn egress_store_error(error: tm_egress::EgressError) -> ServerError {
    match error {
        tm_egress::EgressError::Budget(_) => {
            ServerError::Policy("egress budget exceeded".to_string())
        }
        tm_egress::EgressError::Durability(_) => {
            ServerError::Conflict("egress durable state collision".to_string())
        }
        other => ServerError::Store(format!("egress durable state failed ({})", other.code())),
    }
}

#[async_trait]
impl Store for InMemoryStore {
    async fn create_session(&self, new: NewSession) -> Result<SessionRecord> {
        let now = Utc::now();
        let mut inner = self.inner.lock();
        let session = SessionRecord {
            id: Uuid::new_v4(),
            created_at: now,
            updated_at: now,
            status: "open".to_string(),
            mode: new.mode.clone(),
            mode_state: ModeState::new(new.mode, now),
            persona_status: new.persona_status,
            owner_subject: inner
                .configured_owner_subject
                .clone()
                .unwrap_or_else(|| "brian".to_string()),
            project_id: None,
            memory_policy: MemoryPolicy::default(),
        };
        inner.sessions.insert(session.id, session.clone());
        Ok(session)
    }

    async fn owner_subject(&self) -> Result<String> {
        Ok(self
            .inner
            .lock()
            .configured_owner_subject
            .clone()
            .unwrap_or_else(|| "brian".to_string()))
    }

    async fn configure_owner_subject(&self, owner_subject: &str) -> Result<usize> {
        let owner_subject = owner_subject.trim();
        if owner_subject.is_empty() {
            return Err(ServerError::InvalidRequest(
                "owner subject must not be empty".to_string(),
            ));
        }
        validate_persistence_identifier("owner subject", owner_subject)?;
        let mut inner = self.inner.lock();
        if inner
            .configured_owner_subject
            .as_deref()
            .is_some_and(|configured| configured != owner_subject)
            || (inner.configured_owner_subject.is_some()
                && inner
                    .sessions
                    .values()
                    .any(|session| session.owner_subject != owner_subject))
            || (inner.configured_owner_subject.is_none()
                && inner.sessions.values().any(|session| {
                    !matches!(session.owner_subject.as_str(), "owner" | "brian")
                        && session.owner_subject != owner_subject
                }))
        {
            return Err(ServerError::Conflict(
                "server is already configured for a different owner subject".to_string(),
            ));
        }
        inner.configured_owner_subject = Some(owner_subject.to_string());
        let mut updated = 0;
        let now = Utc::now();
        for session in inner.sessions.values_mut() {
            if session.owner_subject != owner_subject {
                session.owner_subject = owner_subject.to_string();
                session.updated_at = now;
                updated += 1;
            }
        }
        Ok(updated)
    }

    async fn set_session_memory_context(
        &self,
        session_id: Uuid,
        project_id: Option<&str>,
        memory_policy: MemoryPolicy,
    ) -> Result<SessionRecord> {
        if let Some(project_id) = project_id {
            validate_persistence_identifier("project id", project_id)?;
        }
        let mut inner = self.inner.lock();
        let session = inner
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        if session.status == "ended" {
            return Err(ServerError::Conflict(format!(
                "session {session_id} has ended"
            )));
        }
        session.project_id = project_id.map(str::to_string);
        session.memory_policy = memory_policy;
        session.updated_at = Utc::now();
        Ok(session.clone())
    }

    async fn end_session(&self, session_id: Uuid) -> Result<SessionRecord> {
        let mut inner = self.inner.lock();
        if !inner.sessions.contains_key(&session_id) {
            return Err(ServerError::NotFound(format!("session {session_id}")));
        }
        if inner.turns.iter().any(|turn| {
            turn.session_id == session_id && matches!(turn.status.as_str(), "queued" | "running")
        }) {
            return Err(ServerError::Conflict(format!(
                "session {session_id} has queued or running turns"
            )));
        }
        let session = inner
            .sessions
            .get_mut(&session_id)
            .expect("session existence checked");
        session.status = "ended".to_string();
        session.updated_at = Utc::now();
        Ok(session.clone())
    }

    async fn end_session_and_enqueue_dream(
        &self,
        session_id: Uuid,
        subject: String,
        scope: String,
    ) -> Result<EndSessionDreamResult> {
        validate_persistence_identifier("dream subject", &subject)?;
        validate_persistence_identifier("dream scope", &scope)?;
        let now = Utc::now();
        let mut inner = self.inner.lock();
        let (was_ended, subject, scope) = {
            let session = inner
                .sessions
                .get(&session_id)
                .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
            (
                session.status == "ended",
                session.owner_subject.clone(),
                session.memory_scope(),
            )
        };
        validate_persistence_identifier("dream subject", &subject)?;
        validate_persistence_identifier("dream scope", &scope)?;
        if inner.turns.iter().any(|turn| {
            turn.session_id == session_id && matches!(turn.status.as_str(), "queued" | "running")
        }) {
            return Err(ServerError::Conflict(format!(
                "session {session_id} has queued or running turns"
            )));
        }
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            session.status = "ended".to_string();
            session.updated_at = now;
        }

        let mut emitted = Vec::new();
        let base_seq = inner
            .events
            .get(&session_id)
            .map_or(0, |events| events.len() as i64);
        let existing_end_seq = inner
            .events
            .get(&session_id)
            .and_then(|events| {
                events
                    .iter()
                    .rev()
                    .find(|event| event.event_type == "session_end")
            })
            .map(|event| event.seq);
        let new = NewDreamQueueRecord::session_end(session_id, subject, scope, None);
        let inserts_dream = !inner
            .dream_queue
            .iter()
            .any(|record| record.dedupe_key == new.dedupe_key);
        let emits_end = !was_ended || existing_end_seq.is_none() || inserts_dream;
        let source_event_seq = if emits_end {
            Some(base_seq + if inserts_dream { 2 } else { 1 })
        } else {
            existing_end_seq
        };
        let (dream, inserted) = if let Some(existing) = inner
            .dream_queue
            .iter_mut()
            .find(|record| record.dedupe_key == new.dedupe_key)
        {
            existing.subject = new.subject;
            existing.scope = new.scope;
            if existing.source_event_seq.is_none() {
                existing.source_event_seq = source_event_seq;
            }
            (existing.clone(), false)
        } else {
            let dream = DreamQueueRecord {
                id: Uuid::new_v4(),
                session_id,
                subject: new.subject,
                scope: new.scope,
                reason: new.reason,
                status: DreamStatus::Queued,
                dedupe_key: new.dedupe_key,
                source_event_seq,
                attempts: 0,
                enqueued_at: now,
                available_at: new.available_at,
                locked_at: None,
                lease_owner: None,
                lease_epoch: 0,
                heartbeat_at: None,
                completed_at: None,
                error_at: None,
                last_error: None,
            };
            inner.dream_queue.push(dream.clone());
            (dream, true)
        };

        if inserted {
            let events = inner.events.entry(session_id).or_default();
            let event = SessionEvent::new(
                session_id,
                events.len() as i64 + 1,
                "dream_queued",
                json!({
                    "dreamId": dream.id,
                    "sessionId": dream.session_id,
                    "reason": dream.reason,
                    "status": dream.status,
                    "subject": dream.subject,
                    "scope": dream.scope,
                    "dedupeKey": dream.dedupe_key,
                    "sourceEventSeq": dream.source_event_seq,
                    "availableAt": dream.available_at,
                }),
                now,
            );
            events.push(event.clone());
            emitted.push(event);
        }

        if emits_end {
            let events = inner.events.entry(session_id).or_default();
            let event = SessionEvent::new(
                session_id,
                events.len() as i64 + 1,
                "session_end",
                json!({
                    "status": "ended",
                    "reason": "user_requested",
                    "subject": dream.subject,
                    "scope": dream.scope,
                }),
                now,
            );
            debug_assert_eq!(Some(event.seq), source_event_seq);
            events.push(event.clone());
            emitted.push(event);
        }

        let session = inner
            .sessions
            .get(&session_id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        Ok(EndSessionDreamResult {
            session,
            dream,
            events: emitted,
            newly_ended: !was_ended,
        })
    }

    async fn get_session(&self, session_id: Uuid) -> Result<SessionRecord> {
        self.inner
            .lock()
            .sessions
            .get(&session_id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))
    }

    async fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummaryRecord>> {
        let inner = self.inner.lock();
        let mut summaries = inner
            .sessions
            .values()
            .map(|session| {
                let messages = inner.messages.get(&session.id).cloned().unwrap_or_default();
                let title = messages
                    .iter()
                    .find(|message| message.role == "user")
                    .map(|message| message.content.clone());
                let project_summary = inner
                    .project_items
                    .iter()
                    .filter(|item| {
                        item.source_session_id == session.id
                            && item.kind == ProjectItemKind::Summary
                    })
                    .max_by_key(|item| item.created_at)
                    .map(|item| item.text.clone());
                let latest_assistant = messages
                    .iter()
                    .rev()
                    .find(|message| message.role == "assistant")
                    .map(|message| message.content.clone());
                let summary = project_summary.or(latest_assistant);
                let preview = messages.last().map(|message| message.content.clone());
                let last_event_id = inner
                    .events
                    .get(&session.id)
                    .and_then(|events| events.last())
                    .map(|event| event.seq);
                SessionSummaryRecord {
                    session: session.clone(),
                    summary,
                    title,
                    preview,
                    message_count: messages.len() as i64,
                    last_event_id,
                }
            })
            .collect::<Vec<_>>();
        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.session.updated_at));
        summaries.truncate(limit);
        Ok(summaries)
    }

    async fn session_messages(&self, session_id: Uuid) -> Result<Vec<MessageRecord>> {
        self.get_session(session_id).await?;
        Ok(self
            .inner
            .lock()
            .messages
            .get(&session_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn set_mode_state(
        &self,
        session_id: Uuid,
        mode_state: ModeState,
    ) -> Result<SessionRecord> {
        let mut inner = self.inner.lock();
        let session = inner
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        session.mode = mode_state.mode.clone();
        session.mode_state = mode_state;
        session.updated_at = Utc::now();
        Ok(session.clone())
    }

    async fn append_message(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
    ) -> Result<MessageRecord> {
        self.append_message_for_turn(session_id, role, content, None)
            .await
    }

    async fn append_message_for_turn(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
        turn_id: Option<Uuid>,
    ) -> Result<MessageRecord> {
        validate_persistence_identifier("message role", role)?;
        let mut inner = self.inner.lock();
        if !inner.sessions.contains_key(&session_id) {
            return Err(ServerError::NotFound(format!("session {session_id}")));
        }
        if let Some(turn_id) = turn_id {
            if !inner
                .turns
                .iter()
                .any(|turn| turn.id == turn_id && turn.session_id == session_id)
            {
                return Err(ServerError::NotFound(format!("turn {turn_id}")));
            }
            if inner.messages.get(&session_id).is_some_and(|messages| {
                messages
                    .iter()
                    .any(|message| message.turn_id == Some(turn_id) && message.role == role)
            }) {
                return Err(ServerError::Conflict(format!(
                    "turn {turn_id} already has a {role} message"
                )));
            }
        }
        let messages = inner.messages.entry(session_id).or_default();
        let seq = messages.len() as i64 + 1;
        let message = MessageRecord {
            session_id,
            seq,
            role: role.to_string(),
            content: redact_persisted_text(content),
            turn_id,
            created_at: Utc::now(),
        };
        messages.push(message.clone());
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            session.updated_at = message.created_at;
        }
        Ok(message)
    }

    async fn append_event(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
    ) -> Result<SessionEvent> {
        self.append_event_for_turn(session_id, event_type, payload_json, None)
            .await
    }

    async fn append_event_for_turn(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
        turn_id: Option<Uuid>,
    ) -> Result<SessionEvent> {
        validate_persistence_identifier("event type", event_type)?;
        let mut inner = self.inner.lock();
        if !inner.sessions.contains_key(&session_id) {
            return Err(ServerError::NotFound(format!("session {session_id}")));
        }
        if let Some(turn_id) = turn_id
            && !inner
                .turns
                .iter()
                .any(|turn| turn.id == turn_id && turn.session_id == session_id)
        {
            return Err(ServerError::NotFound(format!("turn {turn_id}")));
        }
        let events = inner.events.entry(session_id).or_default();
        let seq = events.len() as i64 + 1;
        let mut event = SessionEvent::new(session_id, seq, event_type, payload_json, Utc::now());
        event.turn_id = turn_id;
        events.push(event.clone());
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            session.updated_at = event.created_at;
        }
        Ok(event)
    }

    async fn append_event_for_turn_once(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
        turn_id: Uuid,
    ) -> Result<(SessionEvent, bool)> {
        validate_persistence_identifier("event type", event_type)?;
        let mut inner = self.inner.lock();
        if !inner.sessions.contains_key(&session_id) {
            return Err(ServerError::NotFound(format!("session {session_id}")));
        }
        if !inner
            .turns
            .iter()
            .any(|turn| turn.id == turn_id && turn.session_id == session_id)
        {
            return Err(ServerError::NotFound(format!("turn {turn_id}")));
        }
        let events = inner.events.entry(session_id).or_default();
        if let Some(existing) = events
            .iter()
            .find(|event| event.turn_id == Some(turn_id) && event.event_type == event_type)
            .cloned()
        {
            return Ok((existing, false));
        }
        let seq = events.len() as i64 + 1;
        let mut event = SessionEvent::new(session_id, seq, event_type, payload_json, Utc::now());
        event.turn_id = Some(turn_id);
        events.push(event.clone());
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            session.updated_at = event.created_at;
        }
        Ok((event, true))
    }

    async fn events_after(
        &self,
        session_id: Uuid,
        last_event_id: Option<i64>,
    ) -> Result<Vec<SessionEvent>> {
        self.get_session(session_id).await?;
        let min_seq = last_event_id.unwrap_or(0);
        Ok(self
            .inner
            .lock()
            .events
            .get(&session_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|event| event.seq > min_seq)
            .collect())
    }

    async fn enqueue_turn(
        &self,
        session_id: Uuid,
        client_message_id: &str,
        content: &str,
    ) -> Result<SessionTurnRecord> {
        let client_message_id = client_message_id.trim();
        if client_message_id.is_empty() || client_message_id.chars().count() > 128 {
            return Err(ServerError::InvalidRequest(
                "client message id must contain 1 to 128 characters".to_string(),
            ));
        }
        validate_persistence_identifier("client message id", client_message_id)?;
        if content.trim().is_empty() {
            return Err(ServerError::InvalidRequest(
                "turn content must not be empty".to_string(),
            ));
        }
        let content_hash = turn_content_hash(content);
        let persisted_content = redact_persisted_text(content);
        let mut inner = self.inner.lock();
        let session = inner
            .sessions
            .get(&session_id)
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        if session.status == "ended" {
            return Err(ServerError::Conflict(format!(
                "session {session_id} has ended"
            )));
        }
        if let Some(existing) = inner.turns.iter().find(|turn| {
            turn.session_id == session_id && turn.client_message_id == client_message_id
        }) {
            if existing.content_hash == content_hash {
                return Ok(existing.clone());
            }
            return Err(ServerError::Conflict(format!(
                "client message id {client_message_id} was already used with different content"
            )));
        }

        let now = Utc::now();
        let turn = SessionTurnRecord {
            id: Uuid::new_v4(),
            session_id,
            client_message_id: client_message_id.to_string(),
            content: persisted_content.clone(),
            content_hash,
            status: "queued".to_string(),
            created_at: now,
            updated_at: now,
            started_at: None,
            completed_at: None,
            worker_id: None,
            error: None,
        };
        let messages = inner.messages.entry(session_id).or_default();
        messages.push(MessageRecord {
            session_id,
            seq: messages.len() as i64 + 1,
            role: "user".to_string(),
            content: persisted_content,
            turn_id: Some(turn.id),
            created_at: now,
        });
        inner.turns.push(turn.clone());
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            session.updated_at = now;
        }
        Ok(turn)
    }

    async fn turn(&self, turn_id: Uuid) -> Result<SessionTurnRecord> {
        self.inner
            .lock()
            .turns
            .iter()
            .find(|turn| turn.id == turn_id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("turn {turn_id}")))
    }

    async fn claim_next_turn(
        &self,
        worker_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<SessionTurnRecord>> {
        let mut inner = self.inner.lock();
        let running_sessions = inner
            .turns
            .iter()
            .filter(|turn| turn.status == "running")
            .map(|turn| turn.session_id)
            .collect::<std::collections::BTreeSet<_>>();
        let Some(index) = inner
            .turns
            .iter()
            .enumerate()
            .filter(|(_, turn)| {
                turn.status == "queued"
                    && !running_sessions.contains(&turn.session_id)
                    && inner
                        .sessions
                        .get(&turn.session_id)
                        .is_some_and(|session| session.status != "ended")
            })
            .min_by_key(|(_, turn)| (turn.created_at, turn.id))
            .map(|(index, _)| index)
        else {
            return Ok(None);
        };
        let turn = &mut inner.turns[index];
        turn.status = "running".to_string();
        turn.updated_at = turn.updated_at.max(now);
        turn.started_at = Some(now);
        turn.completed_at = None;
        turn.worker_id = Some(worker_id);
        turn.error = None;
        Ok(Some(turn.clone()))
    }

    async fn fail_stale_running_turns(
        &self,
        stale_before: DateTime<Utc>,
        failed_at: DateTime<Utc>,
        error: &str,
    ) -> Result<usize> {
        let mut inner = self.inner.lock();
        let mut count = 0;
        for turn in &mut inner.turns {
            if turn.status == "running" && turn.updated_at <= stale_before {
                turn.status = "failed".to_string();
                turn.updated_at = failed_at;
                turn.completed_at = Some(failed_at);
                turn.error = Some(redact_persisted_text(error));
                count += 1;
            }
        }
        Ok(count)
    }

    async fn heartbeat_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<SessionTurnRecord> {
        let mut inner = self.inner.lock();
        let turn = inner
            .turns
            .iter_mut()
            .find(|turn| turn.id == turn_id)
            .ok_or_else(|| ServerError::NotFound(format!("turn {turn_id}")))?;
        if turn.status != "running" || turn.worker_id != Some(worker_id) {
            return Err(ServerError::Conflict(format!(
                "turn {turn_id} is not owned by worker {worker_id}"
            )));
        }
        turn.updated_at = turn.updated_at.max(now);
        Ok(turn.clone())
    }

    async fn complete_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        assistant_content: &str,
        completed_at: DateTime<Utc>,
    ) -> Result<SessionTurnRecord> {
        let mut inner = self.inner.lock();
        let index = inner
            .turns
            .iter()
            .position(|turn| turn.id == turn_id)
            .ok_or_else(|| ServerError::NotFound(format!("turn {turn_id}")))?;
        if inner.turns[index].status == "completed" {
            return Ok(inner.turns[index].clone());
        }
        if inner.turns[index].status != "running" || inner.turns[index].worker_id != Some(worker_id)
        {
            return Err(ServerError::Conflict(format!(
                "turn {turn_id} is not owned by worker {worker_id}"
            )));
        }
        let session_id = inner.turns[index].session_id;
        let messages = inner.messages.entry(session_id).or_default();
        if messages
            .iter()
            .any(|message| message.turn_id == Some(turn_id) && message.role == "assistant")
        {
            return Err(ServerError::Conflict(format!(
                "turn {turn_id} already has an assistant message"
            )));
        }
        messages.push(MessageRecord {
            session_id,
            seq: messages.len() as i64 + 1,
            role: "assistant".to_string(),
            content: redact_persisted_text(assistant_content),
            turn_id: Some(turn_id),
            created_at: completed_at,
        });
        let turn = &mut inner.turns[index];
        turn.status = "completed".to_string();
        turn.updated_at = completed_at;
        turn.completed_at = Some(completed_at);
        turn.error = None;
        let completed = turn.clone();
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            session.updated_at = completed_at;
        }
        Ok(completed)
    }

    async fn fail_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        error: &str,
        failed_at: DateTime<Utc>,
    ) -> Result<SessionTurnRecord> {
        let mut inner = self.inner.lock();
        let turn = inner
            .turns
            .iter_mut()
            .find(|turn| turn.id == turn_id)
            .ok_or_else(|| ServerError::NotFound(format!("turn {turn_id}")))?;
        if turn.status == "failed" {
            return Ok(turn.clone());
        }
        if turn.status != "running" || turn.worker_id != Some(worker_id) {
            return Err(ServerError::Conflict(format!(
                "turn {turn_id} is not owned by worker {worker_id}"
            )));
        }
        turn.status = "failed".to_string();
        turn.updated_at = failed_at;
        turn.completed_at = Some(failed_at);
        turn.error = Some(redact_persisted_text(error));
        Ok(turn.clone())
    }

    async fn begin_mcp_mutation_effect(
        &self,
        intent: tm_mcp::McpMutationIntent,
    ) -> Result<tm_mcp::McpMutationEffectClaim> {
        let session_id = Uuid::parse_str(&intent.session_id).map_err(|_| {
            ServerError::InvalidRequest("MCP mutation session id is not a UUID".to_string())
        })?;
        let turn_id = Uuid::parse_str(&intent.effect_scope_id).map_err(|_| {
            ServerError::InvalidRequest("MCP mutation effect scope is not a turn UUID".to_string())
        })?;
        let mut inner = self.inner.lock();
        if !inner
            .turns
            .iter()
            .any(|turn| turn.id == turn_id && turn.session_id == session_id)
        {
            return Err(ServerError::NotFound(format!(
                "durable MCP mutation turn {turn_id}"
            )));
        }
        if let Some(existing) = inner.mcp_mutation_effects.get(&intent.effect_id) {
            if !existing.intent.same_effect_identity(&intent) {
                return Err(ServerError::Conflict(
                    "MCP mutation effect id collides with different intent".to_string(),
                ));
            }
            return Ok(tm_mcp::McpMutationEffectClaim {
                record: existing.clone(),
                created: false,
            });
        }
        let record = tm_mcp::McpMutationEffectRecord {
            intent,
            status: tm_mcp::McpMutationEffectStatus::Started,
            result_digest: None,
            result_bytes: None,
            error_code: None,
            error_digest: None,
        };
        inner
            .mcp_mutation_effects
            .insert(record.intent.effect_id.clone(), record.clone());
        Ok(tm_mcp::McpMutationEffectClaim {
            record,
            created: true,
        })
    }

    async fn finish_mcp_mutation_effect(
        &self,
        effect_id: &str,
        status: tm_mcp::McpMutationEffectStatus,
        result_digest: Option<&str>,
        result_bytes: Option<usize>,
        error_code: Option<&str>,
        error_digest: Option<&str>,
    ) -> Result<tm_mcp::McpMutationEffectRecord> {
        if !status.is_terminal() {
            return Err(ServerError::InvalidRequest(
                "MCP mutation finish requires terminal status".to_string(),
            ));
        }
        let mut inner = self.inner.lock();
        let record = inner
            .mcp_mutation_effects
            .get_mut(effect_id)
            .ok_or_else(|| ServerError::NotFound(format!("MCP mutation effect {effect_id}")))?;
        let requested = tm_mcp::McpMutationEffectRecord {
            intent: record.intent.clone(),
            status,
            result_digest: result_digest.map(str::to_string),
            result_bytes,
            error_code: error_code.map(str::to_string),
            error_digest: error_digest.map(str::to_string),
        };
        if record.status.is_terminal() {
            if *record == requested {
                return Ok(record.clone());
            }
            return Err(ServerError::Conflict(
                "MCP mutation effect already has a different terminal state".to_string(),
            ));
        }
        *record = requested;
        Ok(record.clone())
    }

    async fn begin_egress_mutation_effect(
        &self,
        intent: tm_egress::EgressMutationIntent,
    ) -> Result<tm_egress::EgressMutationClaim> {
        let session_id = Uuid::parse_str(&intent.session_id).map_err(|_| {
            ServerError::InvalidRequest("egress mutation session id is not a UUID".to_string())
        })?;
        let turn_id = Uuid::parse_str(&intent.effect_scope_id).map_err(|_| {
            ServerError::InvalidRequest(
                "egress mutation effect scope is not a turn UUID".to_string(),
            )
        })?;
        {
            let inner = self.inner.lock();
            if !inner
                .turns
                .iter()
                .any(|turn| turn.id == turn_id && turn.session_id == session_id)
            {
                return Err(ServerError::NotFound(format!(
                    "durable egress mutation turn {turn_id}"
                )));
            }
        }
        tm_egress::EgressStateStore::begin_mutation(self.egress_state.as_ref(), intent)
            .await
            .map_err(egress_store_error)
    }

    async fn finish_egress_mutation_effect(
        &self,
        effect_id: &str,
        status: tm_egress::EgressMutationStatus,
        result_digest: Option<&str>,
        result_bytes: Option<usize>,
        error_code: Option<&str>,
        error_digest: Option<&str>,
    ) -> Result<tm_egress::EgressMutationRecord> {
        tm_egress::EgressStateStore::finish_mutation(
            self.egress_state.as_ref(),
            effect_id,
            status,
            result_digest,
            result_bytes,
            error_code,
            error_digest,
        )
        .await
        .map_err(egress_store_error)
    }

    async fn reserve_egress_budget(
        &self,
        request: tm_egress::EgressBudgetRequest,
    ) -> Result<tm_egress::EgressBudgetReservation> {
        let session_id = Uuid::parse_str(&request.session_id).map_err(|_| {
            ServerError::InvalidRequest("egress budget session id is not a UUID".to_string())
        })?;
        {
            let inner = self.inner.lock();
            let session = inner
                .sessions
                .get(&session_id)
                .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
            if session.status != "open" {
                return Err(ServerError::Conflict(format!(
                    "session {session_id} is not open"
                )));
            }
        }
        tm_egress::EgressStateStore::reserve_budget(self.egress_state.as_ref(), request)
            .await
            .map_err(egress_store_error)
    }

    async fn settle_egress_budget(
        &self,
        reservation: tm_egress::EgressBudgetReservation,
        response_bytes: u64,
        elapsed_ms: u64,
    ) -> Result<()> {
        tm_egress::EgressStateStore::settle_budget(
            self.egress_state.as_ref(),
            reservation,
            response_bytes,
            elapsed_ms,
        )
        .await
        .map_err(egress_store_error)
    }

    async fn clear_egress_session(&self, session_id: &str) -> Result<()> {
        let parsed = Uuid::parse_str(session_id).map_err(|_| {
            ServerError::InvalidRequest("egress session id is not a UUID".to_string())
        })?;
        {
            let inner = self.inner.lock();
            let session = inner
                .sessions
                .get(&parsed)
                .ok_or_else(|| ServerError::NotFound(format!("session {parsed}")))?;
            if session.status != "ended" {
                return Err(ServerError::Conflict(format!(
                    "active session {parsed} egress state cannot be cleared"
                )));
            }
        }
        tm_egress::EgressStateStore::clear_session(self.egress_state.as_ref(), session_id)
            .await
            .map_err(egress_store_error)
    }

    async fn create_approval_request(
        &self,
        request: NewApprovalRequest,
    ) -> Result<ApprovalRequestRecord> {
        let request = sanitize_approval_request_persistence(request)?;
        let audit = crate::evolution::evolution_audit_record(
            &request.effect_payload_json,
            request.id,
            request.id,
            EvolutionAuditStatus::AwaitingApproval,
            request.created_at,
            None,
        )?
        .map(|record| EvolutionAuditEntry {
            idempotency_key: format!("approval:{}:attempt", request.id),
            record,
        });
        let mut inner = self.inner.lock();
        if !inner.sessions.contains_key(&request.session_id) {
            return Err(ServerError::NotFound(format!(
                "session {}",
                request.session_id
            )));
        }
        if let Some(turn_id) = request.turn_id
            && !inner
                .turns
                .iter()
                .any(|turn| turn.id == turn_id && turn.session_id == request.session_id)
        {
            return Err(ServerError::NotFound(format!("turn {turn_id}")));
        }
        if let Some(existing) = inner
            .approval_requests
            .iter()
            .find(|approval| approval.id == request.id)
            .cloned()
        {
            let effect_matches = inner.approval_effects.iter().any(|effect| {
                effect.approval_id == request.id
                    && effect.effect_type == request.effect_type
                    && effect.payload_json == request.effect_payload_json
            });
            if effect_matches
                && existing.session_id == request.session_id
                && existing.turn_id == request.turn_id
                && existing.requester_id == request.requester_id
                && existing.origin == request.origin
                && existing.action == request.action
                && existing.scope_json == request.scope_json
                && existing.options_json == request.options_json
                && existing.resumable == request.resumable
                && existing.created_at == request.created_at
                && existing.expires_at == request.expires_at
            {
                if let Some(audit) = audit {
                    append_evolution_audit_in_memory(&mut inner, audit)?;
                }
                return Ok(existing);
            }
            return Err(ServerError::Conflict(format!(
                "approval {} already exists with different content",
                request.id
            )));
        }
        let record = ApprovalRequestRecord {
            id: request.id,
            session_id: request.session_id,
            turn_id: request.turn_id,
            requester_id: request.requester_id,
            origin: request.origin,
            action: request.action,
            scope_json: request.scope_json,
            options_json: request.options_json,
            status: "pending".to_string(),
            resumable: request.resumable,
            created_at: request.created_at,
            expires_at: request.expires_at,
            heartbeat_at: request.created_at,
            resolved_at: None,
            selected_option_id: None,
            resolution_json: None,
            request_event_seq: None,
            resolution_event_seq: None,
            resolution_version: 0,
        };
        inner.approval_requests.push(record.clone());
        inner.approval_effects.push(ApprovalEffectRecord {
            id: request.id,
            approval_id: request.id,
            session_id: request.session_id,
            effect_type: request.effect_type,
            payload_json: request.effect_payload_json,
            status: "blocked".to_string(),
            attempts: 0,
            available_at: request.expires_at,
            locked_at: None,
            lease_owner: None,
            lease_epoch: 0,
            applied_at: None,
            error_at: None,
            last_error: None,
            created_at: request.created_at,
            updated_at: request.created_at,
        });
        if let Some(audit) = audit {
            append_evolution_audit_in_memory(&mut inner, audit)?;
        }
        Ok(record)
    }

    async fn approval_request_for_skill_proposal(
        &self,
        session_id: Uuid,
        proposal_id: Uuid,
    ) -> Result<Option<ApprovalRequestRecord>> {
        let inner = self.inner.lock();
        Ok(inner
            .approval_effects
            .iter()
            .find(|effect| {
                effect.session_id == session_id
                    && effect.effect_type == "skill_write"
                    && effect
                        .payload_json
                        .get("proposalId")
                        .and_then(Value::as_str)
                        .and_then(|value| Uuid::parse_str(value).ok())
                        == Some(proposal_id)
            })
            .and_then(|effect| {
                inner
                    .approval_requests
                    .iter()
                    .find(|approval| approval.id == effect.approval_id)
            })
            .cloned())
    }
    async fn create_skill_approval_bundle(
        &self,
        bundle: NewSkillApprovalBundle,
    ) -> Result<SkillApprovalBundleResult> {
        let proposal = bundle.proposal;
        let approval = sanitize_approval_request_persistence(bundle.approval)?;
        let proposal_payload_json = redact_persisted_json(bundle.proposal_payload_json);
        let approval_payload_json = redact_persisted_json(bundle.approval_payload_json);
        let proposal_id = proposal.id.to_string();
        let approval_id = approval.id.to_string();
        if approval.session_id != proposal.source_session_id
            || approval.turn_id.is_some()
            || approval.effect_type != "skill_write"
            || approval
                .effect_payload_json
                .get("proposalId")
                .and_then(Value::as_str)
                != Some(proposal_id.as_str())
            || proposal_payload_json
                .get("proposalId")
                .and_then(Value::as_str)
                != Some(proposal_id.as_str())
            || approval_payload_json
                .get("approvalId")
                .and_then(Value::as_str)
                != Some(approval_id.as_str())
        {
            return Err(ServerError::InvalidRequest(
                "skill approval bundle has inconsistent proposal, approval, or event references"
                    .to_string(),
            ));
        }
        self.get_session(proposal.source_session_id).await?;
        let audit = crate::evolution::evolution_audit_record(
            &approval.effect_payload_json,
            approval.id,
            approval.id,
            EvolutionAuditStatus::AwaitingApproval,
            approval.created_at,
            None,
        )?
        .map(|record| EvolutionAuditEntry {
            idempotency_key: format!("approval:{}:attempt", approval.id),
            record,
        });
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .approval_effects
            .iter()
            .find(|effect| {
                effect.session_id == approval.session_id
                    && effect.effect_type == "skill_write"
                    && effect
                        .payload_json
                        .get("proposalId")
                        .and_then(Value::as_str)
                        == Some(proposal_id.as_str())
            })
            .and_then(|effect| {
                inner
                    .approval_requests
                    .iter()
                    .find(|request| request.id == effect.approval_id)
            })
            .cloned()
        {
            return Ok(SkillApprovalBundleResult {
                approval: existing,
                events: Vec::new(),
            });
        }
        if !inner
            .skill_proposals
            .iter()
            .any(|existing| existing.id == proposal.id && existing == &proposal)
        {
            return Err(ServerError::Conflict(format!(
                "skill proposal {} changed before approval persistence",
                proposal.id
            )));
        }
        let first_seq = inner
            .events
            .get(&approval.session_id)
            .map_or(1, |events| events.len() as i64 + 1);
        let proposal_event = SessionEvent::new(
            approval.session_id,
            first_seq,
            "write_proposal",
            proposal_payload_json,
            approval.created_at,
        );
        let approval_event = SessionEvent::new(
            approval.session_id,
            first_seq + 1,
            "approval",
            approval_payload_json,
            approval.created_at,
        );
        let approval_record = ApprovalRequestRecord {
            id: approval.id,
            session_id: approval.session_id,
            turn_id: approval.turn_id,
            requester_id: approval.requester_id,
            origin: approval.origin,
            action: approval.action,
            scope_json: approval.scope_json,
            options_json: approval.options_json,
            status: "pending".to_string(),
            resumable: approval.resumable,
            created_at: approval.created_at,
            expires_at: approval.expires_at,
            heartbeat_at: approval.created_at,
            resolved_at: None,
            selected_option_id: None,
            resolution_json: None,
            request_event_seq: Some(approval_event.seq),
            resolution_event_seq: None,
            resolution_version: 0,
        };
        let effect = ApprovalEffectRecord {
            id: approval.id,
            approval_id: approval.id,
            session_id: approval.session_id,
            effect_type: approval.effect_type,
            payload_json: approval.effect_payload_json,
            status: "blocked".to_string(),
            attempts: 0,
            available_at: approval.expires_at,
            locked_at: None,
            lease_owner: None,
            lease_epoch: 0,
            applied_at: None,
            error_at: None,
            last_error: None,
            created_at: approval.created_at,
            updated_at: approval.created_at,
        };
        inner.approval_requests.push(approval_record.clone());
        inner.approval_effects.push(effect);
        if let Some(audit) = audit {
            append_evolution_audit_in_memory(&mut inner, audit)?;
        }
        inner
            .events
            .entry(approval.session_id)
            .or_default()
            .extend([proposal_event.clone(), approval_event.clone()]);
        if let Some(session) = inner.sessions.get_mut(&approval.session_id) {
            session.updated_at = approval.created_at;
        }
        Ok(SkillApprovalBundleResult {
            approval: approval_record,
            events: vec![proposal_event, approval_event],
        })
    }

    async fn approval_request(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
    ) -> Result<ApprovalRequestRecord> {
        self.inner
            .lock()
            .approval_requests
            .iter()
            .find(|approval| approval.id == approval_id && approval.session_id == session_id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("approval {approval_id}")))
    }

    async fn heartbeat_approval_request(
        &self,
        approval_id: Uuid,
        requester_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<ApprovalRequestRecord> {
        let mut inner = self.inner.lock();
        let approval = inner
            .approval_requests
            .iter_mut()
            .find(|approval| approval.id == approval_id)
            .ok_or_else(|| ServerError::NotFound(format!("approval {approval_id}")))?;
        if approval.status == "pending" {
            if approval.requester_id != requester_id {
                return Err(ServerError::Conflict(format!(
                    "approval {approval_id} belongs to a different requester"
                )));
            }
            approval.heartbeat_at = now;
        }
        Ok(approval.clone())
    }

    async fn resolve_approval_request(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        resolution: NewApprovalResolution,
    ) -> Result<ApprovalRequestRecord> {
        let mut inner = self.inner.lock();
        resolve_approval_in_memory(&mut inner, session_id, approval_id, resolution)
    }

    async fn resolve_approval_request_with_event(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        resolution: NewApprovalResolution,
    ) -> Result<(ApprovalRequestRecord, SessionEvent)> {
        let mut inner = self.inner.lock();
        resolve_approval_with_event_in_memory(&mut inner, session_id, approval_id, resolution)
    }

    async fn link_approval_event(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        event_type: &str,
        event_seq: i64,
    ) -> Result<ApprovalRequestRecord> {
        if !matches!(event_type, "approval" | "approval_resolved") {
            return Err(ServerError::InvalidRequest(format!(
                "unsupported approval event type {event_type}"
            )));
        }
        let mut inner = self.inner.lock();
        if !inner.events.get(&session_id).is_some_and(|events| {
            events.iter().any(|event| {
                event.seq == event_seq
                    && event.event_type == event_type
                    && event.payload_json.get("approvalId").and_then(Value::as_str)
                        == Some(approval_id.to_string().as_str())
            })
        }) {
            return Err(ServerError::NotFound(format!(
                "approval event {session_id}:{event_seq}"
            )));
        }
        let approval = inner
            .approval_requests
            .iter_mut()
            .find(|approval| approval.id == approval_id && approval.session_id == session_id)
            .ok_or_else(|| ServerError::NotFound(format!("approval {approval_id}")))?;
        let linked_seq = if event_type == "approval" {
            &mut approval.request_event_seq
        } else {
            &mut approval.resolution_event_seq
        };
        if linked_seq.is_some_and(|existing| existing != event_seq) {
            return Err(ServerError::Conflict(format!(
                "approval {approval_id} already links a different {event_type} event"
            )));
        }
        *linked_seq = Some(event_seq);
        Ok(approval.clone())
    }

    async fn cancel_stale_non_resumable_approvals(
        &self,
        stale_before: DateTime<Utc>,
        cancelled_at: DateTime<Utc>,
    ) -> Result<Vec<SessionEvent>> {
        let mut inner = self.inner.lock();
        let candidates = inner
            .approval_requests
            .iter()
            .filter(|approval| {
                approval.status == "pending"
                    && !approval.resumable
                    && approval.heartbeat_at <= stale_before
            })
            .map(|approval| (approval.id, approval.session_id, approval.origin.clone()))
            .collect::<Vec<_>>();
        let mut events = Vec::with_capacity(candidates.len());
        for (approval_id, session_id, origin) in candidates {
            let (_, event) = resolve_approval_with_event_in_memory(
                &mut inner,
                session_id,
                approval_id,
                NewApprovalResolution {
                    status: "cancelled".to_string(),
                    selected_option_id: None,
                    resolution_json: json!({
                        "approvalId": approval_id,
                        "backend": origin,
                        "status": "cancelled",
                        "outcome": "cancelled",
                        "reason": "stale_non_resumable",
                    }),
                    resolved_at: cancelled_at,
                },
            )?;
            events.push(event);
        }
        Ok(events)
    }

    async fn expire_pending_approvals(&self, now: DateTime<Utc>) -> Result<Vec<SessionEvent>> {
        let mut inner = self.inner.lock();
        let candidates = inner
            .approval_requests
            .iter()
            .filter(|approval| approval.status == "pending" && approval.expires_at <= now)
            .map(|approval| (approval.id, approval.session_id, approval.origin.clone()))
            .collect::<Vec<_>>();
        let mut events = Vec::with_capacity(candidates.len());
        for (approval_id, session_id, origin) in candidates {
            let (_, event) = resolve_approval_with_event_in_memory(
                &mut inner,
                session_id,
                approval_id,
                NewApprovalResolution {
                    status: "timed_out".to_string(),
                    selected_option_id: None,
                    resolution_json: json!({
                        "approvalId": approval_id,
                        "backend": origin,
                        "status": "timed_out",
                        "outcome": "cancelled",
                        "reason": "expired",
                    }),
                    resolved_at: now,
                },
            )?;
            events.push(event);
        }
        Ok(events)
    }

    async fn claim_approval_effect(
        &self,
        approval_id: Uuid,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<Option<ApprovalEffectLease>> {
        let stale_before = now - lease_timeout;
        let mut inner = self.inner.lock();
        let Some(effect) = inner.approval_effects.iter_mut().find(|effect| {
            effect.approval_id == approval_id
                && approval_effect_is_claimable(effect, now, stale_before)
        }) else {
            return Ok(None);
        };
        Ok(Some(claim_approval_effect_at(effect, owner_id, now)))
    }

    async fn claim_next_approval_effect(
        &self,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<Option<ApprovalEffectLease>> {
        let stale_before = now - lease_timeout;
        let mut inner = self.inner.lock();
        let Some(index) = inner
            .approval_effects
            .iter()
            .enumerate()
            .filter(|(_, effect)| approval_effect_is_claimable(effect, now, stale_before))
            .min_by_key(|(_, effect)| (effect.available_at, effect.created_at, effect.id))
            .map(|(index, _)| index)
        else {
            return Ok(None);
        };
        Ok(Some(claim_approval_effect_at(
            &mut inner.approval_effects[index],
            owner_id,
            now,
        )))
    }

    async fn heartbeat_approval_effect(
        &self,
        lease: &ApprovalEffectLease,
        now: DateTime<Utc>,
    ) -> Result<ApprovalEffectRecord> {
        let mut inner = self.inner.lock();
        let effect = inner
            .approval_effects
            .iter_mut()
            .find(|effect| {
                effect.id == lease.effect.id
                    && effect.status == "claimed"
                    && effect.lease_owner == Some(lease.owner_id)
                    && effect.lease_epoch == lease.epoch
            })
            .ok_or_else(|| stale_approval_effect_lease(lease))?;
        effect.locked_at = Some(now);
        effect.updated_at = now;
        Ok(effect.clone())
    }

    async fn complete_approval_effect(
        &self,
        lease: &ApprovalEffectLease,
        applied_at: DateTime<Utc>,
    ) -> Result<ApprovalEffectRecord> {
        let mut inner = self.inner.lock();
        let effect = inner
            .approval_effects
            .iter_mut()
            .find(|effect| {
                effect.id == lease.effect.id
                    && effect.status == "claimed"
                    && effect.lease_owner == Some(lease.owner_id)
                    && effect.lease_epoch == lease.epoch
            })
            .ok_or_else(|| stale_approval_effect_lease(lease))?;
        if !matches!(
            effect.effect_type.as_str(),
            "approval_continuation" | "approval_resolution"
        ) {
            return Err(ServerError::InvalidRequest(
                "durable proposal effects must complete with their write_proposal event"
                    .to_string(),
            ));
        }
        effect.status = "applied".to_string();
        effect.locked_at = None;
        effect.lease_owner = None;
        effect.applied_at = Some(applied_at);
        effect.updated_at = applied_at;
        effect.error_at = None;
        effect.last_error = None;
        Ok(effect.clone())
    }

    async fn complete_approval_effect_with_event(
        &self,
        lease: &ApprovalEffectLease,
        proposal_payload_json: Value,
        turn_id: Option<Uuid>,
        applied_at: DateTime<Utc>,
    ) -> Result<(ApprovalEffectRecord, SessionEvent)> {
        let proposal_payload_json = redact_persisted_json(proposal_payload_json);
        let mut inner = self.inner.lock();
        let index = inner
            .approval_effects
            .iter()
            .position(|effect| {
                effect.id == lease.effect.id
                    && effect.status == "claimed"
                    && effect.lease_owner == Some(lease.owner_id)
                    && effect.lease_epoch == lease.epoch
            })
            .ok_or_else(|| stale_approval_effect_lease(lease))?;
        if !matches!(
            inner.approval_effects[index].effect_type.as_str(),
            "memory_write"
                | "skill_write"
                | "skill_rollback"
                | "mode_addendum_rollback"
                | "persona_addendum_rollback"
                | "evolution_review"
        ) {
            return Err(ServerError::InvalidRequest(
                "only durable proposal effects may complete with a write_proposal event"
                    .to_string(),
            ));
        }
        let session_id = inner.approval_effects[index].session_id;
        if !inner.sessions.contains_key(&session_id) {
            return Err(ServerError::NotFound(format!("session {session_id}")));
        }
        if let Some(turn_id) = turn_id
            && !inner
                .turns
                .iter()
                .any(|turn| turn.id == turn_id && turn.session_id == session_id)
        {
            return Err(ServerError::NotFound(format!("turn {turn_id}")));
        }
        let audit = crate::evolution::evolution_audit_record(
            &inner.approval_effects[index].payload_json,
            inner.approval_effects[index].approval_id,
            inner.approval_effects[index].id,
            EvolutionAuditStatus::Applied,
            applied_at,
            None,
        )?
        .map(|record| EvolutionAuditEntry {
            idempotency_key: format!("effect:{}:applied", lease.effect.id),
            record,
        });

        #[cfg(test)]
        if inner.fail_complete_approval_effect_with_event_once {
            inner.fail_complete_approval_effect_with_event_once = false;
            return Err(ServerError::Store(
                "injected approval effect finalization crash".to_string(),
            ));
        }

        let event = {
            let events = inner.events.entry(session_id).or_default();
            let mut event = SessionEvent::new(
                session_id,
                events.len() as i64 + 1,
                "write_proposal",
                proposal_payload_json,
                applied_at,
            );
            event.turn_id = turn_id;
            events.push(event.clone());
            event
        };
        let effect = &mut inner.approval_effects[index];
        effect.status = "applied".to_string();
        effect.locked_at = None;
        effect.lease_owner = None;
        effect.applied_at = Some(applied_at);
        effect.updated_at = applied_at;
        effect.error_at = None;
        effect.last_error = None;
        let effect = effect.clone();
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            session.updated_at = applied_at;
        }
        if let Some(audit) = audit {
            append_evolution_audit_in_memory(&mut inner, audit)?;
        }
        Ok((effect, event))
    }

    async fn fail_approval_effect(
        &self,
        lease: &ApprovalEffectLease,
        error: &str,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<ApprovalEffectRecord> {
        let error = redact_persisted_text(error);
        let mut inner = self.inner.lock();
        let index = inner
            .approval_effects
            .iter()
            .position(|effect| {
                effect.id == lease.effect.id
                    && effect.status == "claimed"
                    && effect.lease_owner == Some(lease.owner_id)
                    && effect.lease_epoch == lease.epoch
            })
            .ok_or_else(|| stale_approval_effect_lease(lease))?;
        let failed_at = Utc::now();
        let audit = crate::evolution::evolution_audit_record(
            &inner.approval_effects[index].payload_json,
            inner.approval_effects[index].approval_id,
            inner.approval_effects[index].id,
            EvolutionAuditStatus::Failed,
            failed_at,
            crate::evolution::evolution_policy_error_code(&error),
        )?
        .map(|record| EvolutionAuditEntry {
            idempotency_key: format!("effect:{}:failure:{}", lease.effect.id, lease.epoch),
            record,
        });
        let effect = &mut inner.approval_effects[index];
        effect.status = if effect.attempts >= max_attempts {
            "failed".to_string()
        } else {
            "pending".to_string()
        };
        effect.available_at = next_available_at;
        effect.locked_at = None;
        effect.lease_owner = None;
        effect.updated_at = failed_at;
        effect.error_at = Some(failed_at);
        effect.last_error = Some(error);
        let effect = effect.clone();
        if let Some(audit) = audit {
            append_evolution_audit_in_memory(&mut inner, audit)?;
        }
        Ok(effect)
    }

    async fn append_evolution_audit(
        &self,
        entry: EvolutionAuditEntry,
    ) -> Result<EvolutionAuditRecord> {
        let mut inner = self.inner.lock();
        if !inner.sessions.contains_key(&entry.record.origin.session_id) {
            return Err(ServerError::NotFound(format!(
                "session {}",
                entry.record.origin.session_id
            )));
        }
        append_evolution_audit_in_memory(&mut inner, entry)
    }

    async fn evolution_audits(&self, session_id: Uuid) -> Result<Vec<EvolutionAuditRecord>> {
        let records = self
            .inner
            .lock()
            .evolution_audits
            .iter()
            .filter(|entry| entry.record.origin.session_id == session_id)
            .map(|entry| entry.record.clone())
            .collect::<Vec<_>>();
        Ok(records)
    }

    async fn evolution_memory_proposal(
        &self,
        proposal_id: Uuid,
    ) -> Result<crate::MemoryWriteProposal> {
        self.inner
            .lock()
            .approval_effects
            .iter()
            .filter_map(|effect| effect.payload_json.get("proposal"))
            .find(|proposal| {
                proposal
                    .get("proposalId")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    == Some(proposal_id)
            })
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .ok_or_else(|| ServerError::NotFound(format!("evolution proposal {proposal_id}")))
    }

    async fn create_evolution_review_proposal(
        &self,
        proposal: NewEvolutionReviewProposal,
    ) -> Result<EvolutionReviewProposalRecord> {
        let proposal = sanitize_evolution_review_proposal_persistence(proposal)?;
        self.get_session(proposal.session_id).await?;
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .evolution_review_proposals
            .iter()
            .find(|existing| existing.id == proposal.id)
        {
            if existing.session_id == proposal.session_id
                && existing.target == proposal.target
                && existing.base_version == proposal.base_version
                && existing.base_digest == proposal.base_digest
                && existing.base_active_digest == proposal.base_active_digest
                && existing.changes == proposal.changes
                && existing.content_digest == proposal.content_digest
                && existing.apply_contract == proposal.apply_contract
                && existing.auto_candidate == proposal.auto_candidate
            {
                return Ok(existing.clone());
            }
            return Err(ServerError::Conflict(format!(
                "evolution review proposal {} already exists with different content",
                proposal.id
            )));
        }
        let now = Utc::now();
        let record = EvolutionReviewProposalRecord {
            id: proposal.id,
            session_id: proposal.session_id,
            target: proposal.target,
            base_version: proposal.base_version,
            base_digest: proposal.base_digest,
            base_active_digest: proposal.base_active_digest,
            changes: proposal.changes,
            content_digest: proposal.content_digest,
            status: ReviewProposalStatus::Pending,
            apply_contract: proposal.apply_contract,
            auto_candidate: proposal.auto_candidate,
            created_at: now,
            updated_at: now,
        };
        inner.evolution_review_proposals.push(record.clone());
        Ok(record)
    }

    async fn create_auto_evolution_review_proposal(
        &self,
        proposal: NewEvolutionReviewProposal,
        cooldown_since: DateTime<Utc>,
    ) -> Result<AutoEvolutionReviewProposalResult> {
        let proposal = sanitize_evolution_review_proposal_persistence(proposal)?;
        let candidate = proposal.auto_candidate.as_ref().ok_or_else(|| {
            ServerError::InvalidRequest(
                "auto evolution review proposal is missing candidate metadata".to_string(),
            )
        })?;
        self.get_session(proposal.session_id).await?;
        let source_turn = self.turn(candidate.source_turn_id).await?;
        if source_turn.session_id != proposal.session_id || source_turn.status != "completed" {
            return Err(ServerError::Conflict(
                "persona auto-candidate source turn is not durably completed".to_string(),
            ));
        }

        let mut inner = self.inner.lock();
        let blocked = inner
            .evolution_review_proposals
            .iter()
            .filter(|existing| {
                existing
                    .auto_candidate
                    .as_ref()
                    .is_some_and(|existing| existing.dedupe_key == candidate.dedupe_key)
                    || (existing.target == proposal.target && existing.changes == proposal.changes)
            })
            .filter(|existing| {
                matches!(
                    existing.status,
                    ReviewProposalStatus::Pending | ReviewProposalStatus::Approved
                ) || existing.updated_at >= cooldown_since
            })
            .max_by(|left, right| {
                let left_open = matches!(
                    left.status,
                    ReviewProposalStatus::Pending | ReviewProposalStatus::Approved
                );
                let right_open = matches!(
                    right.status,
                    ReviewProposalStatus::Pending | ReviewProposalStatus::Approved
                );
                left_open
                    .cmp(&right_open)
                    .then_with(|| left.updated_at.cmp(&right.updated_at))
                    .then_with(|| left.id.cmp(&right.id))
            })
            .cloned();
        if let Some(existing) = blocked {
            let disposition = if matches!(
                existing.status,
                ReviewProposalStatus::Pending | ReviewProposalStatus::Approved
            ) {
                AutoEvolutionReviewDisposition::Duplicate
            } else {
                AutoEvolutionReviewDisposition::Cooldown
            };
            return Ok(AutoEvolutionReviewProposalResult {
                disposition,
                proposal: existing,
            });
        }

        let now = Utc::now();
        let record = EvolutionReviewProposalRecord {
            id: proposal.id,
            session_id: proposal.session_id,
            target: proposal.target,
            base_version: proposal.base_version,
            base_digest: proposal.base_digest,
            base_active_digest: proposal.base_active_digest,
            changes: proposal.changes,
            content_digest: proposal.content_digest,
            status: ReviewProposalStatus::Pending,
            apply_contract: proposal.apply_contract,
            auto_candidate: proposal.auto_candidate,
            created_at: now,
            updated_at: now,
        };
        inner.evolution_review_proposals.push(record.clone());
        Ok(AutoEvolutionReviewProposalResult {
            disposition: AutoEvolutionReviewDisposition::Created,
            proposal: record,
        })
    }

    async fn create_auto_evolution_review_bundle(
        &self,
        bundle: NewAutoEvolutionReviewBundle,
    ) -> Result<AutoEvolutionReviewBundleResult> {
        let proposal = sanitize_evolution_review_proposal_persistence(bundle.proposal)?;
        let approval = sanitize_approval_request_persistence(bundle.approval)?;
        let proposal_payload_json = redact_persisted_json(bundle.proposal_payload_json);
        let approval_payload_json = redact_persisted_json(bundle.approval_payload_json);
        let candidate = proposal.auto_candidate.as_ref().ok_or_else(|| {
            ServerError::InvalidRequest(
                "auto evolution review proposal is missing candidate metadata".to_string(),
            )
        })?;
        let proposal_id = proposal.id.to_string();
        let approval_id = approval.id.to_string();
        if approval.session_id != proposal.session_id
            || approval.turn_id != Some(candidate.source_turn_id)
            || approval.effect_type != "evolution_review"
            || approval
                .effect_payload_json
                .get("proposalId")
                .and_then(Value::as_str)
                != Some(proposal_id.as_str())
            || proposal_payload_json
                .get("proposalId")
                .and_then(Value::as_str)
                != Some(proposal_id.as_str())
            || approval_payload_json
                .get("approvalId")
                .and_then(Value::as_str)
                != Some(approval_id.as_str())
        {
            return Err(ServerError::InvalidRequest(
                "auto evolution review bundle has inconsistent proposal, approval, or event references"
                    .to_string(),
            ));
        }
        self.get_session(proposal.session_id).await?;
        let source_turn = self.turn(candidate.source_turn_id).await?;
        if source_turn.session_id != proposal.session_id || source_turn.status != "completed" {
            return Err(ServerError::Conflict(
                "persona auto-candidate source turn is not durably completed".to_string(),
            ));
        }
        let audit = crate::evolution::evolution_audit_record(
            &approval.effect_payload_json,
            approval.id,
            approval.id,
            EvolutionAuditStatus::AwaitingApproval,
            approval.created_at,
            None,
        )?
        .map(|record| EvolutionAuditEntry {
            idempotency_key: format!("approval:{}:attempt", approval.id),
            record,
        });

        let mut inner = self.inner.lock();
        let blocked = inner
            .evolution_review_proposals
            .iter()
            .filter(|existing| {
                existing
                    .auto_candidate
                    .as_ref()
                    .is_some_and(|existing| existing.dedupe_key == candidate.dedupe_key)
                    || (existing.target == proposal.target && existing.changes == proposal.changes)
            })
            .filter(|existing| {
                matches!(
                    existing.status,
                    ReviewProposalStatus::Pending | ReviewProposalStatus::Approved
                ) || existing.updated_at >= bundle.cooldown_since
            })
            .max_by(|left, right| {
                let left_open = matches!(
                    left.status,
                    ReviewProposalStatus::Pending | ReviewProposalStatus::Approved
                );
                let right_open = matches!(
                    right.status,
                    ReviewProposalStatus::Pending | ReviewProposalStatus::Approved
                );
                left_open
                    .cmp(&right_open)
                    .then_with(|| left.updated_at.cmp(&right.updated_at))
                    .then_with(|| left.id.cmp(&right.id))
            })
            .cloned();
        if let Some(existing) = blocked {
            let disposition = if matches!(
                existing.status,
                ReviewProposalStatus::Pending | ReviewProposalStatus::Approved
            ) {
                AutoEvolutionReviewDisposition::Duplicate
            } else {
                AutoEvolutionReviewDisposition::Cooldown
            };
            return Ok(AutoEvolutionReviewBundleResult {
                disposition,
                proposal: existing,
                approval: None,
                events: Vec::new(),
            });
        }
        if inner
            .evolution_review_proposals
            .iter()
            .any(|existing| existing.id == proposal.id)
            || inner
                .approval_requests
                .iter()
                .any(|existing| existing.id == approval.id)
        {
            return Err(ServerError::Conflict(
                "auto evolution review bundle identifiers already exist".to_string(),
            ));
        }

        let record = EvolutionReviewProposalRecord {
            id: proposal.id,
            session_id: proposal.session_id,
            target: proposal.target,
            base_version: proposal.base_version,
            base_digest: proposal.base_digest,
            base_active_digest: proposal.base_active_digest,
            changes: proposal.changes,
            content_digest: proposal.content_digest,
            status: ReviewProposalStatus::Pending,
            apply_contract: proposal.apply_contract,
            auto_candidate: proposal.auto_candidate,
            created_at: approval.created_at,
            updated_at: approval.created_at,
        };
        let first_seq = inner
            .events
            .get(&record.session_id)
            .map_or(1, |events| events.len() as i64 + 1);
        let mut proposal_event = SessionEvent::new(
            record.session_id,
            first_seq,
            "write_proposal",
            proposal_payload_json,
            approval.created_at,
        );
        proposal_event.turn_id = approval.turn_id;
        let mut approval_event = SessionEvent::new(
            record.session_id,
            first_seq + 1,
            "approval",
            approval_payload_json,
            approval.created_at,
        );
        approval_event.turn_id = approval.turn_id;
        let approval_record = ApprovalRequestRecord {
            id: approval.id,
            session_id: approval.session_id,
            turn_id: approval.turn_id,
            requester_id: approval.requester_id,
            origin: approval.origin,
            action: approval.action,
            scope_json: approval.scope_json,
            options_json: approval.options_json,
            status: "pending".to_string(),
            resumable: approval.resumable,
            created_at: approval.created_at,
            expires_at: approval.expires_at,
            heartbeat_at: approval.created_at,
            resolved_at: None,
            selected_option_id: None,
            resolution_json: None,
            request_event_seq: Some(approval_event.seq),
            resolution_event_seq: None,
            resolution_version: 0,
        };
        let effect = ApprovalEffectRecord {
            id: approval.id,
            approval_id: approval.id,
            session_id: approval.session_id,
            effect_type: approval.effect_type,
            payload_json: approval.effect_payload_json,
            status: "blocked".to_string(),
            attempts: 0,
            available_at: approval.expires_at,
            locked_at: None,
            lease_owner: None,
            lease_epoch: 0,
            applied_at: None,
            error_at: None,
            last_error: None,
            created_at: approval.created_at,
            updated_at: approval.created_at,
        };

        #[cfg(test)]
        if inner.fail_auto_evolution_bundle_before_commit_once {
            inner.fail_auto_evolution_bundle_before_commit_once = false;
            return Err(ServerError::Store(
                "injected auto evolution bundle crash".to_string(),
            ));
        }

        inner.evolution_review_proposals.push(record.clone());
        inner.approval_requests.push(approval_record.clone());
        inner.approval_effects.push(effect);
        if let Some(audit) = audit {
            append_evolution_audit_in_memory(&mut inner, audit)?;
        }
        inner
            .events
            .entry(record.session_id)
            .or_default()
            .extend([proposal_event.clone(), approval_event.clone()]);
        if let Some(session) = inner.sessions.get_mut(&record.session_id) {
            session.updated_at = approval.created_at;
        }
        Ok(AutoEvolutionReviewBundleResult {
            disposition: AutoEvolutionReviewDisposition::Created,
            proposal: record,
            approval: Some(approval_record),
            events: vec![proposal_event, approval_event],
        })
    }

    async fn evolution_review_proposal(&self, id: Uuid) -> Result<EvolutionReviewProposalRecord> {
        self.inner
            .lock()
            .evolution_review_proposals
            .iter()
            .find(|proposal| proposal.id == id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("evolution review proposal {id}")))
    }

    async fn evolution_review_proposals_for_session(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<EvolutionReviewProposalRecord>> {
        self.get_session(session_id).await?;
        Ok(self
            .inner
            .lock()
            .evolution_review_proposals
            .iter()
            .filter(|proposal| proposal.session_id == session_id)
            .cloned()
            .collect())
    }

    async fn update_evolution_review_proposal_status(
        &self,
        id: Uuid,
        status: ReviewProposalStatus,
    ) -> Result<EvolutionReviewProposalRecord> {
        let mut inner = self.inner.lock();
        let proposal = inner
            .evolution_review_proposals
            .iter_mut()
            .find(|proposal| proposal.id == id)
            .ok_or_else(|| ServerError::NotFound(format!("evolution review proposal {id}")))?;
        proposal.status = status;
        proposal.updated_at = Utc::now();
        Ok(proposal.clone())
    }

    async fn add_profile_fact(&self, fact: ProfileFactRecord) -> Result<()> {
        validate_profile_fact_persistence(&fact)?;
        self.inner.lock().profile_facts.push(fact);
        Ok(())
    }

    async fn add_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<()> {
        validate_recall_chunk_persistence(&chunk)?;
        self.inner.lock().recall_chunks.push(chunk);
        Ok(())
    }

    async fn upsert_profile_fact(&self, fact: ProfileFactRecord) -> Result<ProfileFactRecord> {
        validate_profile_fact_persistence(&fact)?;
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .profile_facts
            .iter_mut()
            .find(|existing| existing.id == fact.id)
        {
            let mut updated = fact.clone();
            if existing.valid_to.is_some() && updated.valid_to.is_none() {
                updated.valid_to = existing.valid_to;
            }
            *existing = updated;
            return Ok(existing.clone());
        }
        for existing in inner.profile_facts.iter_mut().filter(|existing| {
            existing.subject == fact.subject
                && existing.predicate == fact.predicate
                && existing.object != fact.object
                && existing.valid_to.is_none()
        }) {
            existing.valid_to = Some(fact.valid_from);
        }
        inner.profile_facts.push(fact.clone());
        Ok(fact)
    }

    async fn upsert_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<RecallChunkRecord> {
        validate_recall_chunk_persistence(&chunk)?;
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .recall_chunks
            .iter_mut()
            .find(|existing| existing.id == chunk.id)
        {
            *existing = chunk.clone();
            return Ok(existing.clone());
        }
        inner.recall_chunks.push(chunk.clone());
        Ok(chunk)
    }

    async fn apply_approved_memory_proposal(
        &self,
        proposal: &crate::MemoryWriteProposal,
    ) -> Result<crate::MemoryRecordRef> {
        let scope = match proposal.memory_kind {
            crate::MemoryWriteKind::ProfileFact => "global",
            crate::MemoryWriteKind::RecallChunk => proposal.scope.as_str(),
        };
        let mut inner = self.inner.lock();
        ensure_memory_scope_is_readable(&inner, &proposal.subject, scope)?;
        let mut staged = inner.clone();

        match proposal.memory_kind {
            crate::MemoryWriteKind::ProfileFact => {
                let fact = crate::memory::profile_fact_record(proposal)?;
                validate_profile_fact_persistence(&fact)?;
                let predicate = fact.predicate.as_str();
                let mut predecessors = staged
                    .profile_facts
                    .iter()
                    .filter(|existing| {
                        existing.subject == fact.subject
                            && existing.predicate == predicate
                            && existing.object != fact.object
                            && existing.valid_to.is_none()
                    })
                    .map(|existing| (existing.valid_from, existing.id))
                    .collect::<Vec<_>>();
                for record in staged.memory_records.iter().filter(|record| {
                    matches!(
                        &record.resource,
                        tm_memory::MemoryRecordResource::Semantic(value)
                            if value.owner_subject == fact.subject
                                && value.memory_scope == "global"
                                && value.predicate == predicate
                                && value.object != fact.object
                                && value.status.is_retrievable()
                                && value.effective_to.is_none()
                    )
                }) {
                    predecessors.push((record.resource.observed_at(), record.id()));
                }
                predecessors
                    .sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
                predecessors.dedup_by_key(|item| item.1);
                let existing_forward = staged
                    .memory_records
                    .iter()
                    .find(|record| {
                        record.kind() == MemoryRecordKind::Semantic
                            && record.id() == proposal.record_id
                    })
                    .and_then(|record| record.resource.links().supersedes_record_id);
                let forward = predecessors.first().map(|item| item.1).or(existing_forward);

                for existing in staged.profile_facts.iter_mut().filter(|existing| {
                    existing.subject == fact.subject
                        && existing.predicate == fact.predicate
                        && existing.object != fact.object
                        && existing.valid_to.is_none()
                }) {
                    existing.valid_to = Some(fact.valid_from);
                }
                if let Some(existing) = staged
                    .profile_facts
                    .iter_mut()
                    .find(|existing| existing.id == fact.id)
                {
                    *existing = fact.clone();
                } else {
                    staged.profile_facts.push(fact.clone());
                }

                for (_, predecessor_id) in &predecessors {
                    if let Some(previous) = staged.memory_records.iter_mut().find(|record| {
                        record.kind() == MemoryRecordKind::Semantic
                            && record.id() == *predecessor_id
                    }) {
                        if let tm_memory::MemoryRecordResource::Semantic(value) =
                            &mut previous.resource
                        {
                            value.status = tm_memory::MemoryRecordStatus::Superseded;
                            value.effective_to = Some(proposal.created_at);
                            value.links.superseded_by_record_id = Some(proposal.record_id);
                        }
                        *previous = StoredMemoryRecord::new(previous.resource.clone())
                            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
                    }
                }
                let typed = crate::memory::typed_memory_record(proposal, forward)?;
                if let Some(existing) = staged
                    .memory_records
                    .iter_mut()
                    .find(|record| record.kind() == typed.kind() && record.id() == typed.id())
                {
                    *existing = typed;
                } else {
                    staged.memory_records.push(typed);
                }
            }
            crate::MemoryWriteKind::RecallChunk => {
                let chunk = crate::memory::recall_chunk_record(proposal)?;
                validate_recall_chunk_persistence(&chunk)?;
                if let Some(existing) = staged
                    .recall_chunks
                    .iter_mut()
                    .find(|existing| existing.id == chunk.id)
                {
                    *existing = chunk;
                } else {
                    staged.recall_chunks.push(chunk);
                }
                let typed = crate::memory::typed_memory_record(proposal, None)?;
                if let Some(existing) = staged
                    .memory_records
                    .iter_mut()
                    .find(|record| record.kind() == typed.kind() && record.id() == typed.id())
                {
                    *existing = typed;
                } else {
                    staged.memory_records.push(typed);
                }
            }
        }

        *inner = staged;
        Ok(proposal.record_ref())
    }

    async fn profile_facts(&self, subject: &str) -> Result<Vec<ProfileFactRecord>> {
        let mut facts = self
            .inner
            .lock()
            .profile_facts
            .iter()
            .filter(|fact| fact.subject == subject && fact.valid_to.is_none())
            .cloned()
            .collect::<Vec<_>>();
        facts.sort_by(|left, right| {
            right
                .importance
                .total_cmp(&left.importance)
                .then_with(|| right.valid_from.cmp(&left.valid_from))
        });
        Ok(facts)
    }

    async fn profile_fact(&self, subject: &str, id: Uuid) -> Result<ProfileFactRecord> {
        self.inner
            .lock()
            .profile_facts
            .iter()
            .find(|fact| fact.subject == subject && fact.id == id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("profile fact {subject}/{id}")))
    }

    async fn recall_chunks(
        &self,
        scope: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecallChunkRecord>> {
        let query = query.to_lowercase();
        let mut chunks = self
            .inner
            .lock()
            .recall_chunks
            .iter()
            .filter(|chunk| chunk.scope == scope && chunk.text.to_lowercase().contains(&query))
            .cloned()
            .collect::<Vec<_>>();
        chunks.sort_by(|left, right| {
            right
                .importance
                .total_cmp(&left.importance)
                .then_with(|| right.created_at.cmp(&left.created_at))
        });
        chunks.truncate(limit);
        Ok(chunks)
    }

    async fn recall_chunk(&self, scope: &str, id: Uuid) -> Result<RecallChunkRecord> {
        self.inner
            .lock()
            .recall_chunks
            .iter()
            .find(|chunk| chunk.scope == scope && chunk.id == id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("recall chunk {scope}/{id}")))
    }

    async fn upsert_memory_record(&self, record: StoredMemoryRecord) -> Result<StoredMemoryRecord> {
        let record = sanitize_durable_memory_record(record)?;
        let mut inner = self.inner.lock();
        ensure_memory_scope_is_readable(
            &inner,
            record.resource.owner_subject(),
            record.resource.memory_scope(),
        )?;
        ensure_memory_record_links_are_scoped(&inner, &record)?;
        if let Some(existing) = inner
            .memory_records
            .iter_mut()
            .find(|existing| existing.kind() == record.kind() && existing.id() == record.id())
        {
            *existing = record.clone();
            return Ok(existing.clone());
        }
        if let Some(existing) = inner.memory_records.iter().find(|existing| {
            existing.kind() == record.kind()
                && existing.resource.owner_subject() == record.resource.owner_subject()
                && existing.resource.memory_scope() == record.resource.memory_scope()
                && existing.content_key == record.content_key
                && existing.resource.status().is_retrievable()
                && existing.resource.effective_to().is_none()
        }) {
            return Ok(existing.clone());
        }
        inner.memory_records.push(record.clone());
        Ok(record)
    }

    async fn memory_record(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        kind: MemoryRecordKind,
        id: Uuid,
    ) -> Result<StoredMemoryRecord> {
        let inner = self.inner.lock();
        ensure_memory_scope_is_readable(&inner, owner_subject, memory_scope)?;
        inner
            .memory_records
            .iter()
            .find(|record| {
                record.kind() == kind
                    && record.id() == id
                    && record.resource.owner_subject() == owner_subject
                    && record.resource.memory_scope() == memory_scope
            })
            .cloned()
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "memory record {owner_subject}/{memory_scope}/{}/{}",
                    kind.as_str(),
                    id
                ))
            })
    }

    async fn active_memory_records(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        limit: usize,
    ) -> Result<Vec<StoredMemoryRecord>> {
        let inner = self.inner.lock();
        ensure_memory_scope_is_readable(&inner, owner_subject, memory_scope)?;
        let mut records = inner
            .memory_records
            .iter()
            .filter(|record| {
                record.resource.owner_subject() == owner_subject
                    && record.resource.memory_scope() == memory_scope
                    && memory_record_is_retrievable(&inner, record)
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            right
                .resource
                .importance()
                .total_cmp(&left.resource.importance())
                .then_with(|| {
                    right
                        .resource
                        .observed_at()
                        .cmp(&left.resource.observed_at())
                })
                .then_with(|| left.kind().cmp(&right.kind()))
                .then_with(|| left.id().cmp(&right.id()))
        });
        records.truncate(limit);
        Ok(records)
    }

    async fn enqueue_memory_embedding_job(
        &self,
        job: NewMemoryEmbeddingJob,
    ) -> Result<MemoryEmbeddingJobRecord> {
        let job = sanitize_new_memory_embedding_job(job)?;
        let reembedding_key = job.reembedding_key();
        let mut inner = self.inner.lock();
        ensure_memory_scope_is_readable(&inner, &job.owner_subject, &job.memory_scope)?;
        let record = inner
            .memory_records
            .iter()
            .find(|record| record.kind() == job.record_kind && record.id() == job.record_id)
            .ok_or_else(|| ServerError::NotFound(format!("memory record {}", job.record_id)))?;
        if record.resource.owner_subject() != job.owner_subject
            || record.resource.memory_scope() != job.memory_scope
            || record.content_key != job.content_key
        {
            return Err(ServerError::NotFound(format!(
                "memory record {}/{} in requested authority",
                job.record_kind.as_str(),
                job.record_id
            )));
        }
        if let Some(existing) = inner
            .memory_embedding_jobs
            .iter_mut()
            .find(|existing| existing.reembedding_key == reembedding_key)
        {
            let recoverable = existing.status == MemoryEmbeddingJobStatus::Completed
                || (existing.status == MemoryEmbeddingJobStatus::Failed
                    && (existing.failure_code.as_deref() != Some("input_too_large")
                        || job.input_limit_bytes > existing.input_limit_bytes));
            if recoverable {
                existing.status = MemoryEmbeddingJobStatus::Queued;
                existing.available_at = job.created_at;
                existing.failure_code = None;
                existing.input_limit_bytes = existing.input_limit_bytes.max(job.input_limit_bytes);
                existing.updated_at = job.created_at;
            }
            return Ok(existing.clone());
        }
        let record = MemoryEmbeddingJobRecord {
            id: job.id,
            record_kind: job.record_kind,
            record_id: job.record_id,
            owner_subject: job.owner_subject,
            memory_scope: job.memory_scope,
            content_key: job.content_key,
            provenance: job.provenance,
            reembedding_key,
            status: MemoryEmbeddingJobStatus::Queued,
            input_limit_bytes: job.input_limit_bytes,
            failure_code: None,
            attempts: 0,
            available_at: job.created_at,
            locked_at: None,
            lease_owner: None,
            lease_epoch: 0,
            cancelled_at: None,
            created_at: job.created_at,
            updated_at: job.created_at,
        };
        inner.memory_embedding_jobs.push(record.clone());
        Ok(record)
    }

    async fn memory_embedding_jobs(
        &self,
        owner_subject: &str,
        memory_scope: &str,
    ) -> Result<Vec<MemoryEmbeddingJobRecord>> {
        let inner = self.inner.lock();
        ensure_memory_scope_is_readable(&inner, owner_subject, memory_scope)?;
        let mut jobs = inner
            .memory_embedding_jobs
            .iter()
            .filter(|job| job.owner_subject == owner_subject && job.memory_scope == memory_scope)
            .cloned()
            .collect::<Vec<_>>();
        jobs.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(jobs)
    }

    async fn revoke_memory_scope(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        reason: &str,
    ) -> Result<MemoryScopeTombstone> {
        validate_persistence_identifier("memory tombstone owner subject", owner_subject)?;
        validate_persistence_identifier("memory tombstone scope", memory_scope)?;
        if memory_scope
            .strip_prefix("project:")
            .is_none_or(|slug| slug.trim().is_empty())
        {
            return Err(ServerError::InvalidRequest(
                "memory tombstones only apply to project scopes".to_string(),
            ));
        }
        let reason = redact_persisted_text(reason);
        if reason.trim().is_empty() {
            return Err(ServerError::InvalidRequest(
                "memory tombstone reason must not be empty".to_string(),
            ));
        }
        let now = Utc::now();
        let mut inner = self.inner.lock();
        let tombstone =
            if let Some(existing) = inner.memory_scope_tombstones.iter_mut().find(|item| {
                item.owner_subject == owner_subject && item.memory_scope == memory_scope
            }) {
                existing.reason = reason;
                existing.clone()
            } else {
                let tombstone = MemoryScopeTombstone {
                    owner_subject: owner_subject.to_string(),
                    memory_scope: memory_scope.to_string(),
                    link_alias: None,
                    reason,
                    revoked_at: now,
                };
                inner.memory_scope_tombstones.push(tombstone.clone());
                tombstone
            };
        for job in inner.memory_embedding_jobs.iter_mut().filter(|job| {
            job.owner_subject == owner_subject
                && job.memory_scope == memory_scope
                && matches!(
                    job.status,
                    MemoryEmbeddingJobStatus::Queued | MemoryEmbeddingJobStatus::Running
                )
        }) {
            job.status = MemoryEmbeddingJobStatus::Cancelled;
            job.cancelled_at = Some(now);
            job.locked_at = None;
            job.updated_at = now;
        }
        Ok(tombstone)
    }

    async fn memory_scope_tombstone(
        &self,
        owner_subject: &str,
        memory_scope: &str,
    ) -> Result<Option<MemoryScopeTombstone>> {
        Ok(self
            .inner
            .lock()
            .memory_scope_tombstones
            .iter()
            .find(|item| item.owner_subject == owner_subject && item.memory_scope == memory_scope)
            .cloned())
    }

    async fn upsert_project_item(&self, item: NewProjectItem) -> Result<ProjectItemRecord> {
        let item = sanitize_project_item_persistence(item)?;
        self.get_session(item.source_session_id).await?;
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.project_items.iter_mut().find(|existing| {
            existing.project_id == item.project_id
                && existing.kind == item.kind
                && existing.dedupe_key == item.dedupe_key
        }) {
            existing.text = item.text;
            existing.target_uri = item.target_uri;
            existing.source_event_seq = item.source_event_seq;
            existing.source_uri = item.source_uri;
            existing.provenance_json = item.provenance_json;
            return Ok(existing.clone());
        }
        let record = ProjectItemRecord {
            id: Uuid::new_v4(),
            project_id: item.project_id,
            kind: item.kind,
            text: item.text,
            target_uri: item.target_uri,
            source_session_id: item.source_session_id,
            source_event_seq: item.source_event_seq,
            source_uri: item.source_uri,
            dedupe_key: item.dedupe_key,
            provenance_json: item.provenance_json,
            created_at: Utc::now(),
        };
        inner.project_items.push(record.clone());
        Ok(record)
    }

    async fn project_items(
        &self,
        project_id: &str,
        kind: Option<ProjectItemKind>,
    ) -> Result<Vec<ProjectItemRecord>> {
        let mut items = self
            .inner
            .lock()
            .project_items
            .iter()
            .filter(|item| item.project_id == project_id)
            .filter(|item| kind.is_none_or(|kind| item.kind == kind))
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| item.created_at);
        Ok(items)
    }

    async fn ensure_project(
        &self,
        id: &str,
        title: &str,
        default_memory_policy: MemoryPolicy,
    ) -> Result<ProjectRecord> {
        validate_persistence_identifier("project id", id)?;
        let title = title.trim();
        let title = if title.is_empty() { id } else { title };
        let now = Utc::now();
        let mut inner = self.inner.lock();
        let record = inner
            .projects
            .entry(id.to_string())
            .or_insert_with(|| ProjectRecord {
                id: id.to_string(),
                title: title.to_string(),
                status: ProjectStatus::Active,
                created_at: now,
                updated_at: now,
                archived_at: None,
                default_memory_policy,
            });
        Ok(record.clone())
    }

    async fn project(&self, id: &str) -> Result<Option<ProjectRecord>> {
        Ok(self.inner.lock().projects.get(id).cloned())
    }

    async fn projects(&self, include_archived: bool) -> Result<Vec<ProjectRecord>> {
        let mut projects = self
            .inner
            .lock()
            .projects
            .values()
            .filter(|project| include_archived || project.status == ProjectStatus::Active)
            .cloned()
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(projects)
    }

    async fn archive_project(
        &self,
        owner_subject: &str,
        id: &str,
        reason: &str,
    ) -> Result<ProjectRecord> {
        validate_persistence_identifier("project id", id)?;
        let scope = format!("project:{id}");
        // Serialize the archive with the durable scope tombstone (§30.4): revoke first so a failure
        // leaves the project active, then flip status.
        self.revoke_memory_scope(owner_subject, &scope, reason)
            .await?;
        let now = Utc::now();
        let mut inner = self.inner.lock();
        let project = inner
            .projects
            .get_mut(id)
            .ok_or_else(|| ServerError::NotFound(format!("project {id}")))?;
        project.status = ProjectStatus::Archived;
        project.archived_at = Some(now);
        project.updated_at = now;
        Ok(project.clone())
    }

    async fn enqueue_dream(&self, new: NewDreamQueueRecord) -> Result<DreamQueueRecord> {
        validate_persistence_identifier("dream subject", &new.subject)?;
        validate_persistence_identifier("dream scope", &new.scope)?;
        validate_persistence_identifier("dream dedupe key", &new.dedupe_key)?;
        self.get_session(new.session_id).await?;
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .dream_queue
            .iter_mut()
            .find(|record| record.dedupe_key == new.dedupe_key)
        {
            if existing.source_event_seq.is_none() {
                existing.source_event_seq = new.source_event_seq;
            }
            return Ok(existing.clone());
        }
        let now = Utc::now();
        let record = DreamQueueRecord {
            id: Uuid::new_v4(),
            session_id: new.session_id,
            subject: new.subject,
            scope: new.scope,
            reason: new.reason,
            status: DreamStatus::Queued,
            dedupe_key: new.dedupe_key,
            source_event_seq: new.source_event_seq,
            attempts: 0,
            enqueued_at: now,
            available_at: new.available_at,
            locked_at: None,
            lease_owner: None,
            lease_epoch: 0,
            heartbeat_at: None,
            completed_at: None,
            error_at: None,
            last_error: None,
        };
        inner.dream_queue.push(record.clone());
        Ok(record)
    }

    async fn dream_queue_for_session(&self, session_id: Uuid) -> Result<Vec<DreamQueueRecord>> {
        self.get_session(session_id).await?;
        let mut dreams = self
            .inner
            .lock()
            .dream_queue
            .iter()
            .filter(|record| record.session_id == session_id)
            .cloned()
            .collect::<Vec<_>>();
        dreams.sort_by_key(|record| record.enqueued_at);
        Ok(dreams)
    }

    async fn dream_queue(&self, scope: &str, limit: usize) -> Result<Vec<DreamQueueRecord>> {
        let mut dreams = self
            .inner
            .lock()
            .dream_queue
            .iter()
            .filter(|record| record.scope == scope)
            .cloned()
            .collect::<Vec<_>>();
        dreams.sort_by_key(|record| std::cmp::Reverse(record.enqueued_at));
        dreams.truncate(limit);
        Ok(dreams)
    }

    async fn dream(&self, dream_id: Uuid) -> Result<DreamQueueRecord> {
        self.inner
            .lock()
            .dream_queue
            .iter()
            .find(|record| record.id == dream_id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("dream {dream_id}")))
    }

    async fn claim_ready_dream(
        &self,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        owner_id: Uuid,
    ) -> Result<Option<DreamLease>> {
        self.claim_ready_dream_bounded(now, lease_timeout, owner_id, 3)
            .await
    }

    async fn claim_ready_dream_bounded(
        &self,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        owner_id: Uuid,
        max_attempts: i32,
    ) -> Result<Option<DreamLease>> {
        if max_attempts <= 0 {
            return Err(ServerError::InvalidRequest(
                "dream max_attempts must be positive".to_string(),
            ));
        }
        let stale_before = now - lease_timeout;
        let mut inner = self.inner.lock();
        for record in &mut inner.dream_queue {
            let ready = (record.status == DreamStatus::Queued && record.available_at <= now)
                || (record.status == DreamStatus::Running
                    && record
                        .locked_at
                        .is_some_and(|locked_at| locked_at <= stale_before));
            if ready && record.attempts >= max_attempts {
                record.status = DreamStatus::Failed;
                record.locked_at = None;
                record.lease_owner = None;
                record.completed_at = None;
                record.error_at = Some(now);
                record.last_error =
                    Some("dream lease expired after the final permitted attempt".to_string());
            }
        }
        let Some(index) = inner
            .dream_queue
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                record.attempts < max_attempts
                    && ((record.status == DreamStatus::Queued && record.available_at <= now)
                        || (record.status == DreamStatus::Running
                            && record
                                .locked_at
                                .is_some_and(|locked_at| locked_at <= stale_before)))
            })
            .min_by(|(_, left), (_, right)| {
                left.available_at
                    .cmp(&right.available_at)
                    .then_with(|| left.enqueued_at.cmp(&right.enqueued_at))
            })
            .map(|(index, _)| index)
        else {
            return Ok(None);
        };
        let record = &mut inner.dream_queue[index];
        record.status = DreamStatus::Running;
        record.attempts += 1;
        record.locked_at = Some(now);
        record.lease_owner = Some(owner_id);
        record.lease_epoch += 1;
        record.heartbeat_at = Some(now);
        record.completed_at = None;
        record.error_at = None;
        record.last_error = None;
        Ok(Some(DreamLease::new(
            record.clone(),
            owner_id,
            record.lease_epoch,
        )))
    }

    async fn heartbeat_dream(&self, lease: &DreamLease, now: DateTime<Utc>) -> Result<DreamLease> {
        let mut inner = self.inner.lock();
        let record = inner
            .dream_queue
            .iter_mut()
            .find(|record| {
                record.id == lease.dream.id
                    && record.status == DreamStatus::Running
                    && record.lease_owner == Some(lease.owner_id)
                    && record.lease_epoch == lease.epoch
            })
            .ok_or_else(|| stale_dream_lease(lease))?;
        record.locked_at = Some(now);
        record.heartbeat_at = Some(now);
        Ok(DreamLease::new(record.clone(), lease.owner_id, lease.epoch))
    }

    async fn complete_dream(
        &self,
        lease: &DreamLease,
        now: DateTime<Utc>,
    ) -> Result<DreamQueueRecord> {
        let mut inner = self.inner.lock();
        let record = inner
            .dream_queue
            .iter_mut()
            .find(|record| {
                record.id == lease.dream.id
                    && record.status == DreamStatus::Running
                    && record.lease_owner == Some(lease.owner_id)
                    && record.lease_epoch == lease.epoch
            })
            .ok_or_else(|| stale_dream_lease(lease))?;
        record.status = DreamStatus::Completed;
        record.available_at = now;
        record.locked_at = None;
        record.lease_owner = None;
        record.completed_at = Some(now);
        record.error_at = None;
        record.last_error = None;
        Ok(record.clone())
    }

    async fn fail_dream(
        &self,
        lease: &DreamLease,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<DreamQueueRecord> {
        let error = redact_persisted_text(&error);
        let mut inner = self.inner.lock();
        let record = inner
            .dream_queue
            .iter_mut()
            .find(|record| {
                record.id == lease.dream.id
                    && record.status == DreamStatus::Running
                    && record.lease_owner == Some(lease.owner_id)
                    && record.lease_epoch == lease.epoch
            })
            .ok_or_else(|| stale_dream_lease(lease))?;
        record.status = if record.attempts >= max_attempts {
            DreamStatus::Failed
        } else {
            DreamStatus::Queued
        };
        record.available_at = next_available_at;
        record.locked_at = None;
        record.lease_owner = None;
        record.completed_at = None;
        record.error_at = Some(Utc::now());
        record.last_error = Some(error);
        Ok(record.clone())
    }

    async fn upsert_memory_summary(
        &self,
        summary: NewMemorySummaryRecord,
    ) -> Result<MemorySummaryRecord> {
        let summary = sanitize_memory_summary_persistence(summary)?;
        if let Some(session_id) = summary.source_session_id {
            self.get_session(session_id).await?;
        }
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .memory_summaries
            .iter_mut()
            .find(|existing| existing.dedupe_key == summary.dedupe_key)
        {
            existing.kind = summary.kind;
            existing.subject = summary.subject;
            existing.scope = summary.scope;
            existing.title = summary.title;
            existing.body = summary.body;
            existing.evidence = summary.evidence;
            existing.source_dream_id = summary.source_dream_id;
            existing.source_session_id = summary.source_session_id;
            existing.updated_at = Utc::now();
            return Ok(existing.clone());
        }
        let now = Utc::now();
        let record = MemorySummaryRecord {
            id: Uuid::new_v4(),
            kind: summary.kind,
            subject: summary.subject,
            scope: summary.scope,
            title: summary.title,
            body: summary.body,
            evidence: summary.evidence,
            source_dream_id: summary.source_dream_id,
            source_session_id: summary.source_session_id,
            dedupe_key: summary.dedupe_key,
            created_at: now,
            updated_at: now,
        };
        inner.memory_summaries.push(record.clone());
        Ok(record)
    }

    async fn memory_summary(&self, id: Uuid) -> Result<MemorySummaryRecord> {
        self.inner
            .lock()
            .memory_summaries
            .iter()
            .find(|summary| summary.id == id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("memory summary {id}")))
    }

    async fn memory_summaries(
        &self,
        scope: &str,
        limit: usize,
    ) -> Result<Vec<MemorySummaryRecord>> {
        let mut summaries = self
            .inner
            .lock()
            .memory_summaries
            .iter()
            .filter(|summary| summary.scope == scope)
            .cloned()
            .collect::<Vec<_>>();
        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.updated_at));
        summaries.truncate(limit);
        Ok(summaries)
    }
    async fn upsert_evolution_episode(
        &self,
        new: NewEvolutionEpisodeRecord,
    ) -> Result<(EvolutionEpisodeRecord, bool)> {
        let new = sanitize_evolution_episode_persistence(new)?;
        self.get_session(new.session_id).await?;
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .evolution_episodes
            .iter()
            .find(|episode| episode.turn_id == new.turn_id)
        {
            return Ok((existing.clone(), false));
        }
        let now = Utc::now();
        let record = EvolutionEpisodeRecord {
            id: Uuid::new_v4(),
            session_id: new.session_id,
            turn_id: new.turn_id,
            owner_subject: new.owner_subject,
            memory_scope: new.memory_scope,
            status: tm_memory::EpisodeStatus::Captured,
            terminal_reward: None,
            reward_source: None,
            feedback_outcome: None,
            trace_count: 0,
            created_at: now,
            updated_at: now,
        };
        inner.evolution_episodes.push(record.clone());
        Ok((record, true))
    }

    async fn evolution_episode_for_turn(
        &self,
        turn_id: Uuid,
    ) -> Result<Option<EvolutionEpisodeRecord>> {
        Ok(self
            .inner
            .lock()
            .evolution_episodes
            .iter()
            .find(|episode| episode.turn_id == turn_id)
            .cloned())
    }

    async fn evolution_episodes(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        limit: usize,
    ) -> Result<Vec<EvolutionEpisodeRecord>> {
        let mut episodes = self
            .inner
            .lock()
            .evolution_episodes
            .iter()
            .filter(|episode| {
                episode.owner_subject == owner_subject && episode.memory_scope == memory_scope
            })
            .cloned()
            .collect::<Vec<_>>();
        episodes.sort_by_key(|episode| std::cmp::Reverse(episode.updated_at));
        episodes.truncate(limit);
        Ok(episodes)
    }

    async fn evolution_episode(&self, id: Uuid) -> Result<EvolutionEpisodeRecord> {
        self.inner
            .lock()
            .evolution_episodes
            .iter()
            .find(|episode| episode.id == id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("evolution episode {id}")))
    }

    async fn replace_experience_traces(
        &self,
        episode_id: Uuid,
        traces: Vec<NewExperienceTraceRecord>,
    ) -> Result<Vec<ExperienceTraceRecord>> {
        let traces = traces
            .into_iter()
            .map(sanitize_experience_trace_persistence)
            .collect::<Result<Vec<_>>>()?;
        validate_experience_trace_replacement(episode_id, &traces)?;
        let mut inner = self.inner.lock();
        let episode = inner
            .evolution_episodes
            .iter_mut()
            .find(|episode| episode.id == episode_id)
            .ok_or_else(|| ServerError::NotFound(format!("evolution episode {episode_id}")))?;
        episode.trace_count = u32::try_from(traces.len()).unwrap_or(u32::MAX);
        episode.updated_at = Utc::now();
        inner
            .experience_traces
            .retain(|trace| trace.episode_id != episode_id);
        let now = Utc::now();
        let records = traces
            .into_iter()
            .map(|trace| ExperienceTraceRecord {
                id: Uuid::new_v4(),
                episode_id,
                ordinal: trace.ordinal,
                kind: trace.kind,
                capability: trace.capability,
                action_summary: trace.action_summary,
                observation_summary: trace.observation_summary,
                error_signature: trace.error_signature,
                value: None,
                event_seq: trace.event_seq,
                result_event_seq: trace.result_event_seq,
                created_at: now,
            })
            .collect::<Vec<_>>();
        inner.experience_traces.extend(records.iter().cloned());
        Ok(records)
    }

    async fn experience_traces(&self, episode_id: Uuid) -> Result<Vec<ExperienceTraceRecord>> {
        let mut traces = self
            .inner
            .lock()
            .experience_traces
            .iter()
            .filter(|trace| trace.episode_id == episode_id)
            .cloned()
            .collect::<Vec<_>>();
        traces.sort_by_key(|trace| trace.ordinal);
        Ok(traces)
    }

    async fn set_episode_valuation(
        &self,
        episode_id: Uuid,
        terminal_reward: f32,
        reward_source: tm_memory::RewardSource,
        feedback_outcome: Option<FeedbackOutcome>,
        trace_values: &[(Uuid, f32)],
        skill_outcomes: &[(String, String, bool)],
        status: EpisodeStatus,
    ) -> Result<EvolutionEpisodeRecord> {
        if !terminal_reward.is_finite() || trace_values.iter().any(|(_, value)| !value.is_finite())
        {
            return Err(ServerError::InvalidRequest(
                "evolution values must be finite".to_string(),
            ));
        }
        let mut inner = self.inner.lock();
        let current = inner
            .evolution_episodes
            .iter()
            .find(|episode| episode.id == episode_id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("evolution episode {episode_id}")))?;
        let expected_trace_count = inner
            .experience_traces
            .iter()
            .filter(|trace| trace.episode_id == episode_id)
            .count();
        if expected_trace_count != trace_values.len() {
            return Err(ServerError::InvalidRequest(
                "episode valuation must cover every trace exactly once".to_string(),
            ));
        }
        let trace_values_match = trace_values.iter().all(|(trace_id, value)| {
            inner.experience_traces.iter().any(|trace| {
                trace.id == *trace_id
                    && trace.episode_id == episode_id
                    && trace.value == Some(*value)
            })
        });
        let stored_outcome_count = inner
            .skill_outcomes
            .iter()
            .filter(|(stored_episode_id, _, _, _)| *stored_episode_id == episode_id)
            .count();
        let outcomes_match = stored_outcome_count == skill_outcomes.len()
            && skill_outcomes.iter().all(|(name, digest, pass)| {
                inner
                    .skill_outcomes
                    .contains(&(episode_id, name.clone(), digest.clone(), *pass))
            });
        if current.status == status
            && current.terminal_reward == Some(terminal_reward)
            && current.reward_source == Some(reward_source)
            && current.feedback_outcome == feedback_outcome
            && trace_values_match
            && outcomes_match
        {
            return Ok(current);
        }
        if current.status != EpisodeStatus::Captured {
            return Err(ServerError::Conflict(format!(
                "episode {episode_id} is already terminally valued"
            )));
        }
        #[cfg(test)]
        if std::mem::take(&mut inner.fail_set_episode_valuation_once) {
            return Err(ServerError::Store(
                "simulated episode valuation failure /tmp/private/123".to_string(),
            ));
        }
        for (trace_id, _) in trace_values {
            if !inner
                .experience_traces
                .iter()
                .any(|trace| trace.id == *trace_id && trace.episode_id == episode_id)
            {
                return Err(ServerError::NotFound(format!(
                    "experience trace {trace_id} in episode {episode_id}"
                )));
            }
        }
        for (trace_id, value) in trace_values {
            let trace = inner
                .experience_traces
                .iter_mut()
                .find(|trace| trace.id == *trace_id)
                .expect("trace existence checked before valuation");
            trace.value = Some(*value);
        }
        for (name, digest, pass) in skill_outcomes {
            if inner
                .skill_outcomes
                .insert((episode_id, name.clone(), digest.clone(), *pass))
            {
                let stats = inner
                    .skill_runtime_stats
                    .entry((name.clone(), digest.clone()))
                    .or_default();
                if *pass {
                    stats.1 = stats.1.saturating_add(1);
                } else {
                    stats.2 = stats.2.saturating_add(1);
                }
            }
        }
        let episode = inner
            .evolution_episodes
            .iter_mut()
            .find(|episode| episode.id == episode_id)
            .expect("episode existence checked before valuation");
        episode.terminal_reward = Some(terminal_reward);
        episode.reward_source = Some(reward_source);
        episode.feedback_outcome = feedback_outcome;
        episode.status = status;
        episode.updated_at = Utc::now();
        Ok(episode.clone())
    }

    async fn upsert_evolution_policy(
        &self,
        mut policy: EvolutionPolicyRecord,
    ) -> Result<EvolutionPolicyRecord> {
        validate_evolution_policy(&policy)?;
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.evolution_policies.iter_mut().find(|existing| {
            existing.owner_subject == policy.owner_subject
                && existing.memory_scope == policy.memory_scope
                && existing.signature == policy.signature
        }) {
            let content_changed = existing.trigger != policy.trigger
                || existing.procedure != policy.procedure
                || existing.verification != policy.verification
                || existing.boundary != policy.boundary;
            policy.id = existing.id;
            policy.created_at = existing.created_at;
            policy.version = if content_changed {
                existing.version.saturating_add(1)
            } else {
                existing.version
            };
            policy.updated_at = Utc::now();
            *existing = policy.clone();
            return Ok(policy);
        }
        if policy.version == 0 {
            policy.version = 1;
        }
        inner.evolution_policies.push(policy.clone());
        Ok(policy)
    }

    async fn evolution_policy(&self, id: Uuid) -> Result<EvolutionPolicyRecord> {
        self.inner
            .lock()
            .evolution_policies
            .iter()
            .find(|policy| policy.id == id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("evolution policy {id}")))
    }

    async fn evolution_policies(
        &self,
        owner_subject: &str,
        memory_scope: &str,
        status: Option<PolicyStatus>,
        limit: usize,
    ) -> Result<Vec<EvolutionPolicyRecord>> {
        let mut policies = self
            .inner
            .lock()
            .evolution_policies
            .iter()
            .filter(|policy| {
                policy.owner_subject == owner_subject
                    && policy.memory_scope == memory_scope
                    && status.is_none_or(|status| policy.status == status)
            })
            .cloned()
            .collect::<Vec<_>>();
        policies.sort_by_key(|policy| std::cmp::Reverse(policy.updated_at));
        policies.truncate(limit);
        Ok(policies)
    }

    async fn link_policy_traces(
        &self,
        policy_id: Uuid,
        links: &[(Uuid, Uuid, f32, bool)],
    ) -> Result<()> {
        if links.iter().any(|(_, _, value, _)| !value.is_finite()) {
            return Err(ServerError::InvalidRequest(
                "policy trace values must be finite".to_string(),
            ));
        }
        let mut inner = self.inner.lock();
        if !inner
            .evolution_policies
            .iter()
            .any(|policy| policy.id == policy_id)
        {
            return Err(ServerError::NotFound(format!(
                "evolution policy {policy_id}"
            )));
        }
        for (trace_id, episode_id, _, _) in links {
            if !inner.experience_traces.iter().any(|trace| {
                trace.id == *trace_id && trace.episode_id == *episode_id && trace.value.is_some()
            }) {
                return Err(ServerError::NotFound(format!(
                    "valued experience trace {trace_id} in episode {episode_id}"
                )));
            }
        }
        for (trace_id, episode_id, value, positive) in links {
            if !inner
                .policy_trace_links
                .iter()
                .any(|(existing_policy, existing_trace, _, _, _)| {
                    *existing_policy == policy_id && *existing_trace == *trace_id
                })
            {
                inner.policy_trace_links.push((
                    policy_id,
                    *trace_id,
                    *episode_id,
                    *value,
                    *positive,
                ));
            }
        }
        Ok(())
    }

    async fn upsert_environment_cognition(
        &self,
        cognition: EnvironmentCognitionRecord,
    ) -> Result<EnvironmentCognitionRecord> {
        let mut cognition = sanitize_environment_cognition_persistence(cognition)?;
        let mut inner = self.inner.lock();
        #[cfg(test)]
        if std::mem::take(&mut inner.fail_upsert_environment_cognition_once) {
            return Err(ServerError::Store(
                "simulated environment cognition persistence failure /tmp/private/123".to_string(),
            ));
        }
        if let Some(existing) = inner.environment_cognitions.iter_mut().find(|existing| {
            existing.owner_subject == cognition.owner_subject
                && existing.memory_scope == cognition.memory_scope
        }) {
            let body_changed = existing.body != cognition.body;
            cognition.id = existing.id;
            cognition.created_at = existing.created_at;
            cognition.version = if body_changed {
                existing.version.saturating_add(1)
            } else {
                existing.version
            };
            cognition.updated_at = Utc::now();
            *existing = cognition.clone();
            return Ok(cognition);
        }
        if cognition.version == 0 {
            cognition.version = 1;
        }
        inner.environment_cognitions.push(cognition.clone());
        Ok(cognition)
    }

    async fn environment_cognition(
        &self,
        owner_subject: &str,
        memory_scope: &str,
    ) -> Result<Option<EnvironmentCognitionRecord>> {
        Ok(self
            .inner
            .lock()
            .environment_cognitions
            .iter()
            .find(|cognition| {
                cognition.owner_subject == owner_subject && cognition.memory_scope == memory_scope
            })
            .cloned())
    }

    async fn policy_trace_values(&self, policy_id: Uuid) -> Result<Vec<(Uuid, Uuid, f32, bool)>> {
        if !self
            .inner
            .lock()
            .evolution_policies
            .iter()
            .any(|policy| policy.id == policy_id)
        {
            return Err(ServerError::NotFound(format!(
                "evolution policy {policy_id}"
            )));
        }
        let mut links = self
            .inner
            .lock()
            .policy_trace_links
            .iter()
            .filter(|(existing_policy, _, _, _, _)| *existing_policy == policy_id)
            .map(|(_, trace_id, episode_id, value, positive)| {
                (*trace_id, *episode_id, *value, *positive)
            })
            .collect::<Vec<_>>();
        links.sort_by_key(|(_, episode_id, _, _)| *episode_id);
        Ok(links)
    }

    async fn record_turn_feedback(
        &self,
        session_id: Uuid,
        turn_id: Uuid,
        outcome: FeedbackOutcome,
        comment: Option<&str>,
    ) -> Result<bool> {
        self.get_session(session_id).await?;
        let comment = sanitize_turn_feedback_comment(comment);
        let mut inner = self.inner.lock();
        if let Some((existing_session_id, existing_outcome, _)) = inner.turn_feedback.get(&turn_id)
        {
            if *existing_session_id == session_id && *existing_outcome == outcome {
                return Ok(false);
            }
            return Err(ServerError::Conflict(
                "turn feedback already recorded".to_string(),
            ));
        }
        inner
            .turn_feedback
            .insert(turn_id, (session_id, outcome, comment));
        Ok(true)
    }

    async fn turn_feedback(
        &self,
        turn_id: Uuid,
    ) -> Result<Option<(FeedbackOutcome, Option<String>)>> {
        Ok(self
            .inner
            .lock()
            .turn_feedback
            .get(&turn_id)
            .map(|(_, outcome, comment)| (*outcome, comment.clone())))
    }

    async fn upsert_skill_proposal(
        &self,
        proposal: NewSkillProposalRecord,
    ) -> Result<SkillProposalRecord> {
        let proposal = sanitize_skill_proposal_persistence(proposal)?;
        self.get_session(proposal.source_session_id).await?;
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .skill_proposals
            .iter_mut()
            .find(|existing| existing.dedupe_key == proposal.dedupe_key)
        {
            if existing.status != SkillProposalStatus::Pending {
                let same_revision = existing.name == proposal.name
                    && existing.description == proposal.description
                    && existing.body == proposal.body
                    && existing.trigger == proposal.trigger
                    && existing.use_criteria == proposal.use_criteria
                    && existing.evidence == proposal.evidence
                    && existing.self_critique == proposal.self_critique
                    && existing.verification == proposal.verification
                    && existing.source_dream_id == proposal.source_dream_id
                    && existing.source_session_id == proposal.source_session_id
                    && existing.source_policy_id == proposal.source_policy_id
                    && existing.estimated_gain == proposal.estimated_gain
                    && existing.support_episodes == proposal.support_episodes;
                if same_revision {
                    return Ok(existing.clone());
                }
                return Err(ServerError::Conflict(format!(
                    "terminal skill proposal {} cannot be overwritten",
                    existing.id
                )));
            }
            existing.name = proposal.name;
            existing.description = proposal.description;
            existing.body = proposal.body;
            existing.trigger = proposal.trigger;
            existing.use_criteria = proposal.use_criteria;
            existing.evidence = proposal.evidence;
            existing.self_critique = proposal.self_critique;
            existing.verification = proposal.verification;
            existing.source_dream_id = proposal.source_dream_id;
            existing.source_session_id = proposal.source_session_id;
            existing.source_policy_id = proposal.source_policy_id;
            existing.estimated_gain = proposal.estimated_gain;
            existing.support_episodes = proposal.support_episodes;
            existing.updated_at = Utc::now();
            return Ok(existing.clone());
        }
        let now = Utc::now();
        let record = SkillProposalRecord {
            id: Uuid::new_v4(),
            name: proposal.name,
            description: proposal.description,
            body: proposal.body,
            trigger: proposal.trigger,
            use_criteria: proposal.use_criteria,
            evidence: proposal.evidence,
            self_critique: proposal.self_critique,
            verification: proposal.verification,
            status: SkillProposalStatus::Pending,
            dedupe_key: proposal.dedupe_key,
            source_dream_id: proposal.source_dream_id,
            source_session_id: proposal.source_session_id,
            source_policy_id: proposal.source_policy_id,
            estimated_gain: proposal.estimated_gain,
            support_episodes: proposal.support_episodes,
            created_at: now,
            updated_at: now,
        };
        inner.skill_proposals.push(record.clone());
        Ok(record)
    }

    async fn update_skill_proposal_status(
        &self,
        id: Uuid,
        status: SkillProposalStatus,
    ) -> Result<SkillProposalRecord> {
        let mut inner = self.inner.lock();
        let record = inner
            .skill_proposals
            .iter_mut()
            .find(|proposal| proposal.id == id)
            .ok_or_else(|| ServerError::NotFound(format!("skill proposal {id}")))?;
        record.status = status;
        record.updated_at = Utc::now();
        Ok(record.clone())
    }

    async fn skill_proposal(&self, id: Uuid) -> Result<SkillProposalRecord> {
        self.inner
            .lock()
            .skill_proposals
            .iter()
            .find(|proposal| proposal.id == id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("skill proposal {id}")))
    }

    async fn skill_proposals_for_session(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<SkillProposalRecord>> {
        self.get_session(session_id).await?;
        let mut proposals = self
            .inner
            .lock()
            .skill_proposals
            .iter()
            .filter(|proposal| proposal.source_session_id == session_id)
            .cloned()
            .collect::<Vec<_>>();
        proposals.sort_by_key(|proposal| std::cmp::Reverse(proposal.updated_at));
        Ok(proposals)
    }

    async fn record_skill_exposures_for_turn(
        &self,
        session_id: Uuid,
        turn_id: Uuid,
        skills: &[(String, String)],
    ) -> Result<(SessionEvent, bool)> {
        let payload_json = serde_json::json!({
            "skills": skills.iter().map(|(name, digest)| serde_json::json!({
                "name": name,
                "digest": digest,
            })).collect::<Vec<_>>(),
        });
        let mut inner = self.inner.lock();
        if !inner.sessions.contains_key(&session_id) {
            return Err(ServerError::NotFound(format!("session {session_id}")));
        }
        if !inner
            .turns
            .iter()
            .any(|turn| turn.id == turn_id && turn.session_id == session_id)
        {
            return Err(ServerError::NotFound(format!("turn {turn_id}")));
        }
        let events = inner.events.entry(session_id).or_default();
        if let Some(existing) = events
            .iter()
            .find(|event| event.turn_id == Some(turn_id) && event.event_type == "skill_selected")
            .cloned()
        {
            return Ok((existing, false));
        }
        let seq = events.len() as i64 + 1;
        let mut event =
            SessionEvent::new(session_id, seq, "skill_selected", payload_json, Utc::now());
        event.turn_id = Some(turn_id);
        events.push(event.clone());
        for (name, digest) in skills {
            let stats = inner
                .skill_runtime_stats
                .entry((name.clone(), digest.clone()))
                .or_default();
            stats.0 = stats.0.saturating_add(1);
        }
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            session.updated_at = event.created_at;
        }
        Ok((event, true))
    }

    async fn record_skill_outcome(&self, name: &str, digest: &str, pass: bool) -> Result<()> {
        let mut inner = self.inner.lock();
        let stats = inner
            .skill_runtime_stats
            .entry((name.to_string(), digest.to_string()))
            .or_default();
        if pass {
            stats.1 = stats.1.saturating_add(1);
        } else {
            stats.2 = stats.2.saturating_add(1);
        }
        Ok(())
    }

    async fn skill_runtime_stats(
        &self,
        names: &[String],
    ) -> Result<Vec<(String, String, u64, u64, u64)>> {
        let names = names.iter().collect::<std::collections::BTreeSet<_>>();
        Ok(self
            .inner
            .lock()
            .skill_runtime_stats
            .iter()
            .filter(|((name, _), _)| names.contains(name))
            .map(|((name, digest), (exposures, passes, fails))| {
                (name.clone(), digest.clone(), *exposures, *passes, *fails)
            })
            .collect())
    }

    async fn upsert_cron_job(&self, job: NewCronJobRecord) -> Result<CronJobRecord> {
        let job = sanitize_cron_job_persistence(job)?;
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .cron_jobs
            .iter_mut()
            .find(|existing| existing.id == job.id)
        {
            existing.name = job.name;
            existing.schedule = job.schedule;
            existing.enabled = job.enabled;
            existing.cron_mode = job.cron_mode;
            existing.max_turns = job.max_turns;
            existing.script_timeout_seconds = job.script_timeout_seconds;
            // Job registration updates policy/configuration, but the durable scheduler owns
            // the cursor. Re-registering a job on restart must not skip or regress a due fire.
            if existing.next_run_at.is_none() {
                existing.next_run_at = job.next_run_at;
            }
            existing.updated_at = Utc::now();
            return Ok(existing.clone());
        }
        let record = CronJobRecord {
            id: job.id,
            name: job.name,
            schedule: job.schedule,
            enabled: job.enabled,
            cron_mode: job.cron_mode,
            max_turns: job.max_turns,
            script_timeout_seconds: job.script_timeout_seconds,
            next_run_at: job.next_run_at,
            updated_at: Utc::now(),
        };
        inner.cron_jobs.push(record.clone());
        Ok(record)
    }

    async fn cron_job(&self, id: &str) -> Result<CronJobRecord> {
        self.inner
            .lock()
            .cron_jobs
            .iter()
            .find(|job| job.id == id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("cron job {id}")))
    }

    async fn cron_jobs(&self) -> Result<Vec<CronJobRecord>> {
        let mut jobs = self.inner.lock().cron_jobs.clone();
        jobs.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(jobs)
    }

    async fn materialize_cron_run(
        &self,
        run: NewCronRunRecord,
        expected_next_run_at: DateTime<Utc>,
        next_run_at: DateTime<Utc>,
    ) -> Result<Option<CronRunRecord>> {
        let run = sanitize_cron_run_persistence(run)?;
        if run.scheduled_for != expected_next_run_at || next_run_at <= expected_next_run_at {
            return Err(ServerError::InvalidRequest(
                "cron materialization must match and advance the job cursor".to_string(),
            ));
        }
        let mut inner = self.inner.lock();
        let Some(job) = inner.cron_jobs.iter_mut().find(|job| job.id == run.job_id) else {
            return Err(ServerError::NotFound(format!("cron job {}", run.job_id)));
        };
        if !job.enabled || job.next_run_at != Some(expected_next_run_at) {
            return Ok(None);
        }
        job.next_run_at = Some(next_run_at);
        job.updated_at = Utc::now();

        if let Some(existing) = inner.cron_runs.iter().find(|existing| {
            existing.job_id == run.job_id && existing.scheduled_for == run.scheduled_for
        }) {
            return Ok(Some(existing.clone()));
        }
        let now = Utc::now();
        let record = CronRunRecord {
            id: Uuid::new_v4(),
            job_id: run.job_id,
            scheduled_for: run.scheduled_for,
            status: "queued".to_string(),
            session_id: run.session_id,
            started_at: now,
            completed_at: None,
            attempts: 0,
            available_at: now,
            locked_at: None,
            lease_owner: None,
            lease_epoch: 0,
            last_error: None,
            result_json: run.result_json,
        };
        inner.cron_runs.push(record.clone());
        Ok(Some(record))
    }

    async fn claim_ready_cron_run(
        &self,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        max_attempts: i32,
    ) -> Result<Option<CronLease>> {
        if max_attempts <= 0 {
            return Err(ServerError::InvalidRequest(
                "cron max_attempts must be positive".to_string(),
            ));
        }
        let stale_before = now - lease_timeout;
        let mut inner = self.inner.lock();
        for run in &mut inner.cron_runs {
            let ready = (run.status == "queued" && run.available_at <= now)
                || (run.status == "running"
                    && run.locked_at.is_some_and(|locked| locked <= stale_before));
            if ready && run.attempts >= max_attempts {
                run.status = "failed".to_string();
                run.completed_at = Some(now);
                run.locked_at = None;
                run.lease_owner = None;
                run.last_error =
                    Some("cron lease expired after the final permitted attempt".to_string());
            }
        }
        let Some(index) = inner
            .cron_runs
            .iter()
            .enumerate()
            .filter(|(_, run)| {
                run.attempts < max_attempts
                    && ((run.status == "queued" && run.available_at <= now)
                        || (run.status == "running"
                            && run.locked_at.is_some_and(|locked| locked <= stale_before)))
            })
            .min_by_key(|(_, run)| (run.available_at, run.scheduled_for, run.id))
            .map(|(index, _)| index)
        else {
            return Ok(None);
        };
        let run = &mut inner.cron_runs[index];
        run.status = "running".to_string();
        run.attempts += 1;
        run.available_at = now;
        run.locked_at = Some(now);
        run.lease_owner = Some(owner_id);
        run.lease_epoch += 1;
        run.last_error = None;
        Ok(Some(CronLease {
            run: run.clone(),
            owner_id,
            epoch: run.lease_epoch,
        }))
    }

    async fn claim_cron_run(
        &self,
        run: NewCronRunRecord,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<(CronLease, bool)> {
        let run = sanitize_cron_run_persistence(run)?;
        self.cron_job(&run.job_id).await?;
        if let Some(session_id) = run.session_id {
            self.get_session(session_id).await?;
        }
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.cron_runs.iter_mut().find(|existing| {
            existing.job_id == run.job_id && existing.scheduled_for == run.scheduled_for
        }) {
            let stale_before = now - lease_timeout;
            let claimable = (existing.status == "queued" && existing.available_at <= now)
                || (existing.status == "running"
                    && existing
                        .locked_at
                        .is_some_and(|locked_at| locked_at <= stale_before));
            if claimable && existing.attempts >= 3 {
                existing.status = "failed".to_string();
                existing.completed_at = Some(now);
                existing.locked_at = None;
                existing.lease_owner = None;
                existing.last_error =
                    Some("cron lease expired after the final permitted attempt".to_string());
                return Ok((
                    CronLease {
                        run: existing.clone(),
                        owner_id,
                        epoch: existing.lease_epoch,
                    },
                    false,
                ));
            }
            if claimable {
                existing.status = "running".to_string();
                existing.attempts += 1;
                existing.available_at = now;
                existing.locked_at = Some(now);
                existing.lease_owner = Some(owner_id);
                existing.lease_epoch += 1;
                existing.last_error = None;
            }
            return Ok((
                CronLease {
                    run: existing.clone(),
                    owner_id: existing.lease_owner.unwrap_or(owner_id),
                    epoch: existing.lease_epoch,
                },
                claimable,
            ));
        }
        let record = CronRunRecord {
            id: Uuid::new_v4(),
            job_id: run.job_id,
            scheduled_for: run.scheduled_for,
            status: "running".to_string(),
            session_id: run.session_id,
            started_at: now,
            completed_at: None,
            attempts: 1,
            available_at: now,
            locked_at: Some(now),
            lease_owner: Some(owner_id),
            lease_epoch: 1,
            last_error: None,
            result_json: run.result_json,
        };
        inner.cron_runs.push(record.clone());
        Ok((
            CronLease {
                run: record,
                owner_id,
                epoch: 1,
            },
            true,
        ))
    }

    async fn record_cron_run(&self, run: NewCronRunRecord) -> Result<CronRunRecord> {
        let run = sanitize_cron_run_persistence(run)?;
        self.cron_job(&run.job_id).await?;
        if let Some(session_id) = run.session_id {
            self.get_session(session_id).await?;
        }
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.cron_runs.iter().find(|existing| {
            existing.job_id == run.job_id && existing.scheduled_for == run.scheduled_for
        }) {
            return Ok(existing.clone());
        }
        let now = Utc::now();
        let record = CronRunRecord {
            id: Uuid::new_v4(),
            job_id: run.job_id,
            scheduled_for: run.scheduled_for,
            status: run.status,
            session_id: run.session_id,
            started_at: now,
            completed_at: None,
            attempts: 0,
            available_at: now,
            locked_at: None,
            lease_owner: None,
            lease_epoch: 0,
            last_error: None,
            result_json: run.result_json,
        };
        inner.cron_runs.push(record.clone());
        Ok(record)
    }

    async fn heartbeat_cron_run(&self, lease: &CronLease, now: DateTime<Utc>) -> Result<CronLease> {
        let mut inner = self.inner.lock();
        let record = inner
            .cron_runs
            .iter_mut()
            .find(|run| {
                run.id == lease.run.id
                    && run.status == "running"
                    && run.lease_owner == Some(lease.owner_id)
                    && run.lease_epoch == lease.epoch
            })
            .ok_or_else(|| stale_cron_lease(lease))?;
        record.locked_at = Some(now);
        Ok(CronLease {
            run: record.clone(),
            owner_id: lease.owner_id,
            epoch: lease.epoch,
        })
    }

    async fn complete_cron_run(
        &self,
        lease: &CronLease,
        status: &str,
        session_id: Option<Uuid>,
        result_json: Value,
    ) -> Result<CronRunRecord> {
        validate_persistence_identifier("cron completion status", status)?;
        let result_json = redact_persisted_json(result_json);
        if let Some(session_id) = session_id {
            self.get_session(session_id).await?;
        }
        let mut inner = self.inner.lock();
        let record = inner
            .cron_runs
            .iter_mut()
            .find(|run| {
                run.id == lease.run.id
                    && run.status == "running"
                    && run.lease_owner == Some(lease.owner_id)
                    && run.lease_epoch == lease.epoch
            })
            .ok_or_else(|| stale_cron_lease(lease))?;
        record.status = status.to_string();
        record.session_id = session_id.or(record.session_id);
        record.completed_at = Some(Utc::now());
        record.locked_at = None;
        record.lease_owner = None;
        record.last_error = None;
        record.result_json = result_json;
        Ok(record.clone())
    }

    async fn fail_cron_run(
        &self,
        lease: &CronLease,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<CronRunRecord> {
        let error = redact_persisted_text(&error);
        let mut inner = self.inner.lock();
        let record = inner
            .cron_runs
            .iter_mut()
            .find(|run| {
                run.id == lease.run.id
                    && run.status == "running"
                    && run.lease_owner == Some(lease.owner_id)
                    && run.lease_epoch == lease.epoch
            })
            .ok_or_else(|| stale_cron_lease(lease))?;
        record.status = if record.attempts >= max_attempts {
            "failed".to_string()
        } else {
            "queued".to_string()
        };
        record.available_at = next_available_at;
        record.completed_at = None;
        record.locked_at = None;
        record.lease_owner = None;
        record.last_error = Some(error);
        Ok(record.clone())
    }

    async fn cron_runs(&self, job_id: &str, limit: usize) -> Result<Vec<CronRunRecord>> {
        self.cron_job(job_id).await?;
        let mut runs = self
            .inner
            .lock()
            .cron_runs
            .iter()
            .filter(|run| run.job_id == job_id)
            .cloned()
            .collect::<Vec<_>>();
        runs.sort_by_key(|run| std::cmp::Reverse(run.started_at));
        runs.truncate(limit);
        Ok(runs)
    }

    async fn runtime_metrics(&self, now: DateTime<Utc>) -> Result<StoreRuntimeMetrics> {
        let inner = self.inner.lock();
        let turn_oldest = inner
            .turns
            .iter()
            .filter(|turn| turn.status == "queued")
            .map(|turn| turn.created_at)
            .min();
        let dream_oldest = inner
            .dream_queue
            .iter()
            .filter(|dream| dream.status == DreamStatus::Queued)
            .map(|dream| dream.enqueued_at)
            .min();
        let cron_oldest = inner
            .cron_runs
            .iter()
            .filter(|run| run.status == "queued")
            .map(|run| run.started_at)
            .min();
        let scheduler_cursor = inner
            .cron_jobs
            .iter()
            .filter(|job| job.enabled)
            .filter_map(|job| job.next_run_at)
            .filter(|next| *next <= now)
            .min();
        let approval_effect_oldest = inner
            .approval_effects
            .iter()
            .filter(|effect| effect.status == "pending")
            .map(|effect| effect.created_at)
            .min();
        let age_seconds = |timestamp: Option<DateTime<Utc>>| {
            timestamp
                .map(|timestamp| now.signed_duration_since(timestamp).num_seconds().max(0))
                .unwrap_or(0)
        };
        Ok(StoreRuntimeMetrics {
            turn_queue_depth: inner
                .turns
                .iter()
                .filter(|turn| turn.status == "queued")
                .count() as i64,
            turn_oldest_age_seconds: age_seconds(turn_oldest),
            dream_queue_depth: inner
                .dream_queue
                .iter()
                .filter(|dream| dream.status == DreamStatus::Queued)
                .count() as i64,
            dream_oldest_age_seconds: age_seconds(dream_oldest),
            cron_queue_depth: inner
                .cron_runs
                .iter()
                .filter(|run| run.status == "queued")
                .count() as i64,
            cron_oldest_age_seconds: age_seconds(cron_oldest),
            scheduler_lag_seconds: age_seconds(scheduler_cursor),
            pending_approvals: inner
                .approval_requests
                .iter()
                .filter(|approval| approval.status == "pending")
                .count() as i64,
            approval_effect_queue_depth: inner
                .approval_effects
                .iter()
                .filter(|effect| effect.status == "pending")
                .count() as i64,
            approval_effect_oldest_age_seconds: age_seconds(approval_effect_oldest),
            lease_reclaims: inner
                .dream_queue
                .iter()
                .map(|dream| i64::from((dream.attempts - 1).max(0)))
                .chain(
                    inner
                        .cron_runs
                        .iter()
                        .map(|run| i64::from((run.attempts - 1).max(0))),
                )
                .chain(
                    inner
                        .approval_effects
                        .iter()
                        .map(|effect| i64::from((effect.attempts - 1).max(0))),
                )
                .sum(),
        })
    }
}

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tm_host::{EvolutionAuditRecord, EvolutionAuditStatus};
use tm_memory::{
    DreamLease, DreamQueueRecord, DreamStatus, MemorySummaryRecord, NewDreamQueueRecord,
    NewMemorySummaryRecord, NewSkillProposalRecord, SkillProposalRecord, SkillProposalStatus,
};
use tm_modes::ReviewProposalStatus;
use uuid::Uuid;

use crate::{Result, ServerError};

use super::models::{
    redact_persisted_json, redact_persisted_text, sanitize_cron_job_persistence,
    sanitize_cron_run_persistence, sanitize_evolution_review_proposal_persistence,
    sanitize_memory_summary_persistence, sanitize_project_item_persistence,
    sanitize_skill_proposal_persistence, turn_content_hash, validate_persistence_identifier,
    validate_profile_fact_persistence, validate_recall_chunk_persistence,
};

use super::{
    ApprovalEffectLease, ApprovalEffectRecord, ApprovalRequestRecord, CronJobRecord, CronLease,
    CronRunRecord, EndSessionDreamResult, EvolutionAuditEntry, EvolutionReviewProposalRecord,
    MessageRecord, ModeState, NewApprovalRequest, NewApprovalResolution, NewCronJobRecord,
    NewCronRunRecord, NewEvolutionReviewProposal, NewProjectItem, NewSession, ProfileFactRecord,
    ProjectItemKind, ProjectItemRecord, RecallChunkRecord, SessionEvent, SessionRecord,
    SessionSummaryRecord, SessionTurnRecord, Store, StoreRuntimeMetrics,
};

#[derive(Debug, Default, Clone)]
pub struct InMemoryStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
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
    project_items: Vec<ProjectItemRecord>,
    dream_queue: Vec<DreamQueueRecord>,
    memory_summaries: Vec<MemorySummaryRecord>,
    skill_proposals: Vec<SkillProposalRecord>,
    cron_jobs: Vec<CronJobRecord>,
    cron_runs: Vec<CronRunRecord>,
}

fn append_evolution_audit_in_memory(
    inner: &mut Inner,
    entry: EvolutionAuditEntry,
) -> Result<EvolutionAuditRecord> {
    if let Some(existing) = inner
        .evolution_audits
        .iter()
        .find(|existing| existing.idempotency_key == entry.idempotency_key)
    {
        return Ok(existing.record.clone());
    }
    let record = entry.record.clone();
    inner.evolution_audits.push(entry);
    Ok(record)
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

fn stale_approval_effect_lease(lease: &ApprovalEffectLease) -> ServerError {
    ServerError::NotFound(format!(
        "active approval effect lease {} owner {} epoch {}",
        lease.effect.id, lease.owner_id, lease.epoch
    ))
}

fn approval_effect_is_claimable(
    effect: &ApprovalEffectRecord,
    now: DateTime<Utc>,
    stale_before: DateTime<Utc>,
) -> bool {
    (effect.status == "pending" && effect.available_at <= now)
        || (effect.status == "claimed"
            && effect
                .locked_at
                .is_some_and(|locked| locked <= stale_before))
}

fn claim_approval_effect_at(
    effect: &mut ApprovalEffectRecord,
    owner_id: Uuid,
    now: DateTime<Utc>,
) -> ApprovalEffectLease {
    effect.status = "claimed".to_string();
    effect.attempts += 1;
    effect.locked_at = Some(now);
    effect.lease_owner = Some(owner_id);
    effect.lease_epoch += 1;
    effect.updated_at = now;
    effect.error_at = None;
    effect.last_error = None;
    ApprovalEffectLease {
        effect: effect.clone(),
        owner_id,
        epoch: effect.lease_epoch,
    }
}

fn resolve_approval_in_memory(
    inner: &mut Inner,
    session_id: Uuid,
    approval_id: Uuid,
    mut resolution: NewApprovalResolution,
) -> Result<ApprovalRequestRecord> {
    tm_memory::redact_json_value(&mut resolution.resolution_json);
    if !matches!(
        resolution.status.as_str(),
        "approved" | "denied" | "timed_out" | "cancelled"
    ) {
        return Err(ServerError::InvalidRequest(format!(
            "invalid terminal approval status {}",
            resolution.status
        )));
    }
    let index = inner
        .approval_requests
        .iter()
        .position(|approval| approval.id == approval_id && approval.session_id == session_id)
        .ok_or_else(|| ServerError::NotFound(format!("approval {approval_id}")))?;
    if inner.approval_requests[index].status != "pending" {
        return Err(ServerError::Conflict(format!(
            "approval {approval_id} is already resolved"
        )));
    }
    let effect_payload = inner
        .approval_effects
        .iter()
        .find(|effect| effect.approval_id == approval_id)
        .map(|effect| (effect.id, effect.payload_json.clone()));
    let audit = if let Some((effect_id, payload)) = effect_payload {
        crate::evolution::evolution_audit_record(
            &payload,
            approval_id,
            effect_id,
            crate::evolution::audit_status_from_approval(&resolution.status)?,
            resolution.resolved_at,
            None,
        )?
        .map(|record| EvolutionAuditEntry {
            idempotency_key: format!("approval:{approval_id}:resolution:{}", resolution.status),
            record,
        })
    } else {
        None
    };
    {
        let approval = &mut inner.approval_requests[index];
        approval.status = resolution.status;
        approval.resolved_at = Some(resolution.resolved_at);
        approval.selected_option_id = resolution.selected_option_id;
        approval.resolution_json = Some(resolution.resolution_json.clone());
        approval.resolution_version += 1;
    }
    if let Some(effect) = inner
        .approval_effects
        .iter_mut()
        .find(|effect| effect.approval_id == approval_id)
    {
        effect.status = "pending".to_string();
        effect.available_at = resolution.resolved_at;
        effect.updated_at = resolution.resolved_at;
        if let Value::Object(payload) = &mut effect.payload_json {
            payload.insert("resolution".to_string(), resolution.resolution_json);
        } else {
            effect.payload_json = json!({
                "effect": effect.payload_json.clone(),
                "resolution": resolution.resolution_json,
            });
        }
    } else {
        inner.approval_effects.push(ApprovalEffectRecord {
            id: approval_id,
            approval_id,
            session_id,
            effect_type: "approval_resolution".to_string(),
            payload_json: json!({ "resolution": resolution.resolution_json }),
            status: "pending".to_string(),
            attempts: 0,
            available_at: resolution.resolved_at,
            locked_at: None,
            lease_owner: None,
            lease_epoch: 0,
            applied_at: None,
            error_at: None,
            last_error: None,
            created_at: resolution.resolved_at,
            updated_at: resolution.resolved_at,
        });
    }
    if let Some(audit) = audit {
        append_evolution_audit_in_memory(inner, audit)?;
    }
    Ok(inner.approval_requests[index].clone())
}

fn resolve_approval_with_event_in_memory(
    inner: &mut Inner,
    session_id: Uuid,
    approval_id: Uuid,
    mut resolution: NewApprovalResolution,
) -> Result<(ApprovalRequestRecord, SessionEvent)> {
    tm_memory::redact_json_value(&mut resolution.resolution_json);
    let approval_id_text = approval_id.to_string();
    if resolution
        .resolution_json
        .get("approvalId")
        .and_then(Value::as_str)
        != Some(approval_id_text.as_str())
    {
        return Err(ServerError::InvalidRequest(
            "approval resolution event payload has a different approvalId".to_string(),
        ));
    }
    let event_payload = resolution.resolution_json.clone();
    let event_at = resolution.resolved_at;
    let mut approval = resolve_approval_in_memory(inner, session_id, approval_id, resolution)?;
    let events = inner.events.entry(session_id).or_default();
    let mut event = SessionEvent::new(
        session_id,
        events.len() as i64 + 1,
        "approval_resolved",
        event_payload,
        event_at,
    );
    event.turn_id = approval.turn_id;
    events.push(event.clone());
    let stored = inner
        .approval_requests
        .iter_mut()
        .find(|record| record.id == approval_id && record.session_id == session_id)
        .expect("resolved approval remains present");
    stored.resolution_event_seq = Some(event.seq);
    approval.resolution_event_seq = Some(event.seq);
    if let Some(session) = inner.sessions.get_mut(&session_id) {
        session.updated_at = event_at;
    }
    Ok((approval, event))
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
            memory_scope: "global".to_string(),
        };
        inner.sessions.insert(session.id, session.clone());
        Ok(session)
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

    async fn set_session_memory_scope(
        &self,
        session_id: Uuid,
        memory_scope: &str,
    ) -> Result<SessionRecord> {
        let memory_scope = memory_scope.trim();
        if memory_scope.is_empty() {
            return Err(ServerError::InvalidRequest(
                "memory scope must not be empty".to_string(),
            ));
        }
        validate_persistence_identifier("memory scope", memory_scope)?;
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
        session.memory_scope = memory_scope.to_string();
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
                session.memory_scope.clone(),
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

    async fn create_approval_request(
        &self,
        mut request: NewApprovalRequest,
    ) -> Result<ApprovalRequestRecord> {
        validate_persistence_identifier("approval origin", &request.origin)?;
        validate_persistence_identifier("approval effect type", &request.effect_type)?;
        request.action = redact_persisted_text(&request.action);
        tm_memory::redact_json_value(&mut request.scope_json);
        tm_memory::redact_json_value(&mut request.options_json);
        tm_memory::redact_json_value(&mut request.effect_payload_json);
        if request.origin.trim().is_empty() || request.action.trim().is_empty() {
            return Err(ServerError::InvalidRequest(
                "approval origin and action must not be empty".to_string(),
            ));
        }
        if !request.options_json.is_array() {
            return Err(ServerError::InvalidRequest(
                "approval options must be an array".to_string(),
            ));
        }
        if request.effect_type.trim().is_empty() {
            return Err(ServerError::InvalidRequest(
                "approval effect type must not be empty".to_string(),
            ));
        }
        if request.expires_at <= request.created_at {
            return Err(ServerError::InvalidRequest(
                "approval expiry must be after creation".to_string(),
            ));
        }
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
            created_at: now,
            updated_at: now,
        };
        inner.evolution_review_proposals.push(record.clone());
        Ok(record)
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

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde_json::Value;
use tm_memory::{
    DreamQueueRecord, DreamStatus, MemorySummaryRecord, NewDreamQueueRecord,
    NewMemorySummaryRecord, NewSkillProposalRecord, SkillProposalRecord, SkillProposalStatus,
};
use uuid::Uuid;

use crate::{Result, ServerError};

use super::{
    CronJobRecord, CronRunRecord, MessageRecord, ModeState, NewCronJobRecord, NewCronRunRecord,
    NewProjectItem, NewSession, ProfileFactRecord, ProjectItemKind, ProjectItemRecord,
    RecallChunkRecord, SessionEvent, SessionRecord, SessionSummaryRecord, Store,
};

#[derive(Debug, Default, Clone)]
pub struct InMemoryStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    sessions: BTreeMap<Uuid, SessionRecord>,
    events: BTreeMap<Uuid, Vec<SessionEvent>>,
    messages: BTreeMap<Uuid, Vec<MessageRecord>>,
    profile_facts: Vec<ProfileFactRecord>,
    recall_chunks: Vec<RecallChunkRecord>,
    project_items: Vec<ProjectItemRecord>,
    dream_queue: Vec<DreamQueueRecord>,
    memory_summaries: Vec<MemorySummaryRecord>,
    skill_proposals: Vec<SkillProposalRecord>,
    cron_jobs: Vec<CronJobRecord>,
    cron_runs: Vec<CronRunRecord>,
}

#[async_trait]
impl Store for InMemoryStore {
    async fn create_session(&self, new: NewSession) -> Result<SessionRecord> {
        let now = Utc::now();
        let session = SessionRecord {
            id: Uuid::new_v4(),
            created_at: now,
            updated_at: now,
            status: "open".to_string(),
            mode: new.mode.clone(),
            mode_state: ModeState::new(new.mode, now),
            persona_status: new.persona_status,
        };
        let mut inner = self.inner.lock();
        inner.sessions.insert(session.id, session.clone());
        Ok(session)
    }

    async fn end_session(&self, session_id: Uuid) -> Result<SessionRecord> {
        let mut inner = self.inner.lock();
        let session = inner
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))?;
        session.status = "ended".to_string();
        session.updated_at = Utc::now();
        Ok(session.clone())
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
        self.get_session(session_id).await?;
        let mut inner = self.inner.lock();
        let messages = inner.messages.entry(session_id).or_default();
        let seq = messages.len() as i64 + 1;
        let message = MessageRecord {
            session_id,
            seq,
            role: role.to_string(),
            content: content.to_string(),
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
        self.get_session(session_id).await?;
        let mut inner = self.inner.lock();
        let events = inner.events.entry(session_id).or_default();
        let seq = events.len() as i64 + 1;
        let event = SessionEvent::new(session_id, seq, event_type, payload_json, Utc::now());
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

    async fn add_profile_fact(&self, fact: ProfileFactRecord) -> Result<()> {
        self.inner.lock().profile_facts.push(fact);
        Ok(())
    }

    async fn add_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<()> {
        self.inner.lock().recall_chunks.push(chunk);
        Ok(())
    }

    async fn upsert_profile_fact(&self, fact: ProfileFactRecord) -> Result<ProfileFactRecord> {
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
    ) -> Result<Option<DreamQueueRecord>> {
        let stale_before = now - lease_timeout;
        let mut inner = self.inner.lock();
        let Some(index) = inner
            .dream_queue
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                (record.status == DreamStatus::Queued && record.available_at <= now)
                    || (record.status == DreamStatus::Running
                        && record
                            .locked_at
                            .as_ref()
                            .is_some_and(|locked_at| *locked_at <= stale_before))
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
        record.last_error = None;
        Ok(Some(record.clone()))
    }

    async fn heartbeat_dream(
        &self,
        dream_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<DreamQueueRecord> {
        let mut inner = self.inner.lock();
        let record = inner
            .dream_queue
            .iter_mut()
            .find(|record| record.id == dream_id && record.status == DreamStatus::Running)
            .ok_or_else(|| ServerError::NotFound(format!("running dream {dream_id}")))?;
        record.locked_at = Some(now);
        Ok(record.clone())
    }

    async fn complete_dream(&self, dream_id: Uuid, now: DateTime<Utc>) -> Result<DreamQueueRecord> {
        let mut inner = self.inner.lock();
        let record = inner
            .dream_queue
            .iter_mut()
            .find(|record| record.id == dream_id && record.status == DreamStatus::Running)
            .ok_or_else(|| ServerError::NotFound(format!("running dream {dream_id}")))?;
        record.status = DreamStatus::Completed;
        record.available_at = now;
        record.locked_at = None;
        record.last_error = None;
        Ok(record.clone())
    }

    async fn fail_dream(
        &self,
        dream_id: Uuid,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<DreamQueueRecord> {
        let mut inner = self.inner.lock();
        let record = inner
            .dream_queue
            .iter_mut()
            .find(|record| record.id == dream_id && record.status == DreamStatus::Running)
            .ok_or_else(|| ServerError::NotFound(format!("running dream {dream_id}")))?;
        record.status = if record.attempts >= max_attempts {
            DreamStatus::Failed
        } else {
            DreamStatus::Queued
        };
        record.available_at = next_available_at;
        record.locked_at = None;
        record.last_error = Some(error);
        Ok(record.clone())
    }

    async fn upsert_memory_summary(
        &self,
        summary: NewMemorySummaryRecord,
    ) -> Result<MemorySummaryRecord> {
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
            existing.next_run_at = job.next_run_at;
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

    async fn claim_cron_run(&self, run: NewCronRunRecord) -> Result<(CronRunRecord, bool)> {
        self.cron_job(&run.job_id).await?;
        if let Some(session_id) = run.session_id {
            self.get_session(session_id).await?;
        }
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .cron_runs
            .iter()
            .find(|existing| {
                existing.job_id == run.job_id && existing.scheduled_for == run.scheduled_for
            })
            .cloned()
        {
            return Ok((existing, false));
        }
        let record = CronRunRecord {
            id: Uuid::new_v4(),
            job_id: run.job_id,
            scheduled_for: run.scheduled_for,
            status: run.status,
            session_id: run.session_id,
            started_at: Utc::now(),
            completed_at: None,
            result_json: run.result_json,
        };
        inner.cron_runs.push(record.clone());
        Ok((record, true))
    }

    async fn record_cron_run(&self, run: NewCronRunRecord) -> Result<CronRunRecord> {
        let (record, _) = self.claim_cron_run(run).await?;
        Ok(record)
    }

    async fn complete_cron_run(
        &self,
        run_id: Uuid,
        status: &str,
        session_id: Option<Uuid>,
        result_json: Value,
    ) -> Result<CronRunRecord> {
        if let Some(session_id) = session_id {
            self.get_session(session_id).await?;
        }
        let mut inner = self.inner.lock();
        let record = inner
            .cron_runs
            .iter_mut()
            .find(|run| run.id == run_id)
            .ok_or_else(|| ServerError::NotFound(format!("cron run {run_id}")))?;
        record.status = status.to_string();
        record.session_id = session_id.or(record.session_id);
        record.completed_at = Some(Utc::now());
        record.result_json = result_json;
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
}

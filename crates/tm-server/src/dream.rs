use std::{collections::BTreeMap, sync::Arc, time::Duration as StdDuration};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tm_memory::{
    BudgetedDreamInput, DreamInputBudget, DreamInputMessage, DreamQueueRecord, DreamStatus,
    DreamWorker, DreamWorkerReport, MemoryEvidenceRef, MemorySummaryKind, MemorySummaryRecord,
    NewMemorySummaryRecord, NewSkillProposalRecord, SkillProposalRecord, SkillProposalStatus,
    SkillVerification, redact_dream_text,
};
use tokio::{
    sync::{broadcast, watch},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::memory::{MemoryRecordRef, MemoryWriteProposal, MemoryWriteStatus};
use crate::{
    ApprovalBroker, ApprovalOption, ApprovalPrompt, ApprovalStatus, CodingEventSink, Result,
    ServerError, SessionEvent, Store, StoreCodingEventSink,
};

#[derive(Debug, Clone)]
pub struct DreamWorkerConfig {
    pub enabled: bool,
    pub poll_interval: Duration,
    pub lease_timeout: Duration,
    pub retry_backoff: Duration,
    pub max_attempts: i32,
    pub concurrency: usize,
    pub per_dream_timeout: StdDuration,
    pub proposal_timeout: StdDuration,
    pub redaction: DreamRedactionConfig,
    pub input_budget: DreamInputBudget,
    pub summary_cadence: DreamSummaryCadence,
    pub max_summary_chars: usize,
    pub reflect_importance_threshold: f32,
    pub model_roles: DreamModelRoles,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DreamRedactionConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DreamSummaryCadence {
    pub session_every_dream: bool,
    pub rollup_every_dream: bool,
}

#[derive(Debug, Clone)]
pub struct DreamModelRoles {
    pub extraction: String,
    pub reflection: String,
    pub summarization: String,
    pub skill_distillation: String,
    pub self_critique: String,
    pub verification: String,
    pub embeddings: String,
}

impl Default for DreamWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval: Duration::seconds(5),
            lease_timeout: Duration::seconds(60),
            retry_backoff: Duration::seconds(30),
            max_attempts: 3,
            concurrency: 1,
            per_dream_timeout: StdDuration::from_secs(120),
            proposal_timeout: StdDuration::from_secs(60),
            redaction: DreamRedactionConfig::default(),
            input_budget: DreamInputBudget::default(),
            summary_cadence: DreamSummaryCadence::default(),
            max_summary_chars: 2_400,
            reflect_importance_threshold: 1.5,
            model_roles: DreamModelRoles::default(),
        }
    }
}

impl Default for DreamRedactionConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Default for DreamSummaryCadence {
    fn default() -> Self {
        Self {
            session_every_dream: true,
            rollup_every_dream: true,
        }
    }
}

impl Default for DreamModelRoles {
    fn default() -> Self {
        Self {
            extraction: "cheap".to_string(),
            reflection: "cheap".to_string(),
            summarization: "cheap".to_string(),
            skill_distillation: "cheap".to_string(),
            self_critique: "cheap".to_string(),
            verification: "cheap".to_string(),
            embeddings: "cheap".to_string(),
        }
    }
}

impl DreamModelRoles {
    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("extraction", &self.extraction),
            ("reflection", &self.reflection),
            ("summarization", &self.summarization),
            ("skill_distillation", &self.skill_distillation),
            ("self_critique", &self.self_critique),
            ("verification", &self.verification),
            ("embeddings", &self.embeddings),
        ] {
            if value.trim().is_empty() {
                return Err(ServerError::Policy(format!(
                    "dream model role {name} is not configured"
                )));
            }
        }
        Ok(())
    }
}

type SenderFactory = Arc<dyn Fn(Uuid) -> broadcast::Sender<SessionEvent> + Send + Sync>;

pub struct ServerDreamWorker<S> {
    store: Arc<S>,
    approval_broker: Arc<ApprovalBroker>,
    sender_for: SenderFactory,
    config: DreamWorkerConfig,
}

pub struct DreamWorkerDaemon<S> {
    worker: Arc<ServerDreamWorker<S>>,
}

pub struct DreamWorkerDaemonHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl<S> ServerDreamWorker<S> {
    pub fn new(
        store: Arc<S>,
        approval_broker: Arc<ApprovalBroker>,
        sender_for: SenderFactory,
        config: DreamWorkerConfig,
    ) -> Self {
        Self {
            store,
            approval_broker,
            sender_for,
            config,
        }
    }

    pub fn into_daemon(self) -> DreamWorkerDaemon<S> {
        DreamWorkerDaemon::new(Arc::new(self))
    }
}

impl<S> ServerDreamWorker<S>
where
    S: Store,
{
    pub async fn run_batch_result(&self) -> Result<DreamWorkerReport> {
        if !self.config.enabled {
            return Ok(DreamWorkerReport::default());
        }
        let runs = (0..self.config.concurrency.max(1)).map(|_| self.run_once_result());
        let mut aggregate = DreamWorkerReport::default();
        for report in join_all(runs).await {
            let report = report?;
            aggregate.attempted += report.attempted;
            aggregate.completed += report.completed;
            aggregate.proposals += report.proposals;
        }
        Ok(aggregate)
    }

    pub async fn run_once_result(&self) -> Result<DreamWorkerReport> {
        if !self.config.enabled {
            return Ok(DreamWorkerReport::default());
        }
        let now = Utc::now();
        let Some(dream) = self
            .store
            .claim_ready_dream(now, self.config.lease_timeout)
            .await?
        else {
            return Ok(DreamWorkerReport::default());
        };
        let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
            dream.session_id,
            Arc::clone(&self.store),
            (self.sender_for)(dream.session_id),
        ));
        sink.emit("dream_started", dream_started_payload(&dream))
            .await?;

        let processed = if self.config.per_dream_timeout.is_zero() {
            Err(ServerError::Store("dream timed out".to_string()))
        } else {
            match tokio::time::timeout(
                self.config.per_dream_timeout,
                self.process_claimed_dream(&dream, Arc::clone(&sink)),
            )
            .await
            {
                Ok(processed) => processed,
                Err(_) => Err(ServerError::Store("dream timed out".to_string())),
            }
        };

        match processed {
            Ok(mut report) => {
                let completed = self.store.complete_dream(dream.id, Utc::now()).await?;
                sink.emit(
                    "dream_completed",
                    json!({
                        "dreamId": completed.id,
                        "sessionId": completed.session_id,
                        "status": completed.status,
                        "attempts": completed.attempts,
                        "proposals": report.proposals,
                    }),
                )
                .await?;
                report.completed = 1;
                Ok(report)
            }
            Err(err) => {
                let next_available_at = Utc::now() + self.config.retry_backoff;
                let failed = self
                    .store
                    .fail_dream(
                        dream.id,
                        err.to_string(),
                        next_available_at,
                        self.config.max_attempts,
                    )
                    .await?;
                sink.emit(
                    "dream_failed",
                    json!({
                        "dreamId": failed.id,
                        "sessionId": failed.session_id,
                        "status": failed.status,
                        "attempts": failed.attempts,
                        "lastError": failed.last_error,
                        "availableAt": failed.available_at,
                    }),
                )
                .await?;
                Ok(DreamWorkerReport {
                    attempted: 1,
                    completed: 0,
                    proposals: 0,
                })
            }
        }
    }

    async fn process_claimed_dream(
        &self,
        dream: &DreamQueueRecord,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<DreamWorkerReport> {
        if !self.config.redaction.enabled {
            return Err(ServerError::Policy(
                "dream redaction is disabled by config".to_string(),
            ));
        }
        self.config.model_roles.validate()?;
        let messages = self.store.session_messages(dream.session_id).await?;
        let events = self.store.events_after(dream.session_id, None).await?;
        let redacted_messages = messages
            .iter()
            .map(|message| {
                let redacted = redact_dream_text(&message.content);
                DreamInputMessage {
                    seq: message.seq,
                    role: message.role.clone(),
                    content: redacted.text,
                    had_redactions: !redacted.redactions.is_empty(),
                }
            })
            .collect::<Vec<_>>();
        let redaction_count = redacted_messages
            .iter()
            .filter(|message| message.had_redactions)
            .count();
        let budgeted_input = self.config.input_budget.apply(redacted_messages);
        let redacted_messages = budgeted_input
            .messages()
            .map(RedactedMessage::from)
            .collect::<Vec<_>>();
        sink.emit(
            "dream_progress",
            json!({
                "dreamId": dream.id,
                "phase": "input_collected",
                "messages": budgeted_input.included_messages,
                "totalMessages": budgeted_input.total_messages,
                "omittedMessages": budgeted_input.omitted_messages,
                "truncatedMessages": budgeted_input.truncated_messages,
                "inputChunks": budgeted_input.chunks.len(),
                "inputChars": budgeted_input.included_chars,
                "totalInputChars": budgeted_input.total_chars,
                "inputTruncated": budgeted_input.truncated,
                "events": events.len(),
                "redactedMessages": redaction_count,
            }),
        )
        .await?;

        let evidence = evidence_refs(dream, &redacted_messages, &events);
        let summary_body = bounded_summary(
            dream,
            &redacted_messages,
            &events,
            &budgeted_input,
            self.config.max_summary_chars,
        );
        let title = summary_title(&redacted_messages);
        if self.config.summary_cadence.session_every_dream {
            let summary = self
                .store
                .upsert_memory_summary(NewMemorySummaryRecord {
                    kind: MemorySummaryKind::Session,
                    subject: dream.subject.clone(),
                    scope: dream.scope.clone(),
                    title: title.clone(),
                    body: summary_body,
                    evidence: evidence.clone(),
                    source_dream_id: dream.id,
                    source_session_id: Some(dream.session_id),
                    dedupe_key: format!("summary:session:{}", dream.session_id),
                })
                .await?;
            let session_summary_uri = summary_uri(&summary);
            sink.emit(
                "dream_progress",
                json!({
                    "dreamId": dream.id,
                    "phase": "summary_written",
                    "summaryId": summary.id,
                    "summaryUri": session_summary_uri,
                    "title": summary.title,
                }),
            )
            .await?;
        }

        let memory_proposals = dream_memory_proposals(dream, &redacted_messages, &evidence)?;
        if let Some(reflection) = write_reflection_summary_if_needed(
            self.store.as_ref(),
            dream,
            &memory_proposals,
            &evidence,
            &title,
            self.config.reflect_importance_threshold,
            self.config.max_summary_chars,
        )
        .await?
        {
            sink.emit(
                "dream_progress",
                json!({
                    "dreamId": dream.id,
                    "phase": "reflection_written",
                    "summaryId": reflection.id,
                    "summaryUri": summary_uri(&reflection),
                    "title": reflection.title,
                }),
            )
            .await?;
        }
        if self.config.summary_cadence.rollup_every_dream
            && let Some(rollup) = update_recursive_summary_rollup(
                self.store.as_ref(),
                dream,
                self.config.max_summary_chars,
            )
            .await?
        {
            sink.emit(
                "dream_progress",
                json!({
                    "dreamId": dream.id,
                    "phase": "summary_rollup_updated",
                    "summaryId": rollup.id,
                    "summaryUri": summary_uri(&rollup),
                    "title": rollup.title,
                }),
            )
            .await?;
        }

        for proposal in memory_proposals.iter().cloned() {
            spawn_memory_write_proposal(
                Arc::clone(&self.store),
                Arc::clone(&self.approval_broker),
                Arc::clone(&self.sender_for),
                dream.session_id,
                proposal,
                self.config.proposal_timeout,
            );
        }

        let mut proposal_count = memory_proposals.len();
        if let Some(skill_outcome) = dream_skill_proposal(dream, &redacted_messages, &evidence) {
            match skill_outcome {
                DreamSkillProposal::Accepted(skill_proposal) => {
                    let skill = self.store.upsert_skill_proposal(skill_proposal).await?;
                    proposal_count += 1;
                    spawn_skill_write_proposal(
                        Arc::clone(&self.store),
                        Arc::clone(&self.approval_broker),
                        Arc::clone(&self.sender_for),
                        skill,
                        self.config.proposal_timeout,
                    );
                }
                DreamSkillProposal::Rejected {
                    name,
                    scenario,
                    reason,
                    verification,
                } => {
                    sink.emit(
                        "dream_progress",
                        json!({
                            "dreamId": dream.id,
                            "phase": "skill_proposal_rejected",
                            "name": name,
                            "scenario": scenario,
                            "reason": reason,
                            "verification": verification,
                        }),
                    )
                    .await?;
                }
            }
        }

        Ok(DreamWorkerReport {
            attempted: 1,
            completed: 0,
            proposals: proposal_count,
        })
    }
}

impl<S> DreamWorkerDaemon<S> {
    pub fn new(worker: Arc<ServerDreamWorker<S>>) -> Self {
        Self { worker }
    }
}

impl<S> DreamWorkerDaemon<S>
where
    S: Store,
{
    pub fn spawn(self) -> DreamWorkerDaemonHandle {
        let (shutdown, shutdown_rx) = watch::channel(false);
        let join = tokio::spawn(async move {
            self.run_until_shutdown(shutdown_rx).await;
        });
        DreamWorkerDaemonHandle { shutdown, join }
    }

    pub async fn run_until_shutdown(&self, mut shutdown: watch::Receiver<bool>) {
        if !self.worker.config.enabled {
            return;
        }
        let poll_interval =
            chrono_duration_to_std(self.worker.config.poll_interval, StdDuration::from_secs(5));
        loop {
            if *shutdown.borrow() {
                break;
            }
            if let Err(err) = self.worker.run_batch_result().await {
                tracing::warn!(%err, "dream worker daemon tick failed");
            }
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(poll_interval) => {}
            }
        }
    }
}

impl DreamWorkerDaemonHandle {
    pub fn request_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    pub async fn shutdown(self) -> Result<()> {
        let _ = self.shutdown.send(true);
        self.join
            .await
            .map_err(|err| ServerError::Store(format!("dream worker daemon join failed: {err}")))
    }
}

#[async_trait]
impl<S> DreamWorker for ServerDreamWorker<S>
where
    S: Store,
{
    async fn run_once(&self) -> DreamWorkerReport {
        match self.run_once_result().await {
            Ok(report) => report,
            Err(err) => {
                tracing::warn!(%err, "dream worker run failed before producing a report");
                DreamWorkerReport::default()
            }
        }
    }
}

struct RedactedMessage {
    seq: i64,
    role: String,
    content: String,
    had_redactions: bool,
}

impl From<&DreamInputMessage> for RedactedMessage {
    fn from(message: &DreamInputMessage) -> Self {
        Self {
            seq: message.seq,
            role: message.role.clone(),
            content: message.content.clone(),
            had_redactions: message.had_redactions,
        }
    }
}

fn dream_started_payload(dream: &DreamQueueRecord) -> Value {
    json!({
        "dreamId": dream.id,
        "sessionId": dream.session_id,
        "reason": dream.reason,
        "status": DreamStatus::Running,
        "subject": dream.subject,
        "scope": dream.scope,
        "attempts": dream.attempts,
        "sourceEventSeq": dream.source_event_seq,
    })
}

fn chrono_duration_to_std(duration: Duration, fallback: StdDuration) -> StdDuration {
    duration.to_std().unwrap_or(fallback)
}

fn evidence_refs(
    dream: &DreamQueueRecord,
    messages: &[RedactedMessage],
    events: &[SessionEvent],
) -> Vec<MemoryEvidenceRef> {
    let mut evidence = Vec::new();
    if let Some(seq) = dream.source_event_seq {
        evidence.push(MemoryEvidenceRef {
            session_id: dream.session_id,
            event_seq: Some(seq),
            message_seq: None,
            uri: None,
            label: "session_end".to_string(),
        });
    }
    evidence.extend(messages.iter().take(8).map(|message| MemoryEvidenceRef {
        session_id: dream.session_id,
        event_seq: None,
        message_seq: Some(message.seq),
        uri: None,
        label: format!("message:{}", message.role),
    }));
    evidence.extend(
        events
            .iter()
            .filter_map(|event| {
                event
                    .artifact_uri
                    .as_ref()
                    .or(event.history_uri.as_ref())
                    .map(|uri| (event, uri))
            })
            .take(8)
            .map(|(event, uri)| MemoryEvidenceRef {
                session_id: dream.session_id,
                event_seq: Some(event.seq),
                message_seq: None,
                uri: Some(uri.clone()),
                label: event.event_type.clone(),
            }),
    );
    evidence
}

fn bounded_summary(
    dream: &DreamQueueRecord,
    messages: &[RedactedMessage],
    events: &[SessionEvent],
    input: &BudgetedDreamInput,
    max_chars: usize,
) -> String {
    let first_user = messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| preview_text(&message.content, 280))
        .unwrap_or_else(|| "No user message was captured.".to_string());
    let last_assistant = messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| preview_text(&message.content, 360))
        .unwrap_or_else(|| "No assistant final response was captured.".to_string());
    let artifacts = event_uris(events);
    let decisions = message_lines_with(messages, &["decision:", "decided", "going with"]);
    let open_loops = message_lines_with(messages, &["todo:", "open loop", "follow up", "next"]);
    let unresolved = unresolved_approvals(events);
    let mut lines = vec![
        "Session summary".to_string(),
        format!("Subject: {}", dream.subject),
        format!("Scope: {}", dream.scope),
        format!("Source session: {}", dream.session_id),
        format!(
            "Input budget: included {}/{} messages across {} chunk(s); truncated messages: {}; omitted messages: {}; included chars: {}/{}.",
            input.included_messages,
            input.total_messages,
            input.chunks.len(),
            input.truncated_messages,
            input.omitted_messages,
            input.included_chars,
            input.total_chars
        ),
        format!("What happened: {first_user}"),
        format!("Assistant result: {last_assistant}"),
        format!(
            "Shipped artifacts/resources: {}",
            if artifacts.is_empty() {
                "none observed".to_string()
            } else {
                artifacts.join(", ")
            }
        ),
        format!(
            "Decisions: {}",
            if decisions.is_empty() {
                "none explicit".to_string()
            } else {
                decisions.join(" | ")
            }
        ),
        format!(
            "Open loops: {}",
            if open_loops.is_empty() {
                "none explicit".to_string()
            } else {
                open_loops.join(" | ")
            }
        ),
        format!(
            "Unresolved approvals: {}",
            if unresolved.is_empty() {
                "none".to_string()
            } else {
                unresolved.join(", ")
            }
        ),
    ];
    lines.push(if open_loops.is_empty() && unresolved.is_empty() {
        "Next likely action: continue from the latest assistant result if Brian resumes this scope."
            .to_string()
    } else {
        "Next likely action: surface the open loops or pending approvals before taking new action."
            .to_string()
    });
    preview_text(&lines.join("\n"), max_chars)
}

fn summary_title(messages: &[RedactedMessage]) -> String {
    messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| preview_text(&message.content, 80))
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "Post-session dream summary".to_string())
}

fn dream_memory_proposals(
    dream: &DreamQueueRecord,
    messages: &[RedactedMessage],
    evidence: &[MemoryEvidenceRef],
) -> Result<Vec<MemoryWriteProposal>> {
    let mut proposals = Vec::new();
    for message in messages
        .iter()
        .filter(|message| message.role == "user" && !message.had_redactions)
    {
        let mut extracted = crate::memory::personal_assistant_state_capture_proposals(
            &dream.subject,
            &dream.scope,
            dream.session_id,
            &message.content,
            Utc::now(),
        )?;
        for proposal in &mut extracted {
            proposal.source = format!("dream:{}:session:{}", dream.id, dream.session_id);
            proposal.provenance_label = "post-session-dream".to_string();
            proposal.provenance = json!({
                "label": "post-session-dream",
                "sourceDream": dream.id,
                "sourceSession": dream.session_id,
                "sourceMessageSeq": message.seq,
                "scope": dream.scope,
                "subject": dream.subject,
                "evidence": evidence,
            });
        }
        proposals.extend(extracted);
    }
    Ok(proposals)
}

async fn write_reflection_summary_if_needed<S>(
    store: &S,
    dream: &DreamQueueRecord,
    proposals: &[MemoryWriteProposal],
    evidence: &[MemoryEvidenceRef],
    title_seed: &str,
    threshold: f32,
    max_chars: usize,
) -> Result<Option<MemorySummaryRecord>>
where
    S: Store,
{
    let cumulative_importance = proposals
        .iter()
        .map(|proposal| proposal.importance_score)
        .sum::<f32>();
    if proposals.is_empty() || cumulative_importance < threshold {
        return Ok(None);
    }

    let mut lines = vec![
        "Reflection summary".to_string(),
        format!("Scope: {}", dream.scope),
        format!("Source dream: {}", dream.id),
        format!(
            "Cumulative importance: {:.2} (threshold {:.2}).",
            cumulative_importance, threshold
        ),
        "Signals:".to_string(),
    ];
    lines.extend(proposals.iter().take(6).map(|proposal| {
        format!(
            "- {:.2} {} :: {}",
            proposal.importance_score,
            proposal.memory_kind.as_str(),
            preview_text(&proposal.text, 180)
        )
    }));
    lines.push("Evidence citations:".to_string());
    lines.extend(evidence.iter().take(6).map(evidence_citation));
    lines.push(
        "Derived reflection: preserve the durable decision/open-loop context before the next turn, \
         and prefer recalling the cited memory records over raw session logs."
            .to_string(),
    );

    let record = store
        .upsert_memory_summary(NewMemorySummaryRecord {
            kind: MemorySummaryKind::Reflection,
            subject: dream.subject.clone(),
            scope: dream.scope.clone(),
            title: format!("Reflection: {}", preview_text(title_seed, 72)),
            body: preview_text(&lines.join("\n"), max_chars),
            evidence: evidence.to_vec(),
            source_dream_id: dream.id,
            source_session_id: Some(dream.session_id),
            dedupe_key: format!("summary:reflection:{}", dream.session_id),
        })
        .await?;
    Ok(Some(record))
}

async fn update_recursive_summary_rollup<S>(
    store: &S,
    dream: &DreamQueueRecord,
    max_chars: usize,
) -> Result<Option<MemorySummaryRecord>>
where
    S: Store,
{
    let mut summaries = store.memory_summaries(&dream.scope, 12).await?;
    summaries.retain(|summary| {
        matches!(
            summary.kind,
            MemorySummaryKind::Session
                | MemorySummaryKind::Reflection
                | MemorySummaryKind::Daily
                | MemorySummaryKind::Weekly
        )
    });
    if summaries.is_empty() {
        return Ok(None);
    }
    summaries.sort_by_key(|summary| summary.updated_at);
    let folded = summaries.into_iter().rev().take(6).collect::<Vec<_>>();
    let mut chronological = folded.clone();
    chronological.reverse();

    let mut lines = vec![
        "Recursive scope summary".to_string(),
        format!("Scope: {}", dream.scope),
        format!("Folded summaries: {}.", chronological.len()),
    ];
    lines.extend(chronological.iter().map(|summary| {
        format!(
            "- [{}] {} :: {}",
            summary.kind,
            summary.title,
            preview_text(&summary.body, 260)
        )
    }));
    lines.push(
        "Next recall rule: use this rollup to recover older session context before loading raw logs."
            .to_string(),
    );
    let evidence = chronological
        .iter()
        .map(|summary| MemoryEvidenceRef {
            session_id: summary.source_session_id.unwrap_or(dream.session_id),
            event_seq: None,
            message_seq: None,
            uri: Some(summary_uri(summary)),
            label: format!("summary:{}", summary.kind),
        })
        .collect::<Vec<_>>();

    let record = store
        .upsert_memory_summary(NewMemorySummaryRecord {
            kind: MemorySummaryKind::TopicProject,
            subject: dream.subject.clone(),
            scope: dream.scope.clone(),
            title: format!("Rollup: {}", dream.scope),
            body: preview_text(&lines.join("\n"), max_chars),
            evidence,
            source_dream_id: dream.id,
            source_session_id: None,
            dedupe_key: format!("summary:rollup:{}", dream.scope),
        })
        .await?;
    Ok(Some(record))
}

enum DreamSkillProposal {
    Accepted(NewSkillProposalRecord),
    Rejected {
        name: String,
        scenario: String,
        reason: String,
        verification: SkillVerification,
    },
}

fn dream_skill_proposal(
    dream: &DreamQueueRecord,
    messages: &[RedactedMessage],
    evidence: &[MemoryEvidenceRef],
) -> Option<DreamSkillProposal> {
    let workflow_source = messages
        .iter()
        .find(|message| message.role == "user" && reusable_workflow_signal(&message.content))?;
    let scenario = preview_text(&workflow_source.content, 360);
    let name = skill_name(&scenario);
    let body = format!(
        "# {name}\n\nUse when Brian asks for the recurring workflow captured from this session.\n\n## Trigger\n{scenario}\n\n## Procedure\n- Restate the target outcome and scope.\n- Gather only missing constraints that affect the workflow.\n- Execute the smallest repeatable sequence of steps.\n- Preserve approvals for external, destructive, or sensitive actions.\n- End with the reusable result and any open loops.\n\n## Guardrails\n- Do not edit SOUL.md, mode catalogs, or capability configuration.\n- Do not install or activate automatically.\n"
    );
    let verification = verify_skill_body(&body);
    if !verification.passed {
        return Some(DreamSkillProposal::Rejected {
            name,
            scenario,
            reason: "generated skill proposal failed self-verification".to_string(),
            verification,
        });
    }
    Some(DreamSkillProposal::Accepted(NewSkillProposalRecord {
        name,
        description: "Reusable workflow distilled by a post-session dream.".to_string(),
        body,
        trigger: scenario.clone(),
        use_criteria: "Use only when the user asks for the same recurring workflow, not for one-off repo trivia.".to_string(),
        evidence: evidence.to_vec(),
        self_critique: "The proposal is intentionally narrow, cites the source session, and keeps live skill installation out of scope.".to_string(),
        verification,
        dedupe_key: format!(
            "skill:{}:{}",
            dream.session_id,
            normalize_key(&scenario)
        ),
        source_dream_id: dream.id,
        source_session_id: dream.session_id,
    }))
}

fn verify_skill_body(body: &str) -> SkillVerification {
    let checks = [
        ("has_title", body.starts_with("# ")),
        ("has_trigger", body.contains("## Trigger")),
        ("has_procedure", body.contains("## Procedure")),
        ("has_guardrails", body.contains("## Guardrails")),
        ("does_not_mutate_identity", !body.contains("write SOUL.md")),
        (
            "does_not_claim_live_reload",
            !body.contains("Install automatically") && !body.contains("Activate automatically"),
        ),
    ];
    SkillVerification {
        passed: checks.iter().all(|(_, passed)| *passed),
        checks: checks
            .into_iter()
            .map(|(name, passed)| format!("{name}:{}", if passed { "pass" } else { "fail" }))
            .collect(),
    }
}

fn spawn_memory_write_proposal<S>(
    store: Arc<S>,
    approval_broker: Arc<ApprovalBroker>,
    sender_for: SenderFactory,
    session_id: Uuid,
    proposal: MemoryWriteProposal,
    timeout: StdDuration,
) where
    S: Store,
{
    tokio::spawn(async move {
        let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
            session_id,
            Arc::clone(&store),
            sender_for(session_id),
        ));
        if let Err(err) = sink
            .emit(
                "write_proposal",
                proposal.event_payload(MemoryWriteStatus::Pending, None),
            )
            .await
        {
            tracing::warn!(%err, %session_id, "dream memory proposal event failed");
            return;
        }
        let approval = approval_broker
            .request_permission_detailed_for_backend(
                session_id,
                "memory",
                memory_write_approval_prompt(&proposal, timeout),
                timeout,
                Arc::clone(&sink),
            )
            .await;
        let Ok(approval) = approval else {
            tracing::warn!(%session_id, "dream memory approval request failed");
            return;
        };
        let status = memory_write_status_from_approval(approval.status);
        let record = if approval.status == ApprovalStatus::Approved {
            match persist_memory_write(store.as_ref(), &proposal).await {
                Ok(record) => Some(record),
                Err(err) => {
                    tracing::warn!(%err, %session_id, "dream memory persistence failed");
                    None
                }
            }
        } else {
            None
        };
        if let Err(err) = sink
            .emit(
                "write_proposal",
                proposal.event_payload(status, record.as_ref()),
            )
            .await
        {
            tracing::warn!(%err, %session_id, "dream memory proposal resolution event failed");
        }
    });
}

fn spawn_skill_write_proposal<S>(
    store: Arc<S>,
    approval_broker: Arc<ApprovalBroker>,
    sender_for: SenderFactory,
    proposal: SkillProposalRecord,
    timeout: StdDuration,
) where
    S: Store,
{
    tokio::spawn(async move {
        let session_id = proposal.source_session_id;
        let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
            session_id,
            Arc::clone(&store),
            sender_for(session_id),
        ));
        if let Err(err) = sink
            .emit(
                "write_proposal",
                skill_proposal_payload(&proposal, SkillProposalStatus::Pending),
            )
            .await
        {
            tracing::warn!(%err, %session_id, "dream skill proposal event failed");
            return;
        }
        let approval = approval_broker
            .request_permission_detailed_for_backend(
                session_id,
                "skill",
                skill_write_approval_prompt(&proposal, timeout),
                timeout,
                Arc::clone(&sink),
            )
            .await;
        let Ok(approval) = approval else {
            tracing::warn!(%session_id, "dream skill approval request failed");
            return;
        };
        let status = skill_status_from_approval(approval.status);
        let updated = store
            .update_skill_proposal_status(proposal.id, status)
            .await
            .unwrap_or(proposal);
        if let Err(err) = sink
            .emit("write_proposal", skill_proposal_payload(&updated, status))
            .await
        {
            tracing::warn!(%err, %session_id, "dream skill proposal resolution event failed");
        }
    });
}

async fn persist_memory_write<S>(
    store: &S,
    proposal: &MemoryWriteProposal,
) -> Result<MemoryRecordRef>
where
    S: Store,
{
    match proposal.memory_kind {
        crate::memory::MemoryWriteKind::ProfileFact => {
            let fact = crate::memory::profile_fact_record(proposal)?;
            store.upsert_profile_fact(fact).await?;
        }
        crate::memory::MemoryWriteKind::RecallChunk => {
            let chunk = crate::memory::recall_chunk_record(proposal)?;
            store.upsert_recall_chunk(chunk).await?;
        }
    }
    Ok(proposal.record_ref())
}

fn memory_write_approval_prompt(
    proposal: &MemoryWriteProposal,
    timeout: StdDuration,
) -> ApprovalPrompt {
    ApprovalPrompt {
        action: format!(
            "memory.write {}: {}",
            proposal.memory_kind.as_str(),
            proposal.text
        ),
        scope: json!({
            "proposal": proposal.approval_scope(),
            "timeoutMs": timeout.as_millis(),
            "source": "dream",
        }),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Save memory".to_string(),
                kind: "allow_once".to_string(),
            },
            ApprovalOption {
                option_id: "reject".to_string(),
                name: "Reject memory".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}

fn skill_write_approval_prompt(
    proposal: &SkillProposalRecord,
    timeout: StdDuration,
) -> ApprovalPrompt {
    ApprovalPrompt {
        action: format!("skill.propose {}", proposal.name),
        scope: json!({
            "kind": "skill",
            "proposalId": proposal.id,
            "name": proposal.name,
            "description": proposal.description,
            "uri": skill_proposal_uri(proposal.id),
            "timeoutMs": timeout.as_millis(),
        }),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Accept proposal".to_string(),
                kind: "allow_once".to_string(),
            },
            ApprovalOption {
                option_id: "reject".to_string(),
                name: "Reject proposal".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}

fn skill_proposal_payload(proposal: &SkillProposalRecord, status: SkillProposalStatus) -> Value {
    json!({
        "kind": "skill",
        "proposalId": proposal.id,
        "status": status,
        "name": proposal.name,
        "description": proposal.description,
        "trigger": proposal.trigger,
        "useCriteria": proposal.use_criteria,
        "selfCritique": proposal.self_critique,
        "verification": proposal.verification,
        "dedupeKey": proposal.dedupe_key,
        "sourceDreamId": proposal.source_dream_id,
        "sourceSessionId": proposal.source_session_id,
        "uri": skill_proposal_uri(proposal.id),
        "createdAt": proposal.created_at,
        "updatedAt": proposal.updated_at,
    })
}

fn memory_write_status_from_approval(status: ApprovalStatus) -> MemoryWriteStatus {
    match status {
        ApprovalStatus::Approved => MemoryWriteStatus::Approved,
        ApprovalStatus::Denied => MemoryWriteStatus::Denied,
        ApprovalStatus::TimedOut => MemoryWriteStatus::TimedOut,
        ApprovalStatus::Cancelled => MemoryWriteStatus::Cancelled,
    }
}

fn skill_status_from_approval(status: ApprovalStatus) -> SkillProposalStatus {
    match status {
        ApprovalStatus::Approved => SkillProposalStatus::Approved,
        ApprovalStatus::Denied => SkillProposalStatus::Denied,
        ApprovalStatus::TimedOut => SkillProposalStatus::TimedOut,
        ApprovalStatus::Cancelled => SkillProposalStatus::Cancelled,
    }
}

fn event_uris(events: &[SessionEvent]) -> Vec<String> {
    let mut uris = Vec::new();
    for event in events {
        if let Some(uri) = event.artifact_uri.as_ref().or(event.history_uri.as_ref()) {
            uris.push(uri.clone());
        }
        collect_uri_strings(&event.payload_json, &mut uris);
    }
    uris.sort();
    uris.dedup();
    uris.into_iter().take(8).collect()
}

fn collect_uri_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if looks_like_resource_uri(text) => out.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                collect_uri_strings(item, out);
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                collect_uri_strings(value, out);
            }
        }
        _ => {}
    }
}

fn looks_like_resource_uri(text: &str) -> bool {
    [
        "artifact://",
        "workspace://",
        "linked://",
        "project://",
        "memory://",
        "agent://",
        "history://",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
}

fn message_lines_with(messages: &[RedactedMessage], needles: &[&str]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| message.content.lines())
        .map(str::trim)
        .filter(|line| {
            let lower = line.to_lowercase();
            needles.iter().any(|needle| lower.contains(needle))
        })
        .map(|line| preview_text(line, 180))
        .take(6)
        .collect()
}

fn unresolved_approvals(events: &[SessionEvent]) -> Vec<String> {
    let mut approvals = BTreeMap::<String, String>::new();
    for event in events {
        match event.event_type.as_str() {
            "approval" => {
                if let Some(id) = event.payload_json.get("approvalId").and_then(Value::as_str) {
                    approvals.insert(
                        id.to_string(),
                        event
                            .payload_json
                            .get("action")
                            .and_then(Value::as_str)
                            .unwrap_or("approval")
                            .to_string(),
                    );
                }
            }
            "approval_resolved" => {
                if let Some(id) = event.payload_json.get("approvalId").and_then(Value::as_str) {
                    approvals.remove(id);
                }
            }
            _ => {}
        }
    }
    approvals.into_values().take(6).collect()
}

fn reusable_workflow_signal(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "workflow:",
        "playbook:",
        "reusable workflow",
        "reuse this workflow",
        "my process is",
        "our process is",
        "when i ask",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn skill_name(scenario: &str) -> String {
    let normalized = normalize_key(scenario);
    let suffix = normalized
        .split('-')
        .filter(|part| !part.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join("-");
    if suffix.is_empty() {
        "dream-distilled-workflow".to_string()
    } else {
        format!("dream-{suffix}")
    }
}

fn normalize_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn preview_text(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut out = compact
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

fn evidence_citation(evidence: &MemoryEvidenceRef) -> String {
    let mut parts = vec![format!("session {}", evidence.session_id)];
    if let Some(seq) = evidence.event_seq {
        parts.push(format!("event {seq}"));
    }
    if let Some(seq) = evidence.message_seq {
        parts.push(format!("message {seq}"));
    }
    if let Some(uri) = evidence.uri.as_ref() {
        parts.push(uri.clone());
    }
    format!("- {} ({})", evidence.label, parts.join(", "))
}

fn summary_uri(summary: &MemorySummaryRecord) -> String {
    format!("memory://summaries/{}", summary.id)
}

fn skill_proposal_uri(id: Uuid) -> String {
    format!("memory://skill-proposals/{id}")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use tm_memory::{DreamReason, NewDreamQueueRecord};
    use tm_modes::{AssetStatus, ModeId, ModesConfig};

    use super::*;
    use crate::{
        ApprovalResolveDecision, CronJobRecord, CronRunRecord, InMemoryStore, MessageRecord,
        NewCronJobRecord, NewCronRunRecord, NewProjectItem, NewSession, ProfileFactRecord,
        ProjectItemKind, ProjectItemRecord, RecallChunkRecord, ResolveApprovalRequest,
        SessionRecord, SessionSummaryRecord,
    };

    struct ClaimFailureStore;

    #[async_trait]
    impl Store for ClaimFailureStore {
        async fn create_session(&self, _new: NewSession) -> Result<SessionRecord> {
            panic!("unused store method create_session")
        }

        async fn end_session(&self, _session_id: Uuid) -> Result<SessionRecord> {
            panic!("unused store method end_session")
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

        async fn add_profile_fact(&self, _fact: ProfileFactRecord) -> Result<()> {
            panic!("unused store method add_profile_fact")
        }

        async fn add_recall_chunk(&self, _chunk: RecallChunkRecord) -> Result<()> {
            panic!("unused store method add_recall_chunk")
        }

        async fn upsert_profile_fact(&self, _fact: ProfileFactRecord) -> Result<ProfileFactRecord> {
            panic!("unused store method upsert_profile_fact")
        }

        async fn upsert_recall_chunk(
            &self,
            _chunk: RecallChunkRecord,
        ) -> Result<RecallChunkRecord> {
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

        async fn dream_queue_for_session(
            &self,
            _session_id: Uuid,
        ) -> Result<Vec<DreamQueueRecord>> {
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
        ) -> Result<Option<DreamQueueRecord>> {
            Err(ServerError::Store("claim failed".to_string()))
        }

        async fn heartbeat_dream(
            &self,
            _dream_id: Uuid,
            _now: chrono::DateTime<Utc>,
        ) -> Result<DreamQueueRecord> {
            panic!("unused store method heartbeat_dream")
        }

        async fn complete_dream(
            &self,
            _dream_id: Uuid,
            _now: chrono::DateTime<Utc>,
        ) -> Result<DreamQueueRecord> {
            panic!("unused store method complete_dream")
        }

        async fn fail_dream(
            &self,
            _dream_id: Uuid,
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

        async fn claim_cron_run(&self, _run: NewCronRunRecord) -> Result<(CronRunRecord, bool)> {
            panic!("unused store method claim_cron_run")
        }

        async fn record_cron_run(&self, _run: NewCronRunRecord) -> Result<CronRunRecord> {
            panic!("unused store method record_cron_run")
        }

        async fn complete_cron_run(
            &self,
            _run_id: Uuid,
            _status: &str,
            _session_id: Option<Uuid>,
            _result_json: Value,
        ) -> Result<CronRunRecord> {
            panic!("unused store method complete_cron_run")
        }

        async fn cron_runs(&self, _job_id: &str, _limit: usize) -> Result<Vec<CronRunRecord>> {
            panic!("unused store method cron_runs")
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

    async fn wait_for_event_count(
        store: &InMemoryStore,
        session_id: Uuid,
        event_type: &str,
        count: usize,
    ) -> Vec<SessionEvent> {
        for _ in 0..100 {
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

        let worker = ServerDreamWorker::new(
            Arc::clone(&store),
            Arc::clone(&broker),
            test_sender_factory(),
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
        broker
            .resolve(
                session.id,
                approval_id,
                ResolveApprovalRequest {
                    decision: ApprovalResolveDecision::Approve,
                    option_id: Some("allow".to_string()),
                },
            )
            .unwrap();
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

            let worker = ServerDreamWorker::new(
                Arc::clone(&store),
                Arc::clone(&broker),
                test_sender_factory(),
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
                broker
                    .resolve(
                        session.id,
                        approval_id,
                        ResolveApprovalRequest {
                            decision,
                            option_id: Some(option_id.to_string()),
                        },
                    )
                    .unwrap();
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
                    event.event_type == "write_proposal"
                        && event.payload_json["kind"] == json!("skill")
                })
                .count(),
            2
        );
    }
}

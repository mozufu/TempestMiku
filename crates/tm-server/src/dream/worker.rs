use std::sync::Arc;
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use futures::future::join_all;
use serde_json::{Value, json};
use tokio::{
    sync::{broadcast, watch},
    task::JoinHandle,
};
use uuid::Uuid;

use tm_memory::{
    DreamInputMessage, DreamQueueRecord, DreamStatus, DreamWorker, DreamWorkerReport,
    MemorySummaryKind, NewMemorySummaryRecord, redact_dream_text,
};

use crate::{CodingEventSink, Result, ServerError, SessionEvent, Store, StoreCodingEventSink};

use super::config::DreamWorkerConfig;
use super::proposals::{
    DreamSkillProposal, dream_skill_proposal, spawn_memory_write_proposal,
    spawn_skill_write_proposal,
};
use super::summary::{
    bounded_summary, dream_memory_proposals, evidence_refs, summary_title, summary_uri,
    update_recursive_summary_rollup, write_reflection_summary_if_needed,
};
use super::util::RedactedMessage;

pub(super) type SenderFactory = Arc<dyn Fn(Uuid) -> broadcast::Sender<SessionEvent> + Send + Sync>;

pub struct ServerDreamWorker<S> {
    store: Arc<S>,
    approval_broker: Arc<crate::ApprovalBroker>,
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
        approval_broker: Arc<crate::ApprovalBroker>,
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

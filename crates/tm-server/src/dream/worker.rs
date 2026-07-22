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

use tm_host::{EvolutionTargetClass, SelfEvolutionTier};
use tm_memory::{
    DreamInputMessage, DreamQueueRecord, DreamStatus, DreamWorker, DreamWorkerReport,
    MemorySummaryKind, NewDreamQueueRecord, NewMemorySummaryRecord, redact_dream_text,
};

use crate::{CodingEventSink, Result, ServerError, SessionEvent, Store, StoreCodingEventSink};

use super::config::DreamWorkerConfig;
use super::evolution::{capture_episodes, update_environment, update_policies, value_episodes};
use super::proposals::{
    DreamSkillProposal, MemoryProposalContext, dream_skill_proposal, spawn_memory_write_proposal,
    spawn_skill_write_proposal,
};
use super::summary::{
    bounded_summary, dream_memory_proposals, evidence_refs, summary_title, summary_uri,
    update_recursive_summary_rollup, write_reflection_summary_if_needed,
};
use super::util::RedactedMessage;

pub type SenderFactory = Arc<dyn Fn(Uuid) -> broadcast::Sender<SessionEvent> + Send + Sync>;

pub struct ServerDreamWorker<S> {
    store: Arc<S>,
    approval_broker: Arc<crate::ApprovalBroker>,
    sender_for: SenderFactory,
    config: DreamWorkerConfig,
    self_evolution_tier: SelfEvolutionTier,
    owner_id: Uuid,
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
    ) -> Self
    where
        S: Store,
    {
        approval_broker.bind_store(Arc::clone(&store));
        Self {
            store,
            approval_broker,
            sender_for,
            config,
            self_evolution_tier: SelfEvolutionTier::default(),
            owner_id: Uuid::new_v4(),
        }
    }

    pub fn with_self_evolution_tier(mut self, tier: SelfEvolutionTier) -> Self {
        self.self_evolution_tier = tier;
        self
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
        let Some(lease) = self
            .store
            .claim_ready_dream_bounded(
                now,
                self.config.lease_timeout,
                self.owner_id,
                self.config.max_attempts,
            )
            .await?
        else {
            return Ok(DreamWorkerReport::default());
        };
        let dream = &lease.dream;
        let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
            dream.session_id,
            Arc::clone(&self.store),
            (self.sender_for)(dream.session_id),
        ));
        sink.emit("dream_started", dream_started_payload(dream))
            .await?;

        let processed = if self.config.per_dream_timeout.is_zero() {
            Err(ServerError::Store("dream timed out".to_string()))
        } else {
            let heartbeat_every = self
                .config
                .heartbeat_interval
                .max(StdDuration::from_millis(1));
            let processing = self.process_claimed_dream(dream, Arc::clone(&sink));
            tokio::pin!(processing);
            let mut heartbeat = tokio::time::interval(heartbeat_every);
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            heartbeat.tick().await;
            let processed_with_heartbeat = async {
                loop {
                    tokio::select! {
                        processed = &mut processing => break processed,
                        _ = heartbeat.tick() => {
                            self.store.heartbeat_dream(&lease, Utc::now()).await?;
                        }
                    }
                }
            };
            match tokio::time::timeout(self.config.per_dream_timeout, processed_with_heartbeat)
                .await
            {
                Ok(processed) => processed,
                Err(_) => Err(ServerError::Store("dream timed out".to_string())),
            }
        };

        match processed {
            Ok(mut report) => {
                let completed = self.store.complete_dream(&lease, Utc::now()).await?;
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
                        &lease,
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

        let episodes = capture_episodes(
            &self.store,
            &self.config.evolution,
            dream,
            &events,
            sink.as_ref(),
        )
        .await?;
        match value_episodes(
            &self.store,
            &self.config.evolution,
            dream,
            &episodes,
            &events,
            sink.as_ref(),
        )
        .await
        {
            Ok(mut valued) => {
                let valued_ids = valued
                    .iter()
                    .map(|episode| episode.id)
                    .collect::<std::collections::BTreeSet<_>>();
                valued.extend(
                    episodes
                        .iter()
                        .filter(|episode| {
                            matches!(
                                episode.status,
                                tm_memory::EpisodeStatus::Valued
                                    | tm_memory::EpisodeStatus::Evolved
                            ) && !valued_ids.contains(&episode.id)
                        })
                        .cloned(),
                );
                match update_policies(
                    &self.store,
                    &self.config.evolution,
                    dream,
                    &valued,
                    sink.as_ref(),
                )
                .await
                {
                    Ok(_) => {
                        if let Err(error) = update_environment(
                            &self.store,
                            &self.config.evolution,
                            dream,
                            sink.as_ref(),
                        )
                        .await
                        {
                            let reason = redact_dream_text(&error.to_string()).text;
                            tracing::warn!(%reason, "dream environment cognition update skipped");
                            self.store
                                .enqueue_dream(NewDreamQueueRecord {
                                    session_id: dream.session_id,
                                    subject: dream.subject.clone(),
                                    scope: dream.scope.clone(),
                                    reason: dream.reason,
                                    dedupe_key: format!(
                                        "dream:evolution-environment:{}:{}",
                                        dream.session_id, dream.id
                                    ),
                                    source_event_seq: dream.source_event_seq,
                                    available_at: Utc::now() + self.config.retry_backoff,
                                })
                                .await?;
                            sink.emit(
                                "dream_progress",
                                json!({
                                    "dreamId": dream.id,
                                    "phase": "evolution_skipped",
                                    "reason": reason,
                                }),
                            )
                            .await?;
                        }
                    }
                    Err(error) => {
                        let reason = redact_dream_text(&error.to_string()).text;
                        tracing::warn!(%reason, "dream evolution policy update skipped");
                        self.store
                            .enqueue_dream(NewDreamQueueRecord {
                                session_id: dream.session_id,
                                subject: dream.subject.clone(),
                                scope: dream.scope.clone(),
                                reason: dream.reason,
                                dedupe_key: format!(
                                    "dream:evolution-policies:{}",
                                    dream.session_id
                                ),
                                source_event_seq: dream.source_event_seq,
                                available_at: Utc::now() + self.config.retry_backoff,
                            })
                            .await?;
                        sink.emit(
                            "dream_progress",
                            json!({
                                "dreamId": dream.id,
                                "phase": "evolution_skipped",
                                "reason": reason,
                            }),
                        )
                        .await?;
                    }
                }
            }
            Err(error) => {
                let reason = redact_dream_text(&error.to_string()).text;
                tracing::warn!(%reason, "dream evolution valuation skipped");
                self.store
                    .enqueue_dream(NewDreamQueueRecord {
                        session_id: dream.session_id,
                        subject: dream.subject.clone(),
                        scope: dream.scope.clone(),
                        reason: dream.reason,
                        dedupe_key: format!("dream:evolution-valuation:{}", dream.session_id),
                        source_event_seq: dream.source_event_seq,
                        available_at: Utc::now() + self.config.retry_backoff,
                    })
                    .await?;
                sink.emit(
                    "dream_progress",
                    json!({
                        "dreamId": dream.id,
                        "phase": "evolution_skipped",
                        "reason": reason,
                    }),
                )
                .await?;
            }
        }

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

        let mut proposal_count = 0;
        for proposal in memory_proposals.iter().cloned() {
            let target = crate::evolution::memory_target_class(proposal.memory_kind);
            if let Err(error) = crate::evolution::ensure_evolution_proposal_reachable(
                self.self_evolution_tier,
                target,
            ) {
                let record = crate::evolution::denied_evolution_audit_record(
                    crate::evolution::DeniedEvolutionAuditSpec {
                        tier: self.self_evolution_tier,
                        target_class: target,
                        target_id: proposal.proposal_id.to_string(),
                        actor_id: "dream-worker".to_string(),
                        session_id: dream.session_id,
                        dream_id: Some(dream.id),
                        content: &proposal,
                        occurred_at: Utc::now(),
                    },
                )?;
                self.store
                    .append_evolution_audit(crate::EvolutionAuditEntry {
                        idempotency_key: format!("proposal:{}:denied", proposal.proposal_id),
                        record,
                    })
                    .await?;
                sink.emit(
                    "dream_progress",
                    json!({
                        "dreamId": dream.id,
                        "phase": "evolution_denied",
                        "targetClass": target,
                        "reason": error.to_string(),
                    }),
                )
                .await?;
                continue;
            }
            spawn_memory_write_proposal(
                Arc::clone(&self.store),
                Arc::clone(&self.approval_broker),
                Arc::clone(&self.sender_for),
                proposal,
                self.config.proposal_timeout,
                MemoryProposalContext {
                    session_id: dream.session_id,
                    dream_id: dream.id,
                    self_evolution_tier: self.self_evolution_tier,
                },
            )
            .await?;
            proposal_count += 1;
        }

        if let Some(skill_outcome) = dream_skill_proposal(dream, &redacted_messages, &evidence) {
            match skill_outcome {
                DreamSkillProposal::Accepted(skill_proposal) => {
                    if let Err(error) = crate::evolution::ensure_evolution_proposal_reachable(
                        self.self_evolution_tier,
                        EvolutionTargetClass::SkillProposal,
                    ) {
                        let proposal_id = crate::evolution::deterministic_evolution_proposal_id(
                            "skill",
                            &skill_proposal.dedupe_key,
                        );
                        let record = crate::evolution::denied_evolution_audit_record(
                            crate::evolution::DeniedEvolutionAuditSpec {
                                tier: self.self_evolution_tier,
                                target_class: EvolutionTargetClass::SkillProposal,
                                target_id: proposal_id.to_string(),
                                actor_id: "dream-worker".to_string(),
                                session_id: dream.session_id,
                                dream_id: Some(dream.id),
                                content: &skill_proposal,
                                occurred_at: Utc::now(),
                            },
                        )?;
                        self.store
                            .append_evolution_audit(crate::EvolutionAuditEntry {
                                idempotency_key: format!("proposal:{proposal_id}:denied"),
                                record,
                            })
                            .await?;
                        sink.emit(
                            "dream_progress",
                            json!({
                                "dreamId": dream.id,
                                "phase": "evolution_denied",
                                "targetClass": EvolutionTargetClass::SkillProposal,
                                "reason": error.to_string(),
                            }),
                        )
                        .await?;
                        return Ok(DreamWorkerReport {
                            attempted: 1,
                            completed: 0,
                            proposals: proposal_count,
                        });
                    }
                    let skill = self.store.upsert_skill_proposal(skill_proposal).await?;
                    proposal_count += 1;
                    spawn_skill_write_proposal(
                        Arc::clone(&self.store),
                        Arc::clone(&self.approval_broker),
                        Arc::clone(&self.sender_for),
                        skill,
                        self.config.proposal_timeout,
                        self.self_evolution_tier,
                    )
                    .await?;
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
                let err = tm_memory::redact_dream_text(&err.to_string()).text;
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
                let err = tm_memory::redact_dream_text(&err.to_string()).text;
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

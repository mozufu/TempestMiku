use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::api::AppState;
use crate::api::modes::{build_turn_prompt, mode_changed_payload, mode_profile};
use crate::{
    ChatRunLimits, ChatRunner, ChatTurn, CronJobRecord, CronLease, MemoryProvider,
    NewCronJobRecord, NewCronRunRecord, NewSession, PersistingEventSink, Result, ServerError,
    Store,
};

use super::{
    CronSchedule, SchedulerBounds, WEEKLY_SHIP_LEDGER_JOB_ID, WEEKLY_SHIP_LEDGER_SCHEDULE,
};

const CRON_LEASE_TIMEOUT: Duration = Duration::seconds(60);
const CRON_HEARTBEAT_INTERVAL: StdDuration = StdDuration::from_secs(15);

pub fn weekly_ship_ledger_job(now: DateTime<Utc>) -> Result<NewCronJobRecord> {
    let schedule = CronSchedule::parse(WEEKLY_SHIP_LEDGER_SCHEDULE)?;
    let bounds = SchedulerBounds::default();
    Ok(NewCronJobRecord {
        id: WEEKLY_SHIP_LEDGER_JOB_ID.to_string(),
        name: "Weekly ship ledger".to_string(),
        schedule: WEEKLY_SHIP_LEDGER_SCHEDULE.to_string(),
        enabled: true,
        cron_mode: bounds.cron_mode,
        max_turns: bounds.max_turns,
        script_timeout_seconds: bounds.script_timeout_seconds,
        next_run_at: schedule.next_after(now),
    })
}

pub async fn ensure_weekly_ship_ledger_job<S>(
    store: &S,
    now: DateTime<Utc>,
) -> Result<crate::CronJobRecord>
where
    S: Store,
{
    store.upsert_cron_job(weekly_ship_ledger_job(now)?).await
}

pub async fn trigger_weekly_ship_ledger<S, M, C>(
    state: &AppState<S, M, C>,
    scheduled_for: DateTime<Utc>,
) -> Result<crate::CronRunRecord>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let job = ensure_weekly_ship_ledger_job(state.store.as_ref(), Utc::now()).await?;
    validate_scheduler_job(&job)?;

    let owner_id = Uuid::new_v4();
    let (lease, claimed) = state
        .store
        .claim_cron_run(
            NewCronRunRecord {
                job_id: job.id.clone(),
                scheduled_for,
                status: "running".to_string(),
                session_id: None,
                result_json: json!({
                    "jobId": job.id,
                    "bounds": bounds_json(&job),
                }),
            },
            owner_id,
            Utc::now(),
            CRON_LEASE_TIMEOUT,
        )
        .await?;
    if !claimed {
        return Ok(lease.run);
    }

    execute_weekly_ship_ledger_lease(state, &job, lease).await
}

pub async fn materialize_due_cron_runs<S>(
    store: &S,
    now: DateTime<Utc>,
    max_catch_up: usize,
) -> Result<usize>
where
    S: Store,
{
    if max_catch_up == 0 {
        return Ok(0);
    }
    let mut materialized = 0;
    for job in store.cron_jobs().await? {
        if !job.enabled {
            continue;
        }
        validate_scheduler_job(&job)?;
        let schedule = CronSchedule::parse(&job.schedule)?;
        let Some(mut cursor) = job.next_run_at else {
            continue;
        };
        while cursor <= now && materialized < max_catch_up {
            let next_run_at = schedule.next_after(cursor).ok_or_else(|| {
                ServerError::Store(format!("cron job {} has no next fire", job.id))
            })?;
            let run = NewCronRunRecord {
                job_id: job.id.clone(),
                scheduled_for: cursor,
                status: "queued".to_string(),
                session_id: None,
                result_json: json!({
                    "jobId": job.id,
                    "bounds": bounds_json(&job),
                }),
            };
            if store
                .materialize_cron_run(run, cursor, next_run_at)
                .await?
                .is_none()
            {
                break;
            }
            materialized += 1;
            cursor = next_run_at;
        }
        if materialized >= max_catch_up {
            break;
        }
    }
    Ok(materialized)
}

pub(crate) async fn run_scheduler_daemon<S, M, C>(
    state: AppState<S, M, C>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    poll_interval: StdDuration,
) where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    if let Err(error) = ensure_weekly_ship_ledger_job(state.store.as_ref(), Utc::now()).await {
        let error = tm_memory::redact_dream_text(&error.to_string()).text;
        tracing::error!(%error, "scheduler job registration failed");
        return;
    }
    let owner_id = Uuid::new_v4();
    loop {
        if *shutdown.borrow() {
            return;
        }
        if let Err(error) = run_scheduler_tick(&state, owner_id, Utc::now(), &shutdown).await {
            let error = tm_memory::redact_dream_text(&error.to_string()).text;
            tracing::warn!(%error, "scheduler tick failed");
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            _ = tokio::time::sleep(poll_interval) => {}
        }
    }
}

async fn run_scheduler_tick<S, M, C>(
    state: &AppState<S, M, C>,
    owner_id: Uuid,
    now: DateTime<Utc>,
    shutdown: &tokio::sync::watch::Receiver<bool>,
) -> Result<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    materialize_due_cron_runs(
        state.store.as_ref(),
        now,
        SchedulerBounds::default().max_catch_up,
    )
    .await?;

    for _ in 0..8 {
        if *shutdown.borrow() {
            break;
        }
        let Some(lease) = state
            .store
            .claim_ready_cron_run(owner_id, Utc::now(), CRON_LEASE_TIMEOUT, 3)
            .await?
        else {
            break;
        };
        if lease.epoch > 1 {
            state.runtime_status().record_lease_reclaim();
        }
        let job = state.store.cron_job(&lease.run.job_id).await?;
        let result = match job.id.as_str() {
            WEEKLY_SHIP_LEDGER_JOB_ID => {
                execute_weekly_ship_ledger_lease(state, &job, lease.clone()).await
            }
            other => Err(ServerError::Policy(format!(
                "scheduler has no executor for job {other}"
            ))),
        };
        if let Err(error) = result {
            let _ = state
                .store
                .fail_cron_run(
                    &lease,
                    error.to_string(),
                    Utc::now() + Duration::minutes(5),
                    3,
                )
                .await;
            let error = tm_memory::redact_dream_text(&error.to_string()).text;
            tracing::warn!(run_id = %lease.run.id, job_id = %lease.run.job_id, %error, "cron run failed");
        }
    }
    Ok(())
}

async fn execute_weekly_ship_ledger_lease<S, M, C>(
    state: &AppState<S, M, C>,
    job: &CronJobRecord,
    lease: CronLease,
) -> Result<crate::CronRunRecord>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    validate_scheduler_job(job)?;
    let scheduled_for = lease.run.scheduled_for;

    let assets = state.persona.load_assets();
    let mode = assets.modes.default_mode();
    let persona_status = assets.status.clone();
    let session = state
        .store
        .create_session(NewSession {
            mode: mode.clone(),
            persona_status: persona_status.clone(),
        })
        .await?;
    let profile = mode_profile(&state.persona, &session.mode_state.mode);
    let mode_event = state
        .store
        .append_event(
            session.id,
            "mode",
            mode_changed_payload(None, &session.mode_state, persona_status, &profile)?,
        )
        .await?;
    let _ = state.sender(session.id).send(mode_event);
    let started = state
        .store
        .append_event(
            session.id,
            "cron_run_started",
            json!({
                "jobId": job.id,
                "runId": lease.run.id,
                "scheduledFor": scheduled_for,
                "cronMode": job.cron_mode,
                "maxTurns": job.max_turns,
                "scriptTimeoutSeconds": job.script_timeout_seconds,
            }),
        )
        .await?;
    let _ = state.sender(session.id).send(started);

    let prompt_text = "Run the weekly ship ledger. Count small-but-real shipped work, summarize open loops, and defer any approval-needed action because cron_mode is deny.";
    state
        .store
        .append_message(session.id, "user", prompt_text)
        .await?;
    let composed = build_turn_prompt(state, &mode, prompt_text);
    let sink = Arc::new(PersistingEventSink::new(
        session.id,
        Arc::clone(&state.store),
        state.sender(session.id),
    ));
    let turn = state.chat.run_turn(
        ChatTurn {
            session_id: session.id,
            durable_turn_id: None,
            user_prompt: prompt_text.to_string(),
            mode,
            owner_subject: session.owner_subject.clone(),
            project_id: None,
            memory_scope: "global".to_string(),
            system_prompt: composed.system_prompt,
            // Scheduler authority is fixed by code. Mode capabilities and prompt text cannot
            // expand a background run's grants.
            capabilities: Vec::new(),
            prior_messages: Vec::new(),
            // Background scheduler runs never receive user-model dialectic synthesis.
            dialectic: None,
            limits: ChatRunLimits {
                max_turns: Some(job.max_turns.min(8) as usize),
                cell_wall_ms: Some(job.script_timeout_seconds.min(120) as u64 * 1_000),
            },
            deny_approvals: true,
            host_functions: Vec::new(),
        },
        sink.clone(),
    );
    tokio::pin!(turn);
    let mut heartbeat = tokio::time::interval(CRON_HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let turn_with_heartbeat = async {
        loop {
            tokio::select! {
                result = &mut turn => break result,
                _ = heartbeat.tick() => {
                    if let Err(error) = state.store.heartbeat_cron_run(&lease, Utc::now()).await {
                        state.runtime_status().record_heartbeat_failure();
                        return Err(error);
                    }
                }
            }
        }
    };
    let final_text = match tokio::time::timeout(
        StdDuration::from_secs(job.script_timeout_seconds as u64),
        turn_with_heartbeat,
    )
    .await
    {
        Ok(Ok(final_text)) => final_text,
        Ok(Err(err)) => {
            state
                .store
                .fail_cron_run(
                    &lease,
                    err.to_string(),
                    Utc::now() + Duration::minutes(5),
                    3,
                )
                .await?;
            return Err(err);
        }
        Err(_) => {
            let error = format!(
                "weekly ship ledger exceeded {} second timeout",
                job.script_timeout_seconds
            );
            state
                .store
                .fail_cron_run(&lease, error.clone(), Utc::now() + Duration::minutes(5), 3)
                .await?;
            return Err(ServerError::Store(error));
        }
    };
    sink.flush().await?;
    state
        .store
        .append_message(session.id, "assistant", &final_text)
        .await?;
    let completed = state
        .store
        .append_event(
            session.id,
            "cron_run_completed",
            json!({
                "jobId": job.id,
                "runId": lease.run.id,
                "sessionId": session.id,
                "status": "completed",
                "approvalPolicy": "defer",
            }),
        )
        .await?;
    let _ = state.sender(session.id).send(completed);

    state
        .store
        .complete_cron_run(
            &lease,
            "completed",
            Some(session.id),
            json!({
                "jobId": job.id,
                "sessionId": session.id,
                "finalText": final_text,
                "bounds": bounds_json(job),
            }),
        )
        .await
}

fn validate_scheduler_job(job: &CronJobRecord) -> Result<()> {
    if job.cron_mode != "deny"
        || !(1..=8).contains(&job.max_turns)
        || !(1..=120).contains(&job.script_timeout_seconds)
    {
        return Err(ServerError::Policy(format!(
            "weekly ship ledger bounds invalid: cron_mode={}, max_turns={}, script_timeout_seconds={}",
            job.cron_mode, job.max_turns, job.script_timeout_seconds
        )));
    }
    Ok(())
}

fn bounds_json(job: &crate::CronJobRecord) -> serde_json::Value {
    json!({
        "cronMode": job.cron_mode,
        "maxTurns": job.max_turns,
        "scriptTimeoutSeconds": job.script_timeout_seconds,
        "maxCatchUp": SchedulerBounds::default().max_catch_up,
    })
}

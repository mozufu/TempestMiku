use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::api::AppState;
use crate::api::modes::{build_turn_prompt, mode_changed_payload, mode_profile};
use crate::{
    ChatRunLimits, ChatRunner, ChatTurn, CronJobRecord, CronLease, MemoryProvider,
    NewCronJobRecord, NewCronRunRecord, NewSession, PersistingEventSink, Result, ServerError,
    Store,
};

pub const WEEKLY_SHIP_LEDGER_JOB_ID: &str = "weekly-ship-ledger";
pub const WEEKLY_SHIP_LEDGER_SCHEDULE: &str = "0 9 * * 1";
const CRON_LEASE_TIMEOUT: Duration = Duration::seconds(60);
const CRON_HEARTBEAT_INTERVAL: StdDuration = StdDuration::from_secs(15);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerBounds {
    pub cron_mode: String,
    pub max_turns: i32,
    pub script_timeout_seconds: i32,
    pub max_catch_up: usize,
}

impl Default for SchedulerBounds {
    fn default() -> Self {
        Self {
            cron_mode: "deny".to_string(),
            max_turns: 8,
            script_timeout_seconds: 120,
            max_catch_up: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    minute: CronField,
    hour: CronField,
    day_of_month: CronField,
    month: CronField,
    day_of_week: CronField,
}

impl CronSchedule {
    pub fn parse(value: &str) -> Result<Self> {
        let parts = value.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 5 {
            return Err(ServerError::InvalidRequest(format!(
                "cron schedule must have five fields: {value}"
            )));
        }
        Ok(Self {
            minute: CronField::parse(parts[0], 0, 59)?,
            hour: CronField::parse(parts[1], 0, 23)?,
            day_of_month: CronField::parse(parts[2], 1, 31)?,
            month: CronField::parse(parts[3], 1, 12)?,
            day_of_week: CronField::parse(parts[4], 0, 7)?,
        })
    }

    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut candidate = after + Duration::minutes(1);
        candidate = candidate
            .with_second(0)
            .and_then(|time| time.with_nanosecond(0))?;
        for _ in 0..(366 * 24 * 60) {
            if self.matches(candidate) {
                return Some(candidate);
            }
            candidate += Duration::minutes(1);
        }
        None
    }

    pub fn matches(&self, time: DateTime<Utc>) -> bool {
        self.minute.matches(time.minute())
            && self.hour.matches(time.hour())
            && self.day_of_month.matches(time.day())
            && self.month.matches(time.month())
            && self.day_of_week.matches(day_of_week_value(time))
    }
}

pub fn missed_fire_times(
    schedule: &str,
    after: DateTime<Utc>,
    now: DateTime<Utc>,
    max_catch_up: usize,
) -> Result<Vec<DateTime<Utc>>> {
    if max_catch_up == 0 || now <= after {
        return Ok(Vec::new());
    }
    let schedule = CronSchedule::parse(schedule)?;
    let mut cursor = after;
    let mut fires = Vec::new();
    while fires.len() < max_catch_up {
        let Some(next) = schedule.next_after(cursor) else {
            break;
        };
        if next > now {
            break;
        }
        fires.push(next);
        cursor = next;
    }
    Ok(fires)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CronField {
    Any,
    Values(BTreeSet<u32>),
}

impl CronField {
    fn parse(value: &str, min: u32, max: u32) -> Result<Self> {
        if value == "*" {
            return Ok(Self::Any);
        }
        let mut values = BTreeSet::new();
        for part in value.split(',') {
            let parsed = part.parse::<u32>().map_err(|_| {
                ServerError::InvalidRequest(format!("invalid cron field value {part}"))
            })?;
            if parsed < min || parsed > max {
                return Err(ServerError::InvalidRequest(format!(
                    "cron field value {parsed} out of range {min}-{max}"
                )));
            }
            values.insert(if max == 7 && parsed == 7 { 0 } else { parsed });
        }
        Ok(Self::Values(values))
    }

    fn matches(&self, value: u32) -> bool {
        match self {
            Self::Any => true,
            Self::Values(values) => values.contains(&value),
        }
    }
}

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
            user_prompt: prompt_text.to_string(),
            mode,
            scope: profile.default_scope,
            system_prompt: composed.system_prompt,
            // Scheduler authority is fixed by code. Mode capabilities and prompt text cannot
            // expand a background run's grants.
            capabilities: Vec::new(),
            prior_messages: Vec::new(),
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

fn day_of_week_value(time: DateTime<Utc>) -> u32 {
    time.weekday().num_days_from_sunday()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::TimeZone;
    use parking_lot::Mutex;
    use tm_core::EventSink;

    use super::*;
    use crate::{AppState, AuthConfig, EchoChatRunner, InMemoryStore, StoreMemoryProvider};

    type ObservedBounds = (Vec<String>, bool, ChatRunLimits, usize);

    #[derive(Default)]
    struct BoundsRecordingRunner {
        observed: Mutex<Option<ObservedBounds>>,
    }

    #[async_trait]
    impl ChatRunner for BoundsRecordingRunner {
        async fn run_turn(
            &self,
            turn: ChatTurn,
            _sink: Arc<dyn EventSink + Send + Sync>,
        ) -> Result<String> {
            *self.observed.lock() = Some((
                turn.capabilities,
                turn.deny_approvals,
                turn.limits,
                turn.host_functions.len(),
            ));
            Ok("bounded scheduler result".to_string())
        }
    }

    #[test]
    fn cron_schedule_calculates_next_weekly_run() {
        let schedule = CronSchedule::parse(WEEKLY_SHIP_LEDGER_SCHEDULE).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 7, 8, 10, 15, 0).unwrap();

        assert_eq!(
            schedule.next_after(after),
            Some(Utc.with_ymd_and_hms(2026, 7, 13, 9, 0, 0).unwrap())
        );
    }

    #[test]
    fn cron_schedule_rejects_invalid_fields() {
        assert!(CronSchedule::parse("* * *").is_err());
        assert!(CronSchedule::parse("99 * * * *").is_err());
    }

    #[test]
    fn missed_fire_policy_is_bounded_and_uses_schedule_cursor() {
        let after = Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 7, 29, 12, 0, 0).unwrap();

        assert_eq!(
            missed_fire_times(WEEKLY_SHIP_LEDGER_SCHEDULE, after, now, 2).unwrap(),
            vec![
                Utc.with_ymd_and_hms(2026, 7, 6, 9, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 7, 13, 9, 0, 0).unwrap(),
            ]
        );
        assert_eq!(
            missed_fire_times(WEEKLY_SHIP_LEDGER_SCHEDULE, after, now, 1).unwrap(),
            vec![Utc.with_ymd_and_hms(2026, 7, 6, 9, 0, 0).unwrap()]
        );
        assert!(
            missed_fire_times(WEEKLY_SHIP_LEDGER_SCHEDULE, now, after, 1)
                .unwrap()
                .is_empty()
        );
        assert!(
            missed_fire_times(WEEKLY_SHIP_LEDGER_SCHEDULE, after, now, 0)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn concurrent_materializers_create_one_fire_and_advance_cursor_once() {
        let store = Arc::new(InMemoryStore::default());
        let due = Utc.with_ymd_and_hms(2026, 7, 6, 9, 0, 0).unwrap();
        store
            .upsert_cron_job(NewCronJobRecord {
                id: WEEKLY_SHIP_LEDGER_JOB_ID.to_string(),
                name: "Weekly ship ledger".to_string(),
                schedule: WEEKLY_SHIP_LEDGER_SCHEDULE.to_string(),
                enabled: true,
                cron_mode: "deny".to_string(),
                max_turns: 8,
                script_timeout_seconds: 120,
                next_run_at: Some(due),
            })
            .await
            .unwrap();

        let now = due + Duration::hours(1);
        let (left, right) = tokio::join!(
            materialize_due_cron_runs(store.as_ref(), now, 1),
            materialize_due_cron_runs(store.as_ref(), now, 1),
        );
        assert_eq!(left.unwrap() + right.unwrap(), 1);
        let runs = store
            .cron_runs(WEEKLY_SHIP_LEDGER_JOB_ID, 10)
            .await
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].scheduled_for, due);
        assert_eq!(runs[0].status, "queued");
        assert_eq!(
            store
                .cron_job(WEEKLY_SHIP_LEDGER_JOB_ID)
                .await
                .unwrap()
                .next_run_at,
            Some(Utc.with_ymd_and_hms(2026, 7, 13, 9, 0, 0).unwrap())
        );
    }

    #[tokio::test]
    async fn materialization_catch_up_is_bounded_and_registration_preserves_cursor() {
        let store = Arc::new(InMemoryStore::default());
        let due = Utc.with_ymd_and_hms(2026, 7, 6, 9, 0, 0).unwrap();
        store
            .upsert_cron_job(NewCronJobRecord {
                id: WEEKLY_SHIP_LEDGER_JOB_ID.to_string(),
                name: "Weekly ship ledger".to_string(),
                schedule: WEEKLY_SHIP_LEDGER_SCHEDULE.to_string(),
                enabled: true,
                cron_mode: "deny".to_string(),
                max_turns: 8,
                script_timeout_seconds: 120,
                next_run_at: Some(due),
            })
            .await
            .unwrap();

        let now = Utc.with_ymd_and_hms(2026, 7, 29, 12, 0, 0).unwrap();
        assert_eq!(
            materialize_due_cron_runs(store.as_ref(), now, 2)
                .await
                .unwrap(),
            2
        );
        let cursor = store
            .cron_job(WEEKLY_SHIP_LEDGER_JOB_ID)
            .await
            .unwrap()
            .next_run_at;
        assert_eq!(
            cursor,
            Some(Utc.with_ymd_and_hms(2026, 7, 20, 9, 0, 0).unwrap())
        );
        ensure_weekly_ship_ledger_job(store.as_ref(), now)
            .await
            .unwrap();
        assert_eq!(
            store
                .cron_job(WEEKLY_SHIP_LEDGER_JOB_ID)
                .await
                .unwrap()
                .next_run_at,
            cursor
        );
        assert_eq!(
            store
                .cron_runs(WEEKLY_SHIP_LEDGER_JOB_ID, 10)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn weekly_ship_ledger_trigger_creates_replayable_session_under_bounds() {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
        let chat = Arc::new(EchoChatRunner);
        let state = AppState::new(
            Arc::clone(&store),
            memory,
            chat,
            tm_modes::ModesConfig::default(),
            AuthConfig::NoAuth,
        );
        let scheduled_for = Utc.with_ymd_and_hms(2026, 7, 13, 9, 0, 0).unwrap();

        let run = trigger_weekly_ship_ledger(&state, scheduled_for)
            .await
            .unwrap();

        assert_eq!(run.job_id, WEEKLY_SHIP_LEDGER_JOB_ID);
        assert_eq!(run.status, "completed");
        let session_id = run.session_id.expect("scheduler created a session");
        let job = store.cron_job(WEEKLY_SHIP_LEDGER_JOB_ID).await.unwrap();
        assert_eq!(job.cron_mode, "deny");
        assert!(job.max_turns <= 8);
        assert!(job.script_timeout_seconds <= 120);
        let events = store.events_after(session_id, None).await.unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec![
                "mode",
                "cron_run_started",
                "text",
                "final",
                "cron_run_completed"
            ]
        );
        assert!(
            events.iter().all(|event| event.event_type != "approval"),
            "cron_mode deny must not auto-request approval for the safe weekly ledger"
        );
        let started = events
            .iter()
            .find(|event| event.event_type == "cron_run_started")
            .expect("cron start event");
        assert_eq!(started.payload_json["cronMode"], json!("deny"));
        assert_eq!(started.payload_json["maxTurns"], json!(8));
        assert_eq!(started.payload_json["scriptTimeoutSeconds"], json!(120));
        let completed = events
            .iter()
            .find(|event| event.event_type == "cron_run_completed")
            .expect("cron completion event");
        assert_eq!(completed.payload_json["approvalPolicy"], json!("defer"));
        assert_eq!(run.result_json["bounds"]["cronMode"], json!("deny"));
        assert_eq!(run.result_json["bounds"]["maxTurns"], json!(8));
        assert_eq!(
            run.result_json["bounds"]["scriptTimeoutSeconds"],
            json!(120)
        );
        assert_eq!(run.result_json["bounds"]["maxCatchUp"], json!(1));
    }

    #[tokio::test]
    async fn scheduler_authority_and_limits_are_enforced_in_code() {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
        let runner = Arc::new(BoundsRecordingRunner::default());
        let state = AppState::new(
            Arc::clone(&store),
            memory,
            Arc::clone(&runner),
            tm_modes::ModesConfig::default(),
            AuthConfig::NoAuth,
        );
        let scheduled_for = Utc.with_ymd_and_hms(2026, 7, 13, 9, 0, 0).unwrap();

        trigger_weekly_ship_ledger(&state, scheduled_for)
            .await
            .unwrap();
        let (capabilities, deny_approvals, limits, host_functions) = runner
            .observed
            .lock()
            .clone()
            .expect("runner observed turn");
        assert!(capabilities.is_empty());
        assert!(deny_approvals);
        assert_eq!(limits.max_turns, Some(8));
        assert_eq!(limits.cell_wall_ms, Some(120_000));
        assert_eq!(host_functions, 0);
    }

    #[tokio::test]
    async fn weekly_ship_ledger_trigger_reuses_completed_fire_on_restart() {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
        let state = AppState::new(
            Arc::clone(&store),
            memory,
            Arc::new(EchoChatRunner),
            tm_modes::ModesConfig::default(),
            AuthConfig::NoAuth,
        );
        let scheduled_for = Utc.with_ymd_and_hms(2026, 7, 13, 9, 0, 0).unwrap();

        let first = trigger_weekly_ship_ledger(&state, scheduled_for)
            .await
            .unwrap();
        let second = trigger_weekly_ship_ledger(&state, scheduled_for)
            .await
            .unwrap();

        assert_eq!(second.id, first.id);
        assert_eq!(second.session_id, first.session_id);
        assert_eq!(second.status, "completed");
        let runs = store
            .cron_runs(WEEKLY_SHIP_LEDGER_JOB_ID, 10)
            .await
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(store.list_sessions(10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn weekly_ship_ledger_trigger_reuses_running_fire_on_restart() {
        let store = Arc::new(InMemoryStore::default());
        ensure_weekly_ship_ledger_job(store.as_ref(), Utc::now())
            .await
            .unwrap();
        let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
        let state = AppState::new(
            Arc::clone(&store),
            memory,
            Arc::new(EchoChatRunner),
            tm_modes::ModesConfig::default(),
            AuthConfig::NoAuth,
        );
        let scheduled_for = Utc.with_ymd_and_hms(2026, 7, 13, 9, 0, 0).unwrap();
        let running = store
            .record_cron_run(NewCronRunRecord {
                job_id: WEEKLY_SHIP_LEDGER_JOB_ID.to_string(),
                scheduled_for,
                status: "running".to_string(),
                session_id: None,
                result_json: json!({"preexisting": true}),
            })
            .await
            .unwrap();

        let reused = trigger_weekly_ship_ledger(&state, scheduled_for)
            .await
            .unwrap();

        assert_eq!(reused.id, running.id);
        assert_eq!(reused.status, "running");
        assert_eq!(reused.session_id, None);
        assert_eq!(reused.result_json["preexisting"], json!(true));
        let runs = store
            .cron_runs(WEEKLY_SHIP_LEDGER_JOB_ID, 10)
            .await
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(store.list_sessions(10).await.unwrap().len(), 0);
    }
}

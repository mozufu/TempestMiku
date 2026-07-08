use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::AppState;
use crate::api::modes::{build_turn_prompt, mode_changed_payload, mode_profile};
use crate::{
    ChatRunner, ChatTurn, MemoryProvider, NewCronJobRecord, NewCronRunRecord, NewSession,
    PersistingEventSink, Result, ServerError, Store,
};

pub const WEEKLY_SHIP_LEDGER_JOB_ID: &str = "weekly-ship-ledger";
pub const WEEKLY_SHIP_LEDGER_SCHEDULE: &str = "0 9 * * 1";

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
    let job = ensure_weekly_ship_ledger_job(state.store.as_ref(), scheduled_for).await?;
    if job.cron_mode != "deny" || job.max_turns > 8 || job.script_timeout_seconds > 120 {
        return Err(ServerError::Policy(format!(
            "weekly ship ledger bounds invalid: cron_mode={}, max_turns={}, script_timeout_seconds={}",
            job.cron_mode, job.max_turns, job.script_timeout_seconds
        )));
    }

    let (run, claimed) = state
        .store
        .claim_cron_run(NewCronRunRecord {
            job_id: job.id.clone(),
            scheduled_for,
            status: "running".to_string(),
            session_id: None,
            result_json: json!({
                "jobId": job.id,
                "bounds": bounds_json(&job),
            }),
        })
        .await?;
    if !claimed {
        return Ok(run);
    }

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
                "runId": run.id,
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
    let final_text = state
        .chat
        .run_turn(
            ChatTurn {
                session_id: session.id,
                user_prompt: prompt_text.to_string(),
                mode,
                scope: profile.default_scope,
                system_prompt: composed.system_prompt,
            },
            sink.clone(),
            None,
        )
        .await?;
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
                "runId": run.id,
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
            run.id,
            "completed",
            Some(session.id),
            json!({
                "jobId": job.id,
                "sessionId": session.id,
                "finalText": final_text,
                "bounds": bounds_json(&job),
            }),
        )
        .await
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

    use chrono::TimeZone;

    use super::*;
    use crate::{AppState, AuthConfig, EchoChatRunner, InMemoryStore, StoreMemoryProvider};

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

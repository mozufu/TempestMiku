use serde::{Deserialize, Serialize};

mod schedule;
mod worker;

pub use schedule::{CronSchedule, missed_fire_times};
pub(crate) use worker::run_scheduler_daemon;
pub use worker::{
    ensure_weekly_ship_ledger_job, materialize_due_cron_runs, trigger_weekly_ship_ledger,
    weekly_ship_ledger_job,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::{Duration, TimeZone, Utc};
    use parking_lot::Mutex;
    use serde_json::json;
    use tm_core::EventSink;

    use super::*;
    use crate::{
        AppState, AuthConfig, ChatRunLimits, ChatRunner, ChatTurn, EchoChatRunner, InMemoryStore,
        NewCronJobRecord, NewCronRunRecord, Result, Store, StoreMemoryProvider,
    };

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

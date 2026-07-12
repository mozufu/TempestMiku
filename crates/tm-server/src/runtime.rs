use std::{
    net::SocketAddr,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use tokio::{sync::watch, task::JoinHandle};
use uuid::Uuid;

use crate::{
    AppState, ChatRunner, CodingEventSink, DreamWorkerConfig, MemoryProvider, Result, ServerError,
    Store, StoreCodingEventSink,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServerRole {
    Api,
    Worker,
    All,
}

impl ServerRole {
    pub const fn serves_api(self) -> bool {
        matches!(self, Self::Api | Self::All)
    }

    pub const fn runs_workers(self) -> bool {
        matches!(self, Self::Worker | Self::All)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Worker => "worker",
            Self::All => "all",
        }
    }
}

impl FromStr for ServerRole {
    type Err = ServerError;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "api" => Ok(Self::Api),
            "worker" => Ok(Self::Worker),
            "all" => Ok(Self::All),
            other => Err(ServerError::InvalidRequest(format!(
                "unsupported server role {other}; expected api, worker, or all"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub turn_concurrency: usize,
    pub turn_poll_interval: Duration,
    pub approval_poll_interval: Duration,
    pub push_poll_interval: Duration,
    pub scheduler_poll_interval: Duration,
    pub shutdown_grace: Duration,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            turn_concurrency: 4,
            turn_poll_interval: Duration::from_millis(250),
            approval_poll_interval: Duration::from_millis(500),
            push_poll_interval: Duration::from_millis(500),
            scheduler_poll_interval: Duration::from_secs(5),
            shutdown_grace: Duration::from_secs(30),
        }
    }
}

#[derive(Debug)]
pub struct RuntimeStatus {
    role: ServerRole,
    postgres: bool,
    migrations_applied: bool,
    workers_enabled: bool,
    shutting_down: AtomicBool,
    lease_reclaims: AtomicU64,
    heartbeat_failures: AtomicU64,
    link_hydration_failures: AtomicU64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatusSnapshot {
    pub role: String,
    pub postgres: bool,
    pub migrations_applied: bool,
    pub workers_enabled: bool,
    pub shutting_down: bool,
    pub lease_reclaims: u64,
    pub heartbeat_failures: u64,
    pub link_hydration_failures: u64,
}

impl RuntimeStatus {
    pub fn new(role: ServerRole, postgres: bool, migrations_applied: bool) -> Self {
        Self {
            role,
            postgres,
            migrations_applied,
            workers_enabled: role.runs_workers(),
            shutting_down: AtomicBool::new(false),
            lease_reclaims: AtomicU64::new(0),
            heartbeat_failures: AtomicU64::new(0),
            link_hydration_failures: AtomicU64::new(0),
        }
    }

    pub fn local_test() -> Self {
        Self::new(ServerRole::Api, false, false)
    }

    pub fn snapshot(&self) -> RuntimeStatusSnapshot {
        RuntimeStatusSnapshot {
            role: self.role.as_str().to_string(),
            postgres: self.postgres,
            migrations_applied: self.migrations_applied,
            workers_enabled: self.workers_enabled,
            shutting_down: self.shutting_down.load(Ordering::Relaxed),
            lease_reclaims: self.lease_reclaims.load(Ordering::Relaxed),
            heartbeat_failures: self.heartbeat_failures.load(Ordering::Relaxed),
            link_hydration_failures: self.link_hydration_failures.load(Ordering::Relaxed),
        }
    }

    pub fn add_link_hydration_failures(&self, failures: u64) {
        self.link_hydration_failures
            .fetch_add(failures, Ordering::Relaxed);
    }

    pub(crate) fn record_heartbeat_failure(&self) {
        self.heartbeat_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_lease_reclaim(&self) {
        self.lease_reclaims.fetch_add(1, Ordering::Relaxed);
    }

    fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }
}

pub async fn run_server<S, M, C>(
    addr: SocketAddr,
    state: AppState<S, M, C>,
    role: ServerRole,
    config: RuntimeConfig,
) -> Result<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let state = state.with_auto_turn_dispatcher(false);
    let runtime = state.runtime_status().snapshot();
    if runtime.role != role.as_str() {
        return Err(ServerError::InvalidRequest(format!(
            "runtime status role {} does not match requested role {}",
            runtime.role,
            role.as_str()
        )));
    }
    if role.runs_workers() && !runtime.postgres {
        return Err(ServerError::Policy(
            "worker roles require Postgres-backed state".to_string(),
        ));
    }
    if runtime.postgres && !runtime.migrations_applied {
        return Err(ServerError::Policy(
            "Postgres migrations are not fully applied".to_string(),
        ));
    }
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut tasks = Vec::<JoinHandle<()>>::new();

    if role.runs_workers() {
        tasks.extend(spawn_workers(
            state.clone(),
            config.clone(),
            shutdown_rx.clone(),
        ));
    }

    if role.serves_api() {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|error| ServerError::Store(format!("server bind failed: {error}")))?;
        tracing::info!(%addr, role = role.as_str(), "tm-server listening");
        let router = crate::app(state.clone());
        let mut api_shutdown = shutdown_rx.clone();
        tasks.push(tokio::spawn(async move {
            let result = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                while !*api_shutdown.borrow() {
                    if api_shutdown.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
            if let Err(error) = result {
                let error = tm_memory::redact_dream_text(&error.to_string()).text;
                tracing::error!(%error, "http server failed");
            }
        }));
    } else {
        tracing::info!(role = role.as_str(), "tm-server worker runtime started");
    }

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let runtime_error = loop {
        tokio::select! {
            _ = &mut shutdown => break None,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                if tasks.iter().any(|task| task.is_finished()) {
                    break Some(ServerError::Store(
                        "a supervised server task exited unexpectedly".to_string(),
                    ));
                }
            }
        }
    };
    state.runtime_status().begin_shutdown();
    let _ = shutdown_tx.send(true);
    tracing::info!(
        grace_seconds = config.shutdown_grace.as_secs(),
        "runtime draining"
    );

    let drained = tokio::time::timeout(config.shutdown_grace, join_all(tasks.iter_mut())).await;
    if drained.is_err() {
        tracing::warn!("runtime drain grace expired; aborting remaining tasks");
        for task in &tasks {
            task.abort();
        }
        let _ = join_all(tasks.iter_mut()).await;
    }
    match runtime_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn spawn_workers<S, M, C>(
    state: AppState<S, M, C>,
    config: RuntimeConfig,
    shutdown: watch::Receiver<bool>,
) -> Vec<JoinHandle<()>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let turn = crate::api::sessions::start_supervised_turn_dispatcher(
        state.clone(),
        shutdown.clone(),
        config.turn_concurrency,
        config.turn_poll_interval,
    );
    let approvals = tokio::spawn(run_approval_worker(
        state.clone(),
        shutdown.clone(),
        config.approval_poll_interval,
    ));
    let push = state.push.clone().map(|push| {
        tokio::spawn(run_push_worker(
            push,
            shutdown.clone(),
            config.push_poll_interval,
        ))
    });
    let dream = state
        .dream_worker(DreamWorkerConfig::default())
        .into_daemon();
    let dream_shutdown = shutdown.clone();
    let dream = tokio::spawn(async move {
        dream.run_until_shutdown(dream_shutdown).await;
    });
    let scheduler = tokio::spawn(crate::scheduler::run_scheduler_daemon(
        state,
        shutdown,
        config.scheduler_poll_interval,
    ));
    let mut workers = vec![turn, approvals, dream, scheduler];
    workers.extend(push);
    workers
}

async fn run_push_worker(
    push: Arc<crate::PushService>,
    mut shutdown: watch::Receiver<bool>,
    poll_interval: Duration,
) {
    let owner_id = Uuid::new_v4();
    loop {
        if *shutdown.borrow() {
            return;
        }
        if let Err(error) = push.tick(owner_id).await {
            let error = tm_memory::redact_dream_text(&error.to_string()).text;
            tracing::warn!(%error, "push worker tick failed");
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

async fn run_approval_worker<S, M, C>(
    state: AppState<S, M, C>,
    mut shutdown: watch::Receiver<bool>,
    poll_interval: Duration,
) where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let owner_id = Uuid::new_v4();
    loop {
        if *shutdown.borrow() {
            return;
        }
        if let Err(error) = run_approval_tick(&state, owner_id, &shutdown).await {
            let error = tm_memory::redact_dream_text(&error.to_string()).text;
            tracing::warn!(%error, "approval worker tick failed");
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

async fn run_approval_tick<S, M, C>(
    state: &AppState<S, M, C>,
    owner_id: Uuid,
    shutdown: &watch::Receiver<bool>,
) -> Result<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let now = Utc::now();
    let stale_before = now - chrono::Duration::seconds(30);
    let mut resolution_events = state
        .store
        .cancel_stale_non_resumable_approvals(stale_before, now)
        .await?;
    resolution_events.extend(state.store.expire_pending_approvals(now).await?);
    for event in resolution_events {
        let _ = state.sender(event.session_id).send(event);
    }

    for _ in 0..32 {
        if *shutdown.borrow() {
            break;
        }
        let Some(lease) = state
            .store
            .claim_next_approval_effect(owner_id, Utc::now(), chrono::Duration::seconds(60))
            .await?
        else {
            break;
        };
        let approval = state
            .store
            .approval_request(lease.effect.session_id, lease.effect.approval_id)
            .await?;
        let sink: Arc<dyn CodingEventSink> = match approval.turn_id {
            Some(turn_id) => Arc::new(StoreCodingEventSink::for_turn(
                approval.session_id,
                turn_id,
                Arc::clone(&state.store),
                state.sender(approval.session_id),
            )),
            None => Arc::new(StoreCodingEventSink::new(
                approval.session_id,
                Arc::clone(&state.store),
                state.sender(approval.session_id),
            )),
        };
        if let Err(error) = crate::api::approvals::apply_approval_effect_lease(
            state.store.as_ref(),
            &approval,
            &lease,
            sink,
        )
        .await
        {
            let error = tm_memory::redact_dream_text(&error.to_string()).text;
            tracing::warn!(approval_id = %approval.id, %error, "approval effect failed");
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler installs");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_parser_is_fail_closed() {
        assert_eq!("api".parse::<ServerRole>().unwrap(), ServerRole::Api);
        assert_eq!("worker".parse::<ServerRole>().unwrap(), ServerRole::Worker);
        assert_eq!("all".parse::<ServerRole>().unwrap(), ServerRole::All);
        assert!("background".parse::<ServerRole>().is_err());
    }

    #[test]
    fn role_capabilities_are_explicit() {
        assert!(ServerRole::Api.serves_api());
        assert!(!ServerRole::Api.runs_workers());
        assert!(!ServerRole::Worker.serves_api());
        assert!(ServerRole::Worker.runs_workers());
        assert!(ServerRole::All.serves_api());
        assert!(ServerRole::All.runs_workers());
    }

    #[test]
    fn runtime_status_exposes_operational_counters() {
        let status = RuntimeStatus::new(ServerRole::All, true, true);
        status.record_lease_reclaim();
        status.record_heartbeat_failure();
        status.add_link_hydration_failures(2);

        let snapshot = status.snapshot();
        assert_eq!(snapshot.role, "all");
        assert!(snapshot.postgres);
        assert!(snapshot.migrations_applied);
        assert!(snapshot.workers_enabled);
        assert_eq!(snapshot.lease_reclaims, 1);
        assert_eq!(snapshot.heartbeat_failures, 1);
        assert_eq!(snapshot.link_hydration_failures, 2);
    }
}

use std::{
    collections::BTreeMap,
    convert::Infallible,
    fs,
    path::{Component, Path as FsPath, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::Utc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    middleware,
    response::{IntoResponse, Sse, sse::Event},
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use uuid::Uuid;

use tm_agents::{
    ActorLifecycleEvent, AgentResourceHandler, HistoryResourceHandler, MailboxRegistry,
};
use tm_artifacts::{ResourceContent, preview};
use tm_core::DEFAULT_SYSTEM_PROMPT;
use tm_drive::{IntoSharedDriveStore, SharedDriveStore};
use tm_host::{
    CapabilityGrants, InvocationCtx, LinkedFolders, LinkedResourceHandler, ResourceEntry,
    ResourceRegistry,
};
use tm_memory::DreamQueueRecord;

use crate::{
    ApprovalBroker, ApprovalEffectRecord, ApprovalOption, ApprovalPrompt, ApprovalRequestRecord,
    ApprovalStatus, AuthConfig, AuthContext, AuthDeviceStore, ChatRunner, ChatTurn, CodingBackend,
    CodingEventSink, CodingTurn, DurableApprovalSpec, MemoryProvider, MemoryRecordRef,
    MemoryWriteKind, MemoryWriteProposal, MemoryWriteStatus, ModeCatalog, ModeId, ModeProfile,
    ModesConfig, NewProjectItem, NewSession, PersistingEventSink, ProjectItemKind,
    ProjectItemRecord, ResolveApprovalRequest, Result, ServerError, SessionEvent,
    SkillProposalStatus, Store, StoreCodingEventSink, StoreEvent, store::ModeState,
    store::RecallChunkRecord,
};

const SESSION_RESOURCE_PREVIEW_BYTES: usize = 512;

#[derive(Debug, Clone)]
struct ActorResourceLinkSeed {
    actor_id: String,
    artifact_uri: Option<String>,
    history_uri: Option<String>,
}

impl ActorResourceLinkSeed {
    fn from_lifecycle(event: &ActorLifecycleEvent) -> Option<Self> {
        let link = event.output_link()?;
        Some(Self {
            actor_id: link.actor_id.as_str().to_string(),
            artifact_uri: link.artifact_uri,
            history_uri: link.history_uri,
        })
    }

    fn payload(&self, source_event: &SessionEvent) -> Value {
        json!({
            "kind": "resources_linked",
            "actor_id": self.actor_id,
            "source_event_type": source_event.event_type,
            "source_event_seq": source_event.seq,
            "artifact_uri": self.artifact_uri,
            "history_uri": self.history_uri,
        })
    }
}

pub(crate) mod approvals;
mod auth_devices;
mod events;
mod mode_suggest;
pub(crate) mod modes;
pub(crate) mod pairing;
mod projects;
mod push_notifications;
mod resources;
pub(crate) mod sessions;

#[cfg(test)]
mod tests;

use mode_suggest::{MODE_SUGGEST_APPROVAL_TIMEOUT, MODE_SUGGEST_CAPABILITY, ModeSuggestHostFn};
use modes::{active_skills, build_turn_prompt, mode_changed_payload, mode_profile};
use projects::{build_project_overview, project_id_from_scope, record_project_observations};
use resources::validate_relative_path;

pub use modes::{ModeRequest, ModeResponse};
pub use sessions::{
    CreateSessionRequest, CreateSessionResponse, EndSessionRequest, EndSessionResponse,
    ListSessionsResponse, MemoryWriteProposalResponse, PostMessageRequest, PostMessageResponse,
    ProposeMemoryWriteRequest, SessionMessagesResponse, SetSessionScopeRequest,
    SetSessionScopeResponse,
};

pub struct AppState<S, M, C> {
    pub store: Arc<S>,
    pub memory: Arc<M>,
    pub chat: Arc<C>,
    pub persona: ModesConfig,
    pub auth: AuthContext,
    pub coding_backend: Option<Arc<dyn CodingBackend>>,
    pub approval_broker: Arc<ApprovalBroker>,
    pub artifact_root: PathBuf,
    pub linked_folders: LinkedFolders,
    pub drive_store: Option<SharedDriveStore>,
    pub actor_roster: Arc<MailboxRegistry>,
    pub push: Option<Arc<crate::PushService>>,
    runtime_status: Arc<crate::RuntimeStatus>,
    auto_start_turn_dispatcher: bool,
    turn_notify: Arc<tokio::sync::Notify>,
    turn_dispatcher_started: Arc<AtomicBool>,
    live_events:
        Arc<parking_lot::Mutex<std::collections::BTreeMap<Uuid, broadcast::Sender<SessionEvent>>>>,
}

impl<S, M, C> Clone for AppState<S, M, C> {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
            memory: Arc::clone(&self.memory),
            chat: Arc::clone(&self.chat),
            persona: self.persona.clone(),
            auth: self.auth.clone(),
            coding_backend: self.coding_backend.clone(),
            approval_broker: Arc::clone(&self.approval_broker),
            artifact_root: self.artifact_root.clone(),
            linked_folders: self.linked_folders.clone(),
            drive_store: self.drive_store.clone(),
            actor_roster: Arc::clone(&self.actor_roster),
            push: self.push.clone(),
            runtime_status: Arc::clone(&self.runtime_status),
            auto_start_turn_dispatcher: self.auto_start_turn_dispatcher,
            turn_notify: Arc::clone(&self.turn_notify),
            turn_dispatcher_started: Arc::clone(&self.turn_dispatcher_started),
            live_events: Arc::clone(&self.live_events),
        }
    }
}

impl<S, M, C> AppState<S, M, C> {
    pub fn new(
        store: Arc<S>,
        memory: Arc<M>,
        chat: Arc<C>,
        persona: ModesConfig,
        auth: AuthConfig,
    ) -> Self
    where
        S: Store,
    {
        let approval_broker = Arc::new(ApprovalBroker::default());
        approval_broker.bind_store(Arc::clone(&store));
        Self {
            store,
            memory,
            chat,
            persona,
            auth: AuthContext::new(auth),
            coding_backend: None,
            approval_broker,
            artifact_root: tm_artifacts::default_root(),
            linked_folders: LinkedFolders::default(),
            drive_store: None,
            actor_roster: Arc::new(MailboxRegistry::new()),
            push: None,
            runtime_status: Arc::new(crate::RuntimeStatus::local_test()),
            // Production startup is role-supervised. Unit tests retain the small embedded
            // dispatcher so router tests can exercise the durable API without a daemon.
            auto_start_turn_dispatcher: cfg!(test),
            turn_notify: Arc::new(tokio::sync::Notify::new()),
            turn_dispatcher_started: Arc::new(AtomicBool::new(false)),
            live_events: Arc::new(parking_lot::Mutex::new(std::collections::BTreeMap::new())),
        }
    }

    pub fn with_actor_roster(mut self, roster: Arc<MailboxRegistry>) -> Self {
        self.actor_roster = roster;
        self
    }

    pub fn with_auth_store(mut self, store: Arc<dyn AuthDeviceStore>) -> Self {
        self.auth = self.auth.with_store(store);
        self
    }

    pub fn with_approval_broker(mut self, broker: Arc<ApprovalBroker>) -> Self
    where
        S: Store,
    {
        broker.bind_store(Arc::clone(&self.store));
        self.approval_broker = broker;
        self
    }

    pub fn with_coding_backend(mut self, backend: Arc<dyn CodingBackend>) -> Self {
        self.coding_backend = Some(backend);
        self
    }

    pub fn with_artifact_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.artifact_root = root.into();
        self
    }

    pub fn with_linked_folders(mut self, linked_folders: LinkedFolders) -> Self {
        self.linked_folders = linked_folders;
        self
    }

    pub fn with_drive_store(mut self, drive_store: impl IntoSharedDriveStore) -> Self {
        self.drive_store = Some(drive_store.into_shared_drive_store());
        self
    }

    pub fn with_push_service(mut self, push: Arc<crate::PushService>) -> Self {
        self.push = Some(push);
        self
    }

    pub fn with_runtime_status(mut self, runtime_status: Arc<crate::RuntimeStatus>) -> Self {
        self.runtime_status = runtime_status;
        self
    }

    pub fn with_auto_turn_dispatcher(mut self, enabled: bool) -> Self {
        self.auto_start_turn_dispatcher = enabled;
        self
    }

    pub fn runtime_status(&self) -> &Arc<crate::RuntimeStatus> {
        &self.runtime_status
    }

    pub fn dream_worker(&self, config: crate::DreamWorkerConfig) -> crate::ServerDreamWorker<S>
    where
        S: Store + Send + Sync + 'static,
    {
        let live_events = Arc::clone(&self.live_events);
        let sender_for = Arc::new(move |session_id| {
            live_events
                .lock()
                .entry(session_id)
                .or_insert_with(|| broadcast::channel(256).0)
                .clone()
        });
        crate::ServerDreamWorker::new(
            Arc::clone(&self.store),
            Arc::clone(&self.approval_broker),
            sender_for,
            config,
        )
    }

    pub(crate) fn sender(&self, session_id: Uuid) -> broadcast::Sender<SessionEvent> {
        self.live_events
            .lock()
            .entry(session_id)
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }

    pub(crate) fn notify_turn_dispatcher(&self) {
        self.turn_notify.notify_one();
    }

    fn wire_turn_dispatcher(&self)
    where
        S: Store,
        M: MemoryProvider,
        C: ChatRunner,
    {
        if self.turn_dispatcher_started.swap(true, Ordering::SeqCst) {
            return;
        }
        sessions::start_turn_dispatcher(self.clone(), Arc::clone(&self.turn_notify));
    }

    /// Wire actor lifecycle events into the session SSE stream (P3.5).
    ///
    /// Installs a hook on the `actor_roster` that serializes each `ActorLifecycleEvent` and
    /// appends it to the session store + broadcasts it on the live SSE channel for that session.
    /// The hook uses `tokio::runtime::Handle::current()` captured at call time so it works from
    /// both the main runtime (agents.run / agents.parallel) and from background threads
    /// (agents.spawn), which don't have their own runtime but can still enqueue tasks on the main
    /// runtime via the handle.
    ///
    /// Must be called once at startup, after the `AppState` is fully constructed.
    pub fn wire_lifecycle_sink(&self)
    where
        S: Store + Send + Sync + 'static,
    {
        let store = Arc::clone(&self.store);
        let live_events = Arc::clone(&self.live_events);
        let handle = tokio::runtime::Handle::current();
        self.actor_roster
            .set_lifecycle_hook(move |session_id, event| {
                let Ok(session_uuid) = session_id.parse::<Uuid>() else {
                    return;
                };
                let event_type = event.event_type().to_string();
                let resource_link = ActorResourceLinkSeed::from_lifecycle(&event);
                let Ok(payload) = serde_json::to_value(&event) else {
                    return;
                };
                let sender = live_events
                    .lock()
                    .entry(session_uuid)
                    .or_insert_with(|| broadcast::channel(256).0)
                    .clone();
                let store = Arc::clone(&store);
                handle.spawn(async move {
                    match store.append_event(session_uuid, &event_type, payload).await {
                        Ok(session_event) => {
                            let _ = sender.send(session_event.clone());
                            if let Some(resource_link) = resource_link {
                                let payload = resource_link.payload(&session_event);
                                match store
                                    .append_event(session_uuid, "actor_resources_linked", payload)
                                    .await
                                {
                                    Ok(link_event) => {
                                        let _ = sender.send(link_event);
                                    }
                                    Err(err) => {
                                        let err =
                                            tm_memory::redact_dream_text(&err.to_string()).text;
                                        tracing::warn!(
                                            %err,
                                            "actor resource link event store write failed"
                                        )
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            let err = tm_memory::redact_dream_text(&err.to_string()).text;
                            tracing::warn!(%err, "lifecycle event store write failed")
                        }
                    }
                });
            });

        let store = Arc::clone(&self.store);
        let live_events = Arc::clone(&self.live_events);
        self.actor_roster
            .set_raw_event_hook(move |session_id, event_type, payload| {
                let store = Arc::clone(&store);
                let live_events = Arc::clone(&live_events);
                Box::pin(async move {
                    let session_uuid = session_id
                        .parse::<Uuid>()
                        .map_err(|err| format!("invalid session id {session_id}: {err}"))?;
                    let sender = live_events
                        .lock()
                        .entry(session_uuid)
                        .or_insert_with(|| broadcast::channel(256).0)
                        .clone();
                    let event = store
                        .append_event(session_uuid, &event_type, payload)
                        .await
                        .map_err(|err| err.to_string())?;
                    let _ = sender.send(event);
                    Ok(())
                })
            });
    }
}

pub fn app<S, M, C>(state: AppState<S, M, C>) -> Router
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    if state.auto_start_turn_dispatcher {
        state.wire_turn_dispatcher();
    }
    let protected = Router::<AppState<S, M, C>>::new()
        .route("/ready", get(readiness::<S, M, C>))
        .route("/metrics", get(metrics::<S, M, C>))
        .route("/auth/devices", get(auth_devices::list_devices::<S, M, C>))
        .route("/auth/logout", post(auth_devices::logout::<S, M, C>))
        .route(
            "/auth/push-registration",
            put(push_notifications::register::<S, M, C>)
                .delete(push_notifications::unregister::<S, M, C>),
        )
        .route(
            "/auth/devices/:device_id",
            delete(auth_devices::revoke_device::<S, M, C>),
        )
        .route("/modes", get(modes::list_modes::<S, M, C>))
        .route(
            "/sessions",
            get(sessions::list_sessions::<S, M, C>).post(sessions::create_session::<S, M, C>),
        )
        .route("/sessions/:id", get(sessions::get_session::<S, M, C>))
        .route("/sessions/:id/end", post(sessions::end_session::<S, M, C>))
        .route(
            "/sessions/:id/scope",
            post(sessions::set_session_scope::<S, M, C>),
        )
        .route(
            "/sessions/:id/events",
            get(events::session_events::<S, M, C>),
        )
        .route(
            "/sessions/:id/messages",
            get(sessions::get_session_messages::<S, M, C>).post(sessions::post_message::<S, M, C>),
        )
        .route(
            "/sessions/:id/turns/:turn_id",
            get(sessions::get_turn::<S, M, C>),
        )
        .route(
            "/sessions/:id/memory/proposals",
            post(sessions::propose_memory_write::<S, M, C>),
        )
        .route("/sessions/:id/mode", get(modes::get_mode::<S, M, C>))
        .route(
            "/sessions/:id/mode/suggest",
            post(modes::suggest_mode::<S, M, C>),
        )
        .route(
            "/sessions/:id/mode/apply",
            post(modes::apply_mode::<S, M, C>),
        )
        .route("/sessions/:id/mode/lock", post(modes::lock_mode::<S, M, C>))
        .route(
            "/sessions/:id/mode/unlock",
            post(modes::unlock_mode::<S, M, C>),
        )
        .route(
            "/sessions/:id/mode/override",
            post(modes::override_mode::<S, M, C>),
        )
        .route(
            "/sessions/:id/approvals/:approval_id",
            get(push_notifications::approval::<S, M, C>)
                .post(approvals::resolve_approval::<S, M, C>),
        )
        .route(
            "/sessions/:id/project",
            get(projects::project_overview::<S, M, C>),
        )
        .route(
            "/sessions/:id/project/open-loops",
            get(projects::project_open_loops::<S, M, C>),
        )
        .route(
            "/sessions/:id/project/decisions",
            get(projects::project_decisions::<S, M, C>),
        )
        .route(
            "/sessions/:id/project/next-actions",
            get(projects::project_next_actions::<S, M, C>),
        )
        .route(
            "/sessions/:id/promote",
            post(projects::promote_session::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/resolve",
            get(resources::resolve_resource::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/read",
            get(resources::resolve_resource::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/preview",
            get(resources::preview_resource::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/list",
            get(resources::list_resources::<S, M, C>),
        )
        .route(
            "/sessions/:id/drive/feed",
            get(resources::drive_feed::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/artifacts",
            get(resources::list_artifacts::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/artifacts/:artifact_id",
            get(resources::read_artifact::<S, M, C>),
        )
        .route_layer(middleware::from_fn_with_state(
            state.auth.clone(),
            crate::auth::require_auth,
        ));

    Router::<AppState<S, M, C>>::new()
        .route("/health", get(health))
        .route("/pair", get(pairing::page::<S, M, C>))
        .route(
            "/auth/pairing-codes",
            post(auth_devices::create_pairing_code::<S, M, C>),
        )
        .route("/auth/pair", post(auth_devices::pair_device::<S, M, C>))
        .merge(protected)
        .fallback_service(crate::webui::service())
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn readiness<S, M, C>(State(state): State<AppState<S, M, C>>) -> Result<impl IntoResponse>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let runtime = state.runtime_status().snapshot();
    let ready = !runtime.shutting_down
        && (!runtime.postgres || runtime.migrations_applied)
        && (!runtime.workers_enabled || runtime.postgres);
    let status = if runtime.shutting_down {
        "draining"
    } else if ready {
        "ready"
    } else {
        "not_ready"
    };
    let status_code = if ready {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    Ok((
        status_code,
        Json(json!({
            "status": status,
            "runtime": runtime,
        })),
    ))
}

async fn metrics<S, M, C>(State(state): State<AppState<S, M, C>>) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let store = state.store.runtime_metrics(Utc::now()).await?;
    let push = match &state.push {
        Some(push) => Some(push.runtime_metrics().await?),
        None => None,
    };
    Ok(Json(json!({
        "runtime": state.runtime_status().snapshot(),
        "queues": {
            "turn": {
                "depth": store.turn_queue_depth,
                "oldestAgeSeconds": store.turn_oldest_age_seconds,
            },
            "dream": {
                "depth": store.dream_queue_depth,
                "oldestAgeSeconds": store.dream_oldest_age_seconds,
            },
            "scheduler": {
                "depth": store.cron_queue_depth,
                "oldestAgeSeconds": store.cron_oldest_age_seconds,
                "lagSeconds": store.scheduler_lag_seconds,
            },
            "approvalEffects": {
                "depth": store.approval_effect_queue_depth,
                "oldestAgeSeconds": store.approval_effect_oldest_age_seconds,
            },
            "push": push.map(|push| json!({
                "depth": push.queue_depth,
                "oldestAgeSeconds": push.oldest_age_seconds,
                "retries": push.retries,
                "failedDeliveries": push.failed_deliveries,
                "disabledRegistrations": push.disabled_registrations,
            })),
        },
        "pendingApprovals": store.pending_approvals,
        "leaseReclaims": store.lease_reclaims,
    })))
}

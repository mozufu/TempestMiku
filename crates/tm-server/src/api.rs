use std::{
    collections::BTreeMap,
    convert::Infallible,
    fs,
    path::{Component, Path as FsPath, PathBuf},
    sync::Arc,
    time::Duration,
};

use chrono::Utc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Sse, sse::Event},
    routing::{get, post},
};
use futures::{StreamExt, stream::BoxStream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use tm_artifacts::{ResourceContent, preview};
use tm_core::DEFAULT_SYSTEM_PROMPT;
use tm_host::{
    CapabilityGrants, InvocationCtx, LinkedFolders, LinkedResourceHandler, ResourceEntry,
    ResourceRegistry,
};

use crate::{
    ApprovalBroker, ApprovalStatus, AuthConfig, ChatRunner, ChatTurn, CodingBackend, CodingTurn,
    MemoryProvider, MemoryRecordRef, MemoryWriteKind, MemoryWriteProposal, MemoryWriteStatus, Mode,
    NewProjectItem, NewSession, PersistingEventSink, PersonaConfig, ProjectItemKind,
    ProjectItemRecord, ResolveApprovalRequest, Result, ServerError, SessionEvent, Store,
    StoreCodingEventSink, StoreEvent, store::ModeState, store::RecallChunkRecord,
};

const SESSION_RESOURCE_PREVIEW_BYTES: usize = 512;

mod approvals;
mod events;
mod modes;
mod projects;
mod resources;
mod sessions;

#[cfg(test)]
mod tests;

use modes::{
    active_skills, build_turn_prompt, commit_mode_state, mode_changed_payload,
    route_mode_for_prompt,
};
use projects::{build_project_overview, project_id_from_scope, record_project_observations};
use resources::validate_relative_path;
use sessions::default_subject;

pub use modes::{ModeRequest, ModeResponse};
pub use sessions::{
    CreateSessionRequest, CreateSessionResponse, MemoryWriteProposalResponse, PostMessageRequest,
    ProposeMemoryWriteRequest,
};

pub struct AppState<S, M, C> {
    pub store: Arc<S>,
    pub memory: Arc<M>,
    pub chat: Arc<C>,
    pub persona: PersonaConfig,
    pub auth: AuthConfig,
    pub coding_backend: Option<Arc<dyn CodingBackend>>,
    pub approval_broker: Arc<ApprovalBroker>,
    pub artifact_root: PathBuf,
    pub linked_folders: LinkedFolders,
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
            live_events: Arc::clone(&self.live_events),
        }
    }
}

impl<S, M, C> AppState<S, M, C> {
    pub fn new(
        store: Arc<S>,
        memory: Arc<M>,
        chat: Arc<C>,
        persona: PersonaConfig,
        auth: AuthConfig,
    ) -> Self {
        Self {
            store,
            memory,
            chat,
            persona,
            auth,
            coding_backend: None,
            approval_broker: Arc::new(ApprovalBroker::default()),
            artifact_root: tm_artifacts::default_root(),
            linked_folders: LinkedFolders::default(),
            live_events: Arc::new(parking_lot::Mutex::new(std::collections::BTreeMap::new())),
        }
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

    fn sender(&self, session_id: Uuid) -> broadcast::Sender<SessionEvent> {
        self.live_events
            .lock()
            .entry(session_id)
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }
}

pub fn app<S, M, C>(state: AppState<S, M, C>) -> Router
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    Router::<AppState<S, M, C>>::new()
        .route("/health", get(health))
        .route("/sessions", post(sessions::create_session::<S, M, C>))
        .route("/sessions/:id", get(sessions::get_session::<S, M, C>))
        .route(
            "/sessions/:id/events",
            get(events::session_events::<S, M, C>),
        )
        .route(
            "/sessions/:id/messages",
            post(sessions::post_message::<S, M, C>),
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
            post(approvals::resolve_approval::<S, M, C>),
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
            "/sessions/:id/resources/artifacts",
            get(resources::list_artifacts::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/artifacts/:artifact_id",
            get(resources::read_artifact::<S, M, C>),
        )
        .fallback_service(crate::webui::service())
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

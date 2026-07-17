use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, BoxStream};
use serde::Deserialize;
use serde_json::{Value, json, to_value};
use tm_agents::{ActorBudget, ActorId, ActorRecord, ActorStatus, FailureReason, MailboxRegistry};
use tm_core::{
    AgentConfig, CellBudget, ChatRequest, Error as CoreError, LlmClient, Message,
    Result as CoreResult, StreamEvent, ToolChoice,
};
use tm_host::{CapabilityGrants, FsMode, LinkedFolderConfig, LinkedFolders};
use tm_lang::{TmSandbox, TmSandboxOptions};
use tm_llm::OpenAiClient;
use tm_server::{
    AppState, ApprovalBroker, ApprovalOption, ApprovalPrompt, ApprovalStatus, AuthConfig,
    ChatActorExecutor, CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult,
    EchoChatRunner, HttpApprovalPolicy, InMemoryStore, ModeId, NativeApprovalMode, NativeTmBackend,
    RosterCodingEventSink, ServerDreamWorker, ServerError, Store, StoreEvent, StoreMemoryProvider,
    app,
};
use tokio::{process::Command, sync::broadcast};
use uuid::Uuid;

use crate::{
    E2eConfig, EvidenceManifest, EvidenceRecorder, LiveSpeaker, MikuClient, ScriptedSpeaker,
    ServerEvidence, UiEvidence, WorkflowOptions, default_run_dir, run_actor_smoke, run_workflow,
    timestamp, write_workflow_record,
};

mod api_scenarios;
mod backend;
mod evolution_policy;
mod native_actor_scenario;
mod native_actor_server;
mod runner;
mod server;
mod ui_scenario;

use api_scenarios::{run_actor_api_scenario, run_public_api_scenario};
use backend::RecordingBackend;
pub use evolution_policy::run_record_evolution_policy;
use native_actor_scenario::{ensure_live_llm_env, run_record_native_actor_inner};
use native_actor_server::NativeActorRecordingServer;
pub use runner::{
    run_record_api, run_record_live_api, run_record_native_actor, run_record_suite, run_record_ui,
};
use server::RecordingServer;
use ui_scenario::{capture_resource, record_scenario_result, run_ui_scenario};

#[derive(Debug, Clone, Default)]
pub struct RecordOptions {
    pub output_dir: Option<PathBuf>,
    pub headed: bool,
    pub skip_flutter_build: bool,
}

const LIVE_PREFLIGHT_TOKEN: &str = "TEMPEST_MIKU_E2E_OK";
const NATIVE_P3_BROADCAST_TEXT: &str = "native P3 plus broadcast token";
const NATIVE_P3_FINAL_TEXT: &str = "native P3 plus coordination complete";

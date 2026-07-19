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
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{
    E2eConfig, EvidenceManifest, EvidenceRecorder, LiveSpeaker, MikuClient, ScriptedSpeaker,
    ServerEvidence, WorkflowOptions, default_run_dir, run_actor_smoke, run_workflow, timestamp,
    write_workflow_record,
};

mod api_scenarios;
mod backend;
mod evolution_policy;
mod native_actor_scenario;
mod native_actor_server;
mod runner;
mod server;

use api_scenarios::{run_actor_api_scenario, run_public_api_scenario};
use backend::RecordingBackend;
pub use evolution_policy::run_record_evolution_policy;
use native_actor_scenario::{ensure_live_llm_env, run_record_native_actor_inner};
use native_actor_server::NativeActorRecordingServer;
pub use runner::{run_record_api, run_record_live_api, run_record_native_actor, run_record_suite};
use server::RecordingServer;

#[derive(Debug, Clone, Default)]
pub struct RecordOptions {
    pub output_dir: Option<PathBuf>,
}

async fn capture_resource(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    session_id: &str,
    uri: &str,
) -> Result<()> {
    let preview = client
        .preview_resource(session_id, uri)
        .await
        .with_context(|| format!("previewing resource {uri}"))?;
    let resolved = client
        .resolve_resource(session_id, uri)
        .await
        .with_context(|| format!("resolving resource {uri}"))?;
    recorder.record_resource(session_id, uri, &preview, &resolved)?;
    Ok(())
}

fn record_scenario_result(
    recorder: &EvidenceRecorder,
    name: &str,
    started_at: String,
    result: &Result<Value>,
) {
    recorder.record_scenario(crate::RecordedScenario {
        name: name.to_string(),
        ok: result.is_ok(),
        started_at,
        finished_at: timestamp(),
        details: result
            .as_ref()
            .cloned()
            .unwrap_or_else(|_| json!({ "status": "failed" })),
        error: result.as_ref().err().map(|err| err.to_string()),
    });
}

const LIVE_PREFLIGHT_TOKEN: &str = "TEMPEST_MIKU_E2E_OK";
const NATIVE_P3_BROADCAST_TEXT: &str = "native P3 plus broadcast token";
const NATIVE_P3_FINAL_TEXT: &str = "native P3 plus coordination complete";

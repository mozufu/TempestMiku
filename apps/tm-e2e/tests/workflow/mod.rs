use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, BoxStream};
use serde_json::{json, to_value};
use tm_agents::{ActorBudget, ActorId, ActorRecord, ActorStatus, FailureReason, MailboxRegistry};
use tm_core::{
    AgentConfig, CellBudget, ChatRequest, Error as CoreError, LlmClient, Message,
    Result as CoreResult, StreamEvent,
};
use tm_e2e::{
    E2eConfig, E2eEvent, EVIDENCE_SCHEMA_VERSION, MikuClient, RecordOptions, ScriptedSpeaker,
    WORKFLOW_RECORD_SCHEMA_VERSION, WorkflowOptions, run_actor_smoke, run_drive_smoke,
    run_record_api, run_record_native_coding, run_workflow, write_workflow_record,
};
use tm_host::{CapabilityGrants, FsMode, LinkedFolderConfig, LinkedFolders};
use tm_lang::{TmSandbox, TmSandboxOptions};
use tm_server::{
    AppState, ApprovalBroker, ApprovalOption, ApprovalPrompt, ApprovalStatus, AuthConfig,
    ChatActorExecutor, CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult,
    EchoChatRunner, HttpApprovalPolicy, InMemoryStore, ModeId, NativeApprovalMode, NativeTmBackend,
    RosterCodingEventSink, ServerError, StoreEvent, StoreMemoryProvider, app,
};

mod actor_native;
mod basic_api;
mod drive_recorded;
mod fakes;
mod server;
mod support;

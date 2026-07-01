use std::{
    sync::{Arc, Mutex},
    thread,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tm_core::{Agent, AgentConfig, EventSink, LlmClient};
use tm_host::{
    ApprovalDecision as HostApprovalDecision, ApprovalPolicy, DefaultDenyApprovalPolicy, HostError,
};
use tm_sandbox::{DenoSandbox, DenoSandboxOptions};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    ApprovalBroker, ApprovalOption, ApprovalPrompt, ApprovalStatus, CodingBackend, CodingEventSink,
    CodingTurn, CodingTurnResult, DetailedApprovalOutcome, Result, ServerError, StoreEvent,
};

const NATIVE_DENO_BACKEND: &str = "native-deno";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeApprovalMode {
    Deny,
    Manual,
}

impl NativeApprovalMode {
    pub fn parse(mode: &str) -> Result<Self> {
        match mode {
            "" | "deny" => Ok(Self::Deny),
            "manual" => Ok(Self::Manual),
            other => Err(ServerError::InvalidRequest(format!(
                "unsupported approval mode {other}"
            ))),
        }
    }
}

struct NativeDenoRequest {
    turn: CodingTurn,
    sink: Arc<dyn CodingEventSink>,
    reply: tokio::sync::oneshot::Sender<Result<CodingTurnResult>>,
}

pub struct NativeDenoBackend {
    sender: Mutex<std::sync::mpsc::Sender<NativeDenoRequest>>,
}

impl NativeDenoBackend {
    pub fn new(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        base_options: DenoSandboxOptions,
        approval_mode: NativeApprovalMode,
        approval_broker: Arc<ApprovalBroker>,
    ) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<NativeDenoRequest>();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("native deno backend runtime builds");
            for request in receiver {
                let result = runtime.block_on(run_native_turn(
                    Arc::clone(&llm),
                    cfg.clone(),
                    base_options.clone(),
                    approval_mode,
                    Arc::clone(&approval_broker),
                    request.turn,
                    request.sink,
                ));
                let _ = request.reply.send(result);
            }
        });
        Self {
            sender: Mutex::new(sender),
        }
    }
}

#[async_trait]
impl CodingBackend for NativeDenoBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<CodingTurnResult> {
        let (reply, response) = tokio::sync::oneshot::channel();
        {
            let sender = self
                .sender
                .lock()
                .map_err(|_| ServerError::Store("native deno sender lock poisoned".to_string()))?;
            sender
                .send(NativeDenoRequest { turn, sink, reply })
                .map_err(|_| ServerError::Store("native deno worker stopped".to_string()))?;
        }
        response
            .await
            .map_err(|_| ServerError::Store("native deno worker dropped response".to_string()))?
    }
}

async fn run_native_turn(
    llm: Arc<dyn LlmClient>,
    mut cfg: AgentConfig,
    mut options: DenoSandboxOptions,
    approval_mode: NativeApprovalMode,
    approval_broker: Arc<ApprovalBroker>,
    turn: CodingTurn,
    sink: Arc<dyn CodingEventSink>,
) -> Result<CodingTurnResult> {
    options.session_id = turn.session_id.to_string();
    options.approval_policy = match approval_mode {
        NativeApprovalMode::Deny => Arc::new(DefaultDenyApprovalPolicy),
        NativeApprovalMode::Manual => Arc::new(HttpApprovalPolicy {
            broker: approval_broker,
            session_id: turn.session_id,
            sink: Arc::clone(&sink),
        }),
    };
    cfg.system_prompt = turn.system_prompt.clone();

    let sandbox = Arc::new(DenoSandbox::new(options));
    let agent = Agent::new(llm, sandbox, cfg);
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let event_sink = ForwardingEventSink { sender: event_tx };
    let forwarder = tokio::spawn(forward_events(event_rx, Arc::clone(&sink)));
    let result = agent
        .run(&turn.user_prompt, &event_sink)
        .await
        .map_err(|err| ServerError::Store(err.to_string()));
    drop(event_sink);
    forwarder.await.map_err(|err| {
        ServerError::Store(format!("native deno event forwarder failed: {err}"))
    })??;
    Ok(CodingTurnResult {
        final_text: result?,
        transcript_artifact: None,
    })
}

struct HttpApprovalPolicy {
    broker: Arc<ApprovalBroker>,
    session_id: Uuid,
    sink: Arc<dyn CodingEventSink>,
}

#[async_trait]
impl ApprovalPolicy for HttpApprovalPolicy {
    async fn request(
        &self,
        action: &str,
        timeout: std::time::Duration,
    ) -> tm_host::Result<HostApprovalDecision> {
        let action = action.to_string();
        let detailed = self
            .broker
            .request_permission_detailed_for_backend(
                self.session_id,
                NATIVE_DENO_BACKEND,
                approval_prompt(&action),
                timeout,
                Arc::clone(&self.sink),
            )
            .await
            .map_err(|err| HostError::HostCall(err.to_string()))?;
        host_decision(&action, detailed)
    }
}

fn approval_prompt(action: &str) -> ApprovalPrompt {
    ApprovalPrompt {
        action: action.to_string(),
        scope: json!({
            "action": action,
            "capability": action.split_whitespace().next().unwrap_or(action),
        }),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Allow once".to_string(),
                kind: "allow_once".to_string(),
            },
            ApprovalOption {
                option_id: "reject".to_string(),
                name: "Reject once".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}

fn host_decision(
    action: &str,
    detailed: DetailedApprovalOutcome,
) -> tm_host::Result<HostApprovalDecision> {
    match detailed.status {
        ApprovalStatus::Approved => Ok(HostApprovalDecision::Approved),
        ApprovalStatus::Denied | ApprovalStatus::Cancelled => Ok(HostApprovalDecision::Denied),
        ApprovalStatus::TimedOut => Err(HostError::ApprovalTimeout(action.to_string())),
    }
}

enum NativeEvent {
    Text(String),
    ToolCall(String),
    CellStart(String),
    CellResult(String),
    Final(String),
}

struct ForwardingEventSink {
    sender: mpsc::UnboundedSender<NativeEvent>,
}

impl ForwardingEventSink {
    fn send(&self, event: NativeEvent) {
        let _ = self.sender.send(event);
    }
}

impl EventSink for ForwardingEventSink {
    fn on_text(&self, delta: &str) {
        self.send(NativeEvent::Text(delta.to_string()));
    }

    fn on_tool_call(&self, name: &str) {
        self.send(NativeEvent::ToolCall(name.to_string()));
    }

    fn on_cell_start(&self, code: &str) {
        self.send(NativeEvent::CellStart(code.to_string()));
    }

    fn on_cell_result(&self, shaped: &str) {
        self.send(NativeEvent::CellResult(shaped.to_string()));
    }

    fn on_final(&self, text: &str) {
        self.send(NativeEvent::Final(text.to_string()));
    }
}

async fn forward_events(
    mut events: mpsc::UnboundedReceiver<NativeEvent>,
    sink: Arc<dyn CodingEventSink>,
) -> Result<()> {
    while let Some(event) = events.recv().await {
        let (event_type, payload): (&str, Value) = match event {
            NativeEvent::Text(delta) => (
                "text",
                serde_json::to_value(StoreEvent::Text { delta })
                    .map_err(|err| ServerError::Store(err.to_string()))?,
            ),
            NativeEvent::ToolCall(name) => ("tool_call", json!({ "name": name })),
            NativeEvent::CellStart(code) => ("cell_start", json!({ "code": code })),
            NativeEvent::CellResult(shaped) => ("cell_result", json!({ "shaped": shaped })),
            NativeEvent::Final(text) => (
                "final",
                serde_json::to_value(StoreEvent::Final { text })
                    .map_err(|err| ServerError::Store(err.to_string()))?,
            ),
        };
        sink.emit(event_type, payload).await?;
    }
    Ok(())
}

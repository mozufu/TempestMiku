use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use parking_lot::Mutex as ParkingMutex;
use serde_json::{Value, json};
use tm_core::{
    AgentConfig, ChatRequest, Error as CoreError, LlmClient, Message, Role, StreamEvent,
};
use tm_lang::TmSandboxOptions;
use uuid::Uuid;

use super::super::{NativeApprovalMode, NativeTmBackend, NativeTmBackendOptions};
use crate::{
    ApprovalBroker, CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult, Result,
    ServerError,
};

pub(super) struct StatefulLlm {
    pub(super) requests: ParkingMutex<Vec<Vec<Message>>>,
}

impl StatefulLlm {
    pub(super) fn new() -> Self {
        Self {
            requests: ParkingMutex::new(Vec::new()),
        }
    }

    pub(super) fn tool_results(&self) -> Vec<String> {
        self.requests
            .lock()
            .iter()
            .filter_map(|messages| {
                messages
                    .iter()
                    .find(|message| message.role == Role::Tool)
                    .map(|message| message.content.clone())
            })
            .collect()
    }
}

#[async_trait]
impl LlmClient for StatefulLlm {
    async fn chat_stream(
        &self,
        request: &ChatRequest,
    ) -> tm_core::Result<BoxStream<'static, tm_core::Result<StreamEvent>>> {
        self.requests.lock().push(request.messages.clone());
        let events = if request
            .messages
            .last()
            .is_some_and(|message| message.role == Role::Tool)
        {
            vec![
                StreamEvent::Text("done".to_string()),
                StreamEvent::Finish {
                    reason: Some("stop".to_string()),
                },
            ]
        } else {
            let code = if request.messages.iter().any(|message| {
                message.role == Role::User && message.content == "increment native state"
            }) {
                "let retained = retained + 1;\nretained"
            } else {
                "let retained = 1;\nretained"
            };
            vec![
                StreamEvent::ToolCall {
                    index: 0,
                    id: Some("call_state".to_string()),
                    name: Some("execute".to_string()),
                    arguments: Some(
                        json!({
                            "code": code
                        })
                        .to_string(),
                    ),
                },
                StreamEvent::Finish {
                    reason: Some("tool_calls".to_string()),
                },
            ]
        };
        Ok(Box::pin(stream::iter(
            events.into_iter().map(Ok::<StreamEvent, CoreError>),
        )))
    }
}

pub(super) struct BurstLlm {
    events: ParkingMutex<VecDeque<Vec<StreamEvent>>>,
}

impl BurstLlm {
    pub(super) fn new(event_count: usize) -> Self {
        let mut events = (0..event_count)
            .map(|index| StreamEvent::Text(format!("delta-{index}")))
            .collect::<Vec<_>>();
        events.push(StreamEvent::Finish {
            reason: Some("stop".to_string()),
        });
        Self {
            events: ParkingMutex::new(VecDeque::from([events])),
        }
    }
}

#[async_trait]
impl LlmClient for BurstLlm {
    async fn chat_stream(
        &self,
        _request: &ChatRequest,
    ) -> tm_core::Result<BoxStream<'static, tm_core::Result<StreamEvent>>> {
        let events = self.events.lock().pop_front().unwrap_or_default();
        Ok(Box::pin(stream::iter(
            events.into_iter().map(Ok::<StreamEvent, CoreError>),
        )))
    }
}

#[derive(Default)]
pub(super) struct RecordingCodingSink {
    pub(super) events: ParkingMutex<Vec<(String, Value)>>,
    next_seq: AtomicI64,
    delay: Duration,
    event_to_fail: Option<&'static str>,
    event_failures: AtomicUsize,
    runtime_reset_failures: AtomicUsize,
    pub(super) runtime_reset_attempts: AtomicUsize,
}

impl RecordingCodingSink {
    pub(super) fn slow(delay: Duration) -> Self {
        Self {
            delay,
            ..Self::default()
        }
    }

    pub(super) fn fail_runtime_reset_once() -> Self {
        Self {
            runtime_reset_failures: AtomicUsize::new(1),
            ..Self::default()
        }
    }

    pub(super) fn fail_binding_once() -> Self {
        Self {
            event_to_fail: Some("binding_committed"),
            event_failures: AtomicUsize::new(1),
            ..Self::default()
        }
    }

    pub(super) fn event_types(&self) -> Vec<String> {
        self.events
            .lock()
            .iter()
            .map(|(event_type, _)| event_type.clone())
            .collect()
    }
}

#[async_trait]
impl CodingEventSink for RecordingCodingSink {
    async fn emit(&self, event_type: &str, payload_json: Value) -> Result<crate::SessionEvent> {
        let should_fail_event = self.event_to_fail == Some(event_type)
            && self
                .event_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok();
        if should_fail_event {
            return Err(ServerError::Store(format!(
                "{event_type} persistence failed"
            )));
        }
        if event_type == "runtime_reset" {
            self.runtime_reset_attempts.fetch_add(1, Ordering::SeqCst);
            let should_fail = self
                .runtime_reset_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok();
            if should_fail {
                return Err(ServerError::Store(
                    "runtime_reset persistence failed".to_string(),
                ));
            }
        }
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        self.events
            .lock()
            .push((event_type.to_string(), payload_json.clone()));
        Ok(crate::SessionEvent::new(
            Uuid::nil(),
            self.next_seq.fetch_add(1, Ordering::SeqCst),
            event_type,
            payload_json,
            chrono::Utc::now(),
        ))
    }
}

pub(super) fn coding_turn(session_id: Uuid) -> CodingTurn {
    CodingTurn {
        session_id,
        durable_turn_id: None,
        user_prompt: "advance native state".to_string(),
        system_prompt: "native test system".to_string(),
        mode: tm_modes::ModeId::from("serious_engineer"),
        owner_subject: "brian".to_string(),
        project_id: Some("tempestmiku".to_string()),
        memory_scope: "project:tempestmiku".to_string(),
        capabilities: Vec::new(),
        prior_messages: Vec::new(),
        resource_handlers: Vec::new(),
    }
}

pub(super) fn backend(
    llm: Arc<dyn LlmClient>,
    artifact_root: &std::path::Path,
    options: NativeTmBackendOptions,
) -> NativeTmBackend {
    NativeTmBackend::new_with_options(
        llm,
        AgentConfig {
            model: "fake".to_string(),
            max_turns: 3,
            ..AgentConfig::default()
        },
        TmSandboxOptions {
            artifact_root: artifact_root.to_path_buf(),
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Deny,
        Arc::new(ApprovalBroker::default()),
        options,
    )
}

pub(super) fn options(
    session_ttl: Duration,
    event_channel_capacity: usize,
) -> NativeTmBackendOptions {
    NativeTmBackendOptions {
        shard_count: 2,
        session_ttl,
        event_channel_capacity,
    }
}

pub(super) async fn run_turn(
    backend: &NativeTmBackend,
    turn: CodingTurn,
    sink: Arc<RecordingCodingSink>,
) -> Result<CodingTurnResult> {
    let sink: Arc<dyn CodingEventSink> = sink;
    backend.run_turn(turn, sink).await
}

use super::*;
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use chrono::Utc;
use std::{collections::VecDeque, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use parking_lot::Mutex;
use tm_core::{
    AgentConfig, CellBudget, ChatRequest, Error as CoreError, LlmClient, Message,
    Result as CoreResult, Role, StreamEvent, ToolSpec,
};
use tm_host::{
    CapabilityGrants, FsMode, HostError, InvocationCtx, LinkedFolderConfig, LinkedFolders,
    ResourceEntry, ResourceHandler, ResourceRegistry,
};
use tm_lang::TmSandboxOptions;
use tower::ServiceExt;

use crate::{
    AgentChatRunner, ApprovalOption, ApprovalPrompt, ChatRunner, ChatTurn, CodingEventSink,
    CodingTurn, CodingTurnResult, EchoChatRunner, InMemoryStore, NativeApprovalMode,
    NativeTmBackend, NativeTmBackendOptions, PostgresStore, StoreMemoryProvider,
    auth::ForwardedAuthConfig,
    store::{ProfileFactRecord, RecallChunkRecord},
};
use tm_modes::KNOWN_SKILLS;

fn test_app(persona: ModesConfig, auth: AuthConfig) -> (Router, Arc<InMemoryStore>) {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: std::env::current_dir().expect("test workspace path"),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .expect("test linked project");
    let state = AppState::new(store.clone(), memory, chat, persona, auth)
        .with_linked_folders(linked)
        .with_coding_backend(Arc::new(EchoCodingBackend));
    (app(state), store)
}

fn postgres_test_dsn() -> Option<String> {
    (std::env::var("TM_POSTGRES_TESTS").ok().as_deref() == Some("1"))
        .then(|| {
            std::env::var("TM_TEST_DATABASE_URL")
                .or_else(|_| std::env::var("TM_DATABASE_URL"))
                .ok()
        })
        .flatten()
}

async fn postgres_test_app() -> Option<(Router, Arc<PostgresStore>)> {
    let dsn = postgres_test_dsn()?;
    let store = Arc::new(PostgresStore::connect(&dsn).await.unwrap());
    store.configure_owner_subject("brian").await.unwrap();
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    );
    Some((app(state), store))
}

fn test_app_with_state(
    state: AppState<InMemoryStore, StoreMemoryProvider<InMemoryStore>, EchoChatRunner>,
) -> (Router, Arc<InMemoryStore>) {
    let store = Arc::clone(&state.store);
    (app(state), store)
}

struct ScriptedBackend {
    kind: ScriptedBackendKind,
}

enum ScriptedBackendKind {
    Events,
    Approval { broker: Arc<ApprovalBroker> },
    Artifact { root: PathBuf },
}

struct ScriptedLlm {
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    requests: Mutex<Vec<Vec<Message>>>,
    tools: Mutex<Vec<Vec<ToolSpec>>>,
}

impl ScriptedLlm {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(Vec::new()),
            tools: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    async fn chat_stream(
        &self,
        req: &ChatRequest,
    ) -> CoreResult<BoxStream<'static, CoreResult<StreamEvent>>> {
        self.requests.lock().push(req.messages.clone());
        self.tools.lock().push(req.tools.clone());
        let script = self.scripts.lock().pop_front().unwrap_or_default();
        Ok(Box::pin(stream::iter(
            script.into_iter().map(Ok::<StreamEvent, CoreError>),
        )))
    }
}

impl ScriptedBackend {
    fn events() -> Self {
        Self {
            kind: ScriptedBackendKind::Events,
        }
    }

    fn approval(broker: Arc<ApprovalBroker>) -> Self {
        Self {
            kind: ScriptedBackendKind::Approval { broker },
        }
    }

    fn artifact(root: PathBuf) -> Self {
        Self {
            kind: ScriptedBackendKind::Artifact { root },
        }
    }
}

#[async_trait]
impl CodingBackend for ScriptedBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<CodingTurnResult> {
        match &self.kind {
            ScriptedBackendKind::Events => {
                assert_eq!(turn.mode, ModeId::from("serious_engineer"));
                assert_eq!(turn.project_id, None);
                assert_eq!(turn.memory_scope, "global");
                sink.emit("text", json!({ "event": "text", "delta": "working" }))
                    .await?;
                sink.emit(
                    "diff",
                    json!({
                        "backend": "test",
                        "path": "crates/tm-modes/src/lib.rs",
                        "oldText": null,
                        "newText": "patched",
                    }),
                )
                .await?;
                let artifact = tm_artifacts::ArtifactRef {
                    uri: "artifact://0".to_string(),
                    id: "0".to_string(),
                    kind: "text".to_string(),
                    mime: "application/jsonl".to_string(),
                    title: Some("test transcript".to_string()),
                    size_bytes: 2,
                    preview: "{}".to_string(),
                };
                sink.emit(
                    "artifact",
                    json!({ "backend": "test", "artifact": artifact }),
                )
                .await?;
                let final_text = "Completed via scripted backend with artifact://0".to_string();
                sink.emit(
                    "final",
                    serde_json::to_value(StoreEvent::Final {
                        text: final_text.clone(),
                    })?,
                )
                .await?;
                Ok(CodingTurnResult {
                    final_text,
                    transcript_artifact: None,
                })
            }
            ScriptedBackendKind::Approval { broker } => {
                let outcome = broker
                    .request_permission(
                        turn.session_id,
                        ApprovalPrompt {
                            action: "write file".to_string(),
                            scope: json!({ "path": "crates/tm-modes/src/lib.rs" }),
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
                        },
                        Duration::from_secs(5),
                        sink.clone(),
                    )
                    .await?;
                let final_text = format!("approval outcome: {outcome:?} artifact://approval");
                sink.emit(
                    "final",
                    serde_json::to_value(StoreEvent::Final {
                        text: final_text.clone(),
                    })?,
                )
                .await?;
                Ok(CodingTurnResult {
                    final_text,
                    transcript_artifact: None,
                })
            }
            ScriptedBackendKind::Artifact { root } => {
                let store = tm_artifacts::ArtifactStore::open(root, turn.session_id.to_string())
                    .map_err(|err| ServerError::Store(err.to_string()))?;
                let artifact = store
                    .put_text(
                        "{\"line\":\"scripted transcript\"}\n",
                        Some("scripted transcript".to_string()),
                        "application/jsonl",
                    )
                    .map_err(|err| ServerError::Store(err.to_string()))?;
                sink.emit(
                    "artifact",
                    json!({ "backend": "test", "artifact": artifact }),
                )
                .await?;
                let final_text = "Completed via scripted backend with artifact://0".to_string();
                sink.emit(
                    "final",
                    serde_json::to_value(StoreEvent::Final {
                        text: final_text.clone(),
                    })?,
                )
                .await?;
                Ok(CodingTurnResult {
                    final_text,
                    transcript_artifact: None,
                })
            }
        }
    }
}

#[derive(Default)]
struct RecordingChatRunner {
    turns: Arc<Mutex<Vec<ChatTurn>>>,
}

#[async_trait]
impl ChatRunner for RecordingChatRunner {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn tm_core::EventSink + Send + Sync>,
    ) -> Result<String> {
        self.turns.lock().push(turn);
        let text = "recorded chat turn".to_string();
        sink.on_text(&text);
        sink.on_final(&text);
        Ok(text)
    }
}

#[derive(Default)]
struct RecordingBackend {
    turns: Arc<Mutex<Vec<CodingTurn>>>,
}

#[async_trait]
impl CodingBackend for RecordingBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<CodingTurnResult> {
        self.turns.lock().push(turn);
        let final_text = "recorded coding turn".to_string();
        sink.emit(
            "final",
            serde_json::to_value(StoreEvent::Final {
                text: final_text.clone(),
            })?,
        )
        .await?;
        Ok(CodingTurnResult {
            final_text,
            transcript_artifact: None,
        })
    }
}

/// Coding-mode analogue of `EchoChatRunner`: completes any coding turn with a
/// deterministic non-empty response and a replayable `final` event. Used by the
/// shared `test_app` so serious-engineer turns dispatch and complete under the fail-closed backend
/// contract without asserting on a specific backend.
struct EchoCodingBackend;

#[async_trait]
impl CodingBackend for EchoCodingBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<CodingTurnResult> {
        let final_text = format!("Miku heard: {}", turn.user_prompt);
        sink.emit("text", json!({ "event": "text", "delta": &final_text }))
            .await?;
        sink.emit(
            "final",
            serde_json::to_value(StoreEvent::Final {
                text: final_text.clone(),
            })?,
        )
        .await?;
        Ok(CodingTurnResult {
            final_text,
            transcript_artifact: None,
        })
    }
}

async fn response_json(res: axum::response::Response) -> Value {
    let body = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn create(app: &Router) -> CreateSessionResponse {
    create_with_body(app, Body::empty()).await
}

async fn create_project_session(app: &Router) -> CreateSessionResponse {
    create_with_body(
        app,
        Body::from(
            r#"{"mode":"serious_engineer","projectId":"tempestmiku","memoryPolicy":"project"}"#,
        ),
    )
    .await
}

async fn create_with_body(app: &Router, body: Body) -> CreateSessionResponse {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn message_body(content: impl Into<String>) -> Body {
    Body::from(
        json!({
            "clientMessageId": Uuid::new_v4().to_string(),
            "content": content.into(),
        })
        .to_string(),
    )
}

async fn accepted_turn_id(response: axum::response::Response) -> Uuid {
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    response_json(response).await["turnId"]
        .as_str()
        .expect("queued turn id")
        .parse()
        .expect("turn id is a UUID")
}

async fn wait_for_turn(app: &Router, session_id: Uuid, turn_id: Uuid) -> Value {
    for _ in 0..500 {
        let status = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sessions/{session_id}/turns/{turn_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        let turn = response_json(status).await;
        match turn["status"].as_str() {
            Some("completed") | Some("failed") => return turn,
            _ => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
    panic!("durable turn {turn_id} did not finish")
}

async fn post_user_message(app: &Router, session_id: Uuid, content: &str) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/messages"))
                .header("content-type", "application/json")
                .body(message_body(content))
                .unwrap(),
        )
        .await
        .unwrap();
    let turn_id = accepted_turn_id(res).await;
    let turn = wait_for_turn(app, session_id, turn_id).await;
    assert_eq!(turn["status"], json!("completed"), "{turn}");
}

async fn post_memory_proposal(
    app: &Router,
    session_id: Uuid,
    payload: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/memory/proposals"))
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn get_session_resource_json(
    app: &Router,
    session_id: Uuid,
    endpoint: &str,
    uri: &str,
) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{session_id}/resources/{endpoint}?uri={uri}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{endpoint} {uri}");
    response_json(response).await
}

async fn resolve_test_approval(app: &Router, session_id: Uuid, approval_id: Uuid, decision: &str) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/approvals/{approval_id}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "decision": decision }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    if status != StatusCode::OK {
        panic!(
            "approval resolution failed with {status}: {}",
            response_json(res).await
        );
    }
}

fn write_mode_assets_fixture(root: &std::path::Path) {
    std::fs::write(
        root.join("SOUL.md"),
        "# Fixture SOUL\nIdentity constant and Mode Router fixture.",
    )
    .unwrap();
    std::fs::write(
        root.join("modes.json"),
        serde_json::to_string_pretty(&ModesConfig::default().load_assets().modes).unwrap(),
    )
    .unwrap();
    for skill in KNOWN_SKILLS {
        let dir = root.join("skills").join(skill);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("# {skill}\n{skill} fixture body"),
        )
        .unwrap();
    }
}

async fn wait_for_event_payload<S>(store: &Arc<S>, session_id: Uuid, event_type: &str) -> Value
where
    S: Store,
{
    for _ in 0..100 {
        if let Some(event) = store
            .as_ref()
            .events_after(session_id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|event| event.event_type == event_type)
        {
            return event.payload_json;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let events = store.as_ref().events_after(session_id, None).await.unwrap();
    panic!(
        "event {event_type} was not persisted; saw {:?}",
        events
            .iter()
            .map(|event| (&event.event_type, &event.payload_json))
            .collect::<Vec<_>>()
    )
}

async fn wait_for_nth_event_payload<S>(
    store: &Arc<S>,
    session_id: Uuid,
    event_type: &str,
    index: usize,
) -> Value
where
    S: Store,
{
    for _ in 0..100 {
        if let Some(event) = store
            .as_ref()
            .events_after(session_id, None)
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == event_type)
            .nth(index)
        {
            return event.payload_json;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("event {event_type} #{index} was not persisted")
}

async fn wait_for_write_proposal_status<S>(
    store: &Arc<S>,
    session_id: Uuid,
    status: MemoryWriteStatus,
) -> Value
where
    S: Store,
{
    for _ in 0..100 {
        if let Some(event) = store
            .as_ref()
            .events_after(session_id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|event| {
                event.event_type == "write_proposal"
                    && event.payload_json["status"] == json!(status)
            })
        {
            return event.payload_json;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "write_proposal status {} was not persisted",
        status.as_str()
    )
}

mod auth;
mod coding_native;
mod core_flow;
mod egress;
mod evolution_review;
mod mcp;
mod memory;
mod pairing;
mod project_resources;
mod router;
mod skill_lifecycle;

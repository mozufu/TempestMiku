use std::{convert::Infallible, path::PathBuf, sync::Arc, time::Duration};

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

use crate::{
    ApprovalBroker, AuthConfig, ChatRunner, CodingBackend, CodingTurn, MemoryProvider, Mode,
    NewSession, PersistingEventSink, PersonaConfig, ResolveApprovalRequest, Result, ServerError,
    SessionEvent, Store, StoreCodingEventSink, StoreEvent, store::RecallChunkRecord, webui,
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
        .route("/sessions", post(create_session::<S, M, C>))
        .route("/sessions/:id/events", get(session_events::<S, M, C>))
        .route("/sessions/:id/messages", post(post_message::<S, M, C>))
        .route(
            "/sessions/:id/approvals/:approval_id",
            post(resolve_approval::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/artifacts",
            get(list_artifacts::<S, M, C>),
        )
        .route(
            "/sessions/:id/resources/artifacts/:artifact_id",
            get(read_artifact::<S, M, C>),
        )
        .merge(webui::routes::<AppState<S, M, C>>())
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

#[derive(Debug, Default, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub mode: Mode,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub id: Uuid,
    pub mode: Mode,
    pub persona_status: crate::PersonaStatus,
    pub label: String,
    pub voice_cap: String,
    pub default_scope: String,
}

async fn create_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    headers: HeaderMap,
    payload: Option<Json<CreateSessionRequest>>,
) -> Result<Json<CreateSessionResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let mode = payload
        .map(|Json(payload)| payload.mode)
        .unwrap_or_default();
    let persona_status = state.persona.load_status();
    let session = state
        .store
        .create_session(NewSession {
            mode,
            persona_status: persona_status.clone(),
        })
        .await?;
    let mode_event = state
        .store
        .append_event(
            session.id,
            "mode",
            serde_json::to_value(StoreEvent::Mode {
                mode: session.mode,
                label: session.mode.label().to_string(),
                voice_cap: session.mode.voice_cap().to_string(),
                persona_status: persona_status.clone(),
            })?,
        )
        .await?;
    let _ = state.sender(session.id).send(mode_event);
    Ok(Json(CreateSessionResponse {
        id: session.id,
        mode: session.mode,
        persona_status,
        label: session.mode.label().to_string(),
        voice_cap: session.mode.voice_cap().to_string(),
        default_scope: session.mode.default_scope().to_string(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct PostMessageRequest {
    pub content: String,
    #[serde(default = "default_subject")]
    pub subject: String,
    #[serde(default)]
    pub scope: Option<String>,
}

fn default_subject() -> String {
    "brian".to_string()
}

async fn post_message<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<PostMessageRequest>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let scope = payload
        .scope
        .clone()
        .unwrap_or_else(|| session.mode.default_scope().to_string());
    state
        .store
        .append_message(session_id, "user", &payload.content)
        .await?;
    let memory = state
        .memory
        .context_for_turn(&payload.subject, &scope, &payload.content)
        .await?;
    let user_prompt = if memory.is_empty() {
        payload.content.clone()
    } else {
        format!(
            "{}\n\n[recall]\n{}",
            payload.content,
            memory.render_prompt_block()
        )
    };
    let response = if matches!(session.mode, Mode::SeriousEngineer | Mode::Handoff) {
        if let Some(backend) = &state.coding_backend {
            let sink = Arc::new(StoreCodingEventSink::new(
                session_id,
                Arc::clone(&state.store),
                state.sender(session_id),
            ));
            backend
                .run_turn(
                    CodingTurn {
                        session_id,
                        user_prompt,
                        mode: session.mode,
                        scope: scope.clone(),
                    },
                    sink,
                )
                .await?
                .final_text
        } else {
            let sink = Arc::new(PersistingEventSink::new(
                session_id,
                Arc::clone(&state.store),
                state.sender(session_id),
            ));
            let response = state.chat.run_turn(user_prompt, sink.clone()).await?;
            sink.flush().await?;
            response
        }
    } else {
        let sink = Arc::new(PersistingEventSink::new(
            session_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        ));
        let response = state.chat.run_turn(user_prompt, sink.clone()).await?;
        sink.flush().await?;
        response
    };

    state
        .store
        .append_message(session_id, "assistant", &response)
        .await?;
    if matches!(session.mode, Mode::SeriousEngineer | Mode::Handoff) && !response.trim().is_empty()
    {
        state
            .store
            .add_recall_chunk(RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: scope.clone(),
                text: format!(
                    "Project summary/open loop from session {session_id}: {}",
                    response.trim()
                ),
                source: format!("session:{session_id}:assistant"),
                created_at: Utc::now(),
            })
            .await?;
    }
    Ok(Json(json!({ "status": "accepted" })))
}

async fn resolve_approval<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, approval_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<ResolveApprovalRequest>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    state.store.get_session(session_id).await?;
    state
        .approval_broker
        .resolve(session_id, approval_id, payload)?;
    Ok(Json(json!({ "status": "resolved" })))
}

#[derive(Debug, Default, Deserialize)]
struct ArtifactReadQuery {
    selector: Option<String>,
}

async fn list_artifacts<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    state.store.get_session(session_id).await?;
    let store = tm_artifacts::ArtifactStore::open(&state.artifact_root, session_id.to_string())
        .map_err(|err| ServerError::Store(err.to_string()))?;
    Ok(Json(serde_json::to_value(store.list())?))
}

async fn read_artifact<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, artifact_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
    Query(query): Query<ArtifactReadQuery>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    state.store.get_session(session_id).await?;
    let store = tm_artifacts::ArtifactStore::open(&state.artifact_root, session_id.to_string())
        .map_err(|err| ServerError::Store(err.to_string()))?;
    let uri = format!("artifact://{artifact_id}");
    let content = store
        .read(&uri, query.selector.as_deref())
        .map_err(map_artifact_error)?;
    Ok(Json(serde_json::to_value(content)?))
}

fn map_artifact_error(err: tm_artifacts::ArtifactError) -> ServerError {
    match err {
        tm_artifacts::ArtifactError::NotFound { uri, .. } => ServerError::NotFound(uri),
        tm_artifacts::ArtifactError::InvalidUri(uri) => ServerError::InvalidRequest(uri),
        tm_artifacts::ArtifactError::InvalidSelector(selector) => {
            ServerError::InvalidRequest(selector)
        }
        tm_artifacts::ArtifactError::Io(err) => ServerError::Store(err.to_string()),
    }
}
async fn session_events<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<impl IntoResponse>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok());
    let receiver = state.sender(session_id).subscribe();
    let replay = state.store.events_after(session_id, last_event_id).await?;
    let replay_finished = replay.iter().any(|event| event.event_type == "final");
    let live = BroadcastStream::new(receiver).filter_map(|event| async move { event.ok() });
    let replay_stream = futures::stream::iter(replay);
    let event_stream: BoxStream<'static, SessionEvent> = if replay_finished {
        replay_stream.boxed()
    } else {
        replay_stream.chain(live).boxed()
    };
    let stream = event_stream.map(|event| Ok::<Event, Infallible>(sse_event(event)));
    Ok(Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15))))
}

fn sse_event(event: SessionEvent) -> Event {
    Event::default()
        .id(event.seq.to_string())
        .event(event.event_type)
        .json_data(event.payload_json)
        .expect("serializing stored JSON payload cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use chrono::Utc;
    use std::path::PathBuf;

    use async_trait::async_trait;
    use tower::ServiceExt;

    use crate::{
        ApprovalOption, ApprovalPrompt, CodingEventSink, CodingTurn, CodingTurnResult,
        EchoChatRunner, InMemoryStore, StoreMemoryProvider,
        auth::ForwardedAuthConfig,
        store::{ProfileFactRecord, RecallChunkRecord},
    };

    fn test_app(persona: PersonaConfig, auth: AuthConfig) -> (Router, Arc<InMemoryStore>) {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let state = AppState::new(store.clone(), memory, chat, persona, auth);
        (app(state), store)
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
                    assert_eq!(turn.mode, Mode::SeriousEngineer);
                    assert_eq!(turn.scope, "project:tempestmiku");
                    sink.emit("text", json!({ "event": "text", "delta": "working" }))
                        .await?;
                    sink.emit(
                        "diff",
                        json!({
                            "backend": "test",
                            "path": "crates/tm-persona/src/lib.rs",
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
                                scope: json!({ "path": "crates/tm-persona/src/lib.rs" }),
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
                    let store =
                        tm_artifacts::ArtifactStore::open(root, turn.session_id.to_string())
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

    async fn response_json(res: axum::response::Response) -> Value {
        let body = axum::body::to_bytes(res.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn create(app: &Router) -> CreateSessionResponse {
        create_with_body(app, Body::empty()).await
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

    #[tokio::test]
    async fn session_creation_message_append_event_append_and_replay_work() {
        let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
        let session = create(&app).await;
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"hello"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let all = store.events_after(session.id, None).await.unwrap();
        assert_eq!(
            all.iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["mode", "text", "final"]
        );
        assert_eq!(all[0].payload_json["voice_cap"], serde_json::json!("中"));
        let replay = store.events_after(session.id, Some(1)).await.unwrap();
        assert_eq!(
            replay
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["text", "final"]
        );
    }

    #[tokio::test]
    async fn missing_persona_path_boots_degraded_with_warning() {
        let (app, _) = test_app(
            PersonaConfig::from_path("/definitely/missing/tempestmiku/persona"),
            AuthConfig::NoAuth,
        );
        let session = create(&app).await;
        match session.persona_status {
            crate::PersonaStatus::Degraded { warning } => assert!(warning.contains("missing")),
            crate::PersonaStatus::Loaded { .. } => panic!("missing path must degrade"),
        }
    }

    #[tokio::test]
    async fn memory_context_injects_profile_facts_and_recall_chunks() {
        let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
        store
            .add_profile_fact(ProfileFactRecord {
                id: Uuid::new_v4(),
                subject: "brian".to_string(),
                predicate: "prefers".to_string(),
                object: "boring Rust".to_string(),
                confidence: 0.9,
                provenance: "test".to_string(),
                valid_from: Utc::now(),
                valid_to: None,
            })
            .await
            .unwrap();
        store
            .add_recall_chunk(RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: "global".to_string(),
                text: "hello project open loop".to_string(),
                source: "test".to_string(),
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        let session = create(&app).await;
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"hello"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let events = store.events_after(session.id, None).await.unwrap();
        let final_event = events
            .iter()
            .find(|event| event.event_type == "final")
            .unwrap();
        assert!(final_event.payload_json.to_string().contains("boring Rust"));
        assert!(
            final_event
                .payload_json
                .to_string()
                .contains("hello project open loop")
        );
    }

    #[tokio::test]
    async fn serious_engineer_session_uses_project_scope_and_recalls_next_session() {
        let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
        let session_a = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
        assert_eq!(session_a.voice_cap, "關");
        assert_eq!(session_a.default_scope, "project:tempestmiku");
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session_a.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"tempestmiku open loop"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let chunks = store
            .recall_chunks("project:tempestmiku", "tempestmiku", 5)
            .await
            .unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(
            chunks[0]
                .text
                .contains("Project summary/open loop from session")
        );
        let session_b = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session_b.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"tempestmiku open loop"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let events = store.events_after(session_b.id, None).await.unwrap();
        let final_event = events
            .iter()
            .find(|event| event.event_type == "final")
            .unwrap();
        assert!(
            final_event
                .payload_json
                .to_string()
                .contains("Project summary/open loop from session")
        );
    }

    #[tokio::test]
    async fn serious_engineer_uses_coding_backend_and_replays_events() {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let state = AppState::new(
            store.clone(),
            memory,
            chat,
            PersonaConfig::default(),
            AuthConfig::NoAuth,
        )
        .with_coding_backend(Arc::new(ScriptedBackend::events()));
        let (app, store) = test_app_with_state(state);
        let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"ship p0a"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let events = store.events_after(session.id, None).await.unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["mode", "text", "diff", "artifact", "final"]
        );
        let replay = store.events_after(session.id, Some(1)).await.unwrap();
        assert_eq!(
            replay
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["text", "diff", "artifact", "final"]
        );
        let final_event = events
            .iter()
            .find(|event| event.event_type == "final")
            .unwrap();
        let final_text = final_event.payload_json.to_string();
        assert!(final_text.contains("artifact://"));
        assert!(!final_text.contains("喵"));
    }

    #[tokio::test]
    async fn approval_route_resolves_pending_backend_permission() {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let mut state = AppState::new(
            store.clone(),
            memory,
            chat,
            PersonaConfig::default(),
            AuthConfig::NoAuth,
        );
        let broker = Arc::clone(&state.approval_broker);
        state = state.with_coding_backend(Arc::new(ScriptedBackend::approval(Arc::clone(&broker))));
        let (app, store) = test_app_with_state(state);
        let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

        let post_app = app.clone();
        let message = tokio::spawn(async move {
            post_app
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/sessions/{}/messages", session.id))
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"content":"needs approval"}"#))
                        .unwrap(),
                )
                .await
                .unwrap()
        });

        let approval_id =
            wait_for_event_payload(&store, session.id, "approval").await["approvalId"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap();
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/sessions/{}/approvals/{}",
                        session.id, approval_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"decision":"approve"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(message.await.unwrap().status(), StatusCode::OK);

        let resolved = store
            .events_after(session.id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|event| event.event_type == "approval_resolved")
            .unwrap();
        assert_eq!(resolved.payload_json["outcome"], json!("selected"));
        assert_eq!(resolved.payload_json["optionId"], json!("allow"));
    }

    #[tokio::test]
    async fn artifact_resource_route_reads_session_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let state = AppState::new(
            store,
            memory,
            chat,
            PersonaConfig::default(),
            AuthConfig::NoAuth,
        )
        .with_artifact_root(root.clone())
        .with_coding_backend(Arc::new(ScriptedBackend::artifact(root)));
        let (app, _) = test_app_with_state(state);
        let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"write transcript"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sessions/{}/resources/artifacts", session.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_json = response_json(list).await;
        assert_eq!(list_json[0]["uri"], json!("artifact://0"));

        let read = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sessions/{}/resources/artifacts/0", session.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);
        let read_json = response_json(read).await;
        assert_eq!(read_json["uri"], json!("artifact://0"));
        assert!(
            read_json["content"]
                .as_str()
                .unwrap()
                .contains("scripted transcript")
        );
    }

    async fn wait_for_event_payload(
        store: &InMemoryStore,
        session_id: Uuid,
        event_type: &str,
    ) -> Value {
        for _ in 0..100 {
            if let Some(event) = store
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
        panic!("event {event_type} was not persisted")
    }

    #[tokio::test]
    async fn bearer_and_forwarded_auth_are_enforced() {
        let (app, _) = test_app(
            PersonaConfig::default(),
            AuthConfig::BearerToken("secret".to_string()),
        );
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let allowed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sessions")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);

        let (forwarded, _) = test_app(
            PersonaConfig::default(),
            AuthConfig::Forwarded(ForwardedAuthConfig {
                user_header: "x-forwarded-user".to_string(),
                expected_user: Some("brian".to_string()),
            }),
        );
        let wrong = forwarded
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sessions")
                    .header("x-forwarded-user", "not-brian")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
        let ok = forwarded
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sessions")
                    .header("x-forwarded-user", "brian")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
    }
}

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
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
    AuthConfig, ChatRunner, MemoryProvider, Mode, NewSession, PersistingEventSink, PersonaConfig,
    Result, SessionEvent, Store, StoreEvent, webui,
};

pub struct AppState<S, M, C> {
    pub store: Arc<S>,
    pub memory: Arc<M>,
    pub chat: Arc<C>,
    pub persona: PersonaConfig,
    pub auth: AuthConfig,
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
            live_events: Arc::new(parking_lot::Mutex::new(std::collections::BTreeMap::new())),
        }
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
        .merge(webui::routes::<AppState<S, M, C>>())
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub id: Uuid,
    pub mode: Mode,
    pub persona_status: crate::PersonaStatus,
}

async fn create_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    headers: HeaderMap,
) -> Result<Json<CreateSessionResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let persona_status = state.persona.load_status();
    let session = state
        .store
        .create_session(NewSession {
            mode: Mode::PersonalAssistant,
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
                label: "Personal Assistant".to_string(),
                persona_status: persona_status.clone(),
            })?,
        )
        .await?;
    let _ = state.sender(session.id).send(mode_event);
    Ok(Json(CreateSessionResponse {
        id: session.id,
        mode: session.mode,
        persona_status,
    }))
}

#[derive(Debug, Deserialize)]
pub struct PostMessageRequest {
    pub content: String,
    #[serde(default = "default_subject")]
    pub subject: String,
    #[serde(default = "default_scope")]
    pub scope: String,
}

fn default_subject() -> String {
    "brian".to_string()
}

fn default_scope() -> String {
    "global".to_string()
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
    state
        .store
        .append_message(session_id, "user", &payload.content)
        .await?;
    let memory = state
        .memory
        .context_for_turn(&payload.subject, &payload.scope, &payload.content)
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
    let sink = Arc::new(PersistingEventSink::new(
        session_id,
        Arc::clone(&state.store),
        state.sender(session_id),
    ));
    let response = state.chat.run_turn(user_prompt, sink.clone()).await?;
    sink.flush().await?;
    state
        .store
        .append_message(session_id, "assistant", &response)
        .await?;
    Ok(Json(json!({ "status": "accepted" })))
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
    use tower::ServiceExt;

    use crate::{
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

    async fn create(app: &Router) -> CreateSessionResponse {
        let res = app
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

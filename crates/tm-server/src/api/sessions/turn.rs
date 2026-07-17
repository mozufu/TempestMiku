use super::*;
use tokio::sync::Notify;

mod body;
mod dispatcher;
mod drive_recall;
mod validation;

use validation::{validate_client_message_id, validate_message_content};

#[cfg(test)]
mod dispatcher_tests;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PostMessageRequest {
    pub client_message_id: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostMessageResponse {
    pub turn_id: Uuid,
    pub client_message_id: String,
    pub status: String,
}

pub(crate) async fn post_message<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Json(payload): Json<PostMessageRequest>,
) -> Result<impl IntoResponse>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    validate_client_message_id(&payload.client_message_id)?;
    validate_message_content(&payload.content)?;
    let turn = state
        .store
        .enqueue_turn(session_id, &payload.client_message_id, &payload.content)
        .await?;
    state.notify_turn_dispatcher();
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(PostMessageResponse {
            turn_id: turn.id,
            client_message_id: turn.client_message_id,
            status: turn.status,
        }),
    ))
}

pub(crate) async fn get_turn<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, turn_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<crate::SessionTurnRecord>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let turn = state.store.turn(turn_id).await?;
    if turn.session_id != session_id {
        return Err(ServerError::NotFound(format!("turn {turn_id}")));
    }
    Ok(Json(turn))
}

pub(crate) fn start_turn_dispatcher<S, M, C>(state: AppState<S, M, C>, notify: Arc<Notify>)
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let (shutdown_guard, shutdown) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        // The app-level dispatcher exists only for deterministic in-process tests. Keeping
        // the sender alive prevents the watch channel from looking like a shutdown request.
        let _shutdown_guard = shutdown_guard;
        dispatcher::run_turn_dispatcher(
            state,
            notify,
            shutdown,
            4,
            Duration::from_millis(250),
            dispatcher::TurnMaintenance::default(),
        )
        .await;
    });
}

pub(crate) fn start_supervised_turn_dispatcher<S, M, C>(
    state: AppState<S, M, C>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    concurrency: usize,
    poll_interval: Duration,
) -> tokio::task::JoinHandle<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let notify = Arc::clone(&state.turn_notify);
    tokio::spawn(dispatcher::run_turn_dispatcher(
        state,
        notify,
        shutdown,
        concurrency,
        poll_interval,
        dispatcher::TurnMaintenance::default(),
    ))
}

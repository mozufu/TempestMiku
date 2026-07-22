use super::*;

const MAX_FEEDBACK_COMMENT_BYTES: usize = 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnFeedbackRequest {
    pub outcome: tm_memory::FeedbackOutcome,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnFeedbackResponse {
    pub turn_id: Uuid,
    pub outcome: tm_memory::FeedbackOutcome,
    pub recorded: bool,
}

pub(crate) async fn turn_feedback<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, turn_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<TurnFeedbackRequest>,
) -> Result<Json<TurnFeedbackResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.store.get_session(session_id).await?;
    let turn = state.store.turn(turn_id).await?;
    if turn.session_id != session_id {
        return Err(ServerError::NotFound(format!("turn {turn_id}")));
    }
    if !matches!(turn.status.as_str(), "completed" | "failed") {
        return Err(ServerError::Conflict("turn is not finished".to_string()));
    }
    if payload
        .comment
        .as_ref()
        .is_some_and(|comment| comment.len() > MAX_FEEDBACK_COMMENT_BYTES)
    {
        return Err(ServerError::InvalidRequest(format!(
            "feedback comment exceeds {MAX_FEEDBACK_COMMENT_BYTES} bytes"
        )));
    }

    let recorded = state
        .store
        .record_turn_feedback(
            session_id,
            turn_id,
            payload.outcome,
            payload.comment.as_deref(),
        )
        .await?;
    state
        .store
        .append_event_for_turn_once(
            session_id,
            "turn_feedback",
            json!({
                "turnId": turn_id,
                "outcome": payload.outcome.as_str(),
            }),
            turn_id,
        )
        .await?;

    Ok(Json(TurnFeedbackResponse {
        turn_id,
        outcome: payload.outcome,
        recorded,
    }))
}

use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EventsQuery {
    last_event_id: Option<i64>,
    last_event_id_legacy: Option<i64>,
}

pub(super) async fn session_events<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
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
    let last_event_id = last_event_id
        .or(query.last_event_id)
        .or(query.last_event_id_legacy);
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

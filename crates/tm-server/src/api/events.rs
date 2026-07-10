use super::*;
use std::net::SocketAddr;

use axum::extract::ConnectInfo;

const AUTH_REVALIDATION_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionEventEnvelope<'a> {
    #[serde(rename = "type")]
    event_type: &'a str,
    // Legacy persisted events keep this null; durable turns serialize their UUID here.
    turn_id: Option<&'a Uuid>,
    payload: &'a Value,
    created_at: &'a chrono::DateTime<Utc>,
}

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
    connect: Option<ConnectInfo<SocketAddr>>,
    Query(query): Query<EventsQuery>,
) -> Result<impl IntoResponse>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok());
    let last_event_id = last_event_id
        .or(query.last_event_id)
        .or(query.last_event_id_legacy);
    let mut receiver = state.sender(session_id).subscribe();
    let (replay, replay_is_terminal) =
        replay_window(state.store.as_ref(), session_id, last_event_id).await?;
    let store = Arc::clone(&state.store);
    let auth = state.auth.clone();
    let peer = connect.map(|value| value.0);
    let stream = async_stream::stream! {
        let mut last_delivered = last_event_id.unwrap_or(0);
        let mut ended = replay_is_terminal && replay.is_empty();
        let mut authorized = true;
        let mut auth_tick = tokio::time::interval(AUTH_REVALIDATION_INTERVAL);
        auth_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        for event in replay {
            if event.seq <= last_delivered {
                continue;
            }
            if auth.authenticate(&headers, peer).await.is_err() {
                authorized = false;
                break;
            }
            last_delivered = event.seq;
            ended = event.event_type == "session_end";
            yield Ok::<Event, Infallible>(sse_event(event));
            if ended {
                break;
            }
        }
        if replay_is_terminal {
            ended = true;
        }
        while authorized && !ended {
            let received = tokio::select! {
                _ = auth_tick.tick() => {
                    None
                }
                received = receiver.recv() => Some(received),
            };
            let Some(received) = received else {
                if auth.authenticate(&headers, peer).await.is_err() {
                    break;
                }
                // Worker-only processes cannot publish into this API process's in-memory
                // broadcast channel. Poll the durable high-water mark alongside auth
                // revalidation so split api/worker deployments still stream every event.
                let Ok((missed, replay_is_terminal)) = replay_window(
                    store.as_ref(),
                    session_id,
                    Some(last_delivered),
                ).await else {
                    break;
                };
                for event in missed {
                    if event.seq <= last_delivered {
                        continue;
                    }
                    last_delivered = event.seq;
                    ended = event.event_type == "session_end";
                    yield Ok::<Event, Infallible>(sse_event(event));
                    if ended {
                        break;
                    }
                }
                if replay_is_terminal {
                    ended = true;
                }
                continue;
            };
            match received {
                Ok(event) => {
                    if event.seq <= last_delivered {
                        continue;
                    }
                    if auth.authenticate(&headers, peer).await.is_err() {
                        break;
                    }
                    // Treat the in-memory event as a wake-up only. Reading the durable
                    // sequence keeps concurrent broadcasters ordered and lets the terminal
                    // filter discard every row after `session_end`.
                    let Ok((missed, replay_is_terminal)) = replay_window(
                        store.as_ref(),
                        session_id,
                        Some(last_delivered),
                    ).await else {
                        break;
                    };
                    for event in missed {
                        if event.seq <= last_delivered {
                            continue;
                        }
                        last_delivered = event.seq;
                        ended = event.event_type == "session_end";
                        yield Ok::<Event, Infallible>(sse_event(event));
                        if ended {
                            break;
                        }
                    }
                    if replay_is_terminal {
                        ended = true;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let Ok((missed, replay_is_terminal)) = replay_window(
                        store.as_ref(),
                        session_id,
                        Some(last_delivered),
                    ).await else {
                        break;
                    };
                    for event in missed {
                        if event.seq <= last_delivered {
                            continue;
                        }
                        if auth.authenticate(&headers, peer).await.is_err() {
                            authorized = false;
                            break;
                        }
                        last_delivered = event.seq;
                        ended = event.event_type == "session_end";
                        yield Ok::<Event, Infallible>(sse_event(event));
                        if ended {
                            break;
                        }
                    }
                    if replay_is_terminal {
                        ended = true;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Ok(Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15))))
}

/// Loads one ordered replay window and makes `session_end` an authoritative terminal fence.
///
/// Session status is read first. Both production stores commit the ended status and terminal
/// event atomically, so an observed ended status guarantees that the following event read can
/// see `session_end`. If the caller's cursor is already at or beyond that event, the returned
/// window is empty and terminal. Any corrupt or legacy rows after the terminal event are dropped.
async fn replay_window<S: Store>(
    store: &S,
    session_id: Uuid,
    last_event_id: Option<i64>,
) -> Result<(Vec<SessionEvent>, bool)> {
    let session_ended = store.get_session(session_id).await?.status == "ended";
    let mut events = store.events_after(session_id, last_event_id).await?;
    if let Some(terminal_index) = events
        .iter()
        .position(|event| event.event_type == "session_end")
    {
        events.truncate(terminal_index + 1);
        return Ok((events, true));
    }
    if session_ended {
        events.clear();
        return Ok((events, true));
    }
    Ok((events, false))
}

fn sse_event(event: SessionEvent) -> Event {
    let envelope = SessionEventEnvelope {
        event_type: &event.event_type,
        turn_id: event.turn_id.as_ref(),
        payload: &event.payload_json,
        created_at: &event.created_at,
    };
    Event::default()
        .id(event.seq.to_string())
        .event("session_event")
        .json_data(envelope)
        .expect("serializing stored JSON payload cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn emits_only_the_versioned_session_event_envelope() {
        let session_id = Uuid::new_v4();
        let turn_id = Uuid::new_v4();
        let mut stored = SessionEvent::new(
            session_id,
            42,
            "text",
            json!({ "delta": "miku" }),
            Utc::now(),
        );
        stored.turn_id = Some(turn_id);
        let stream = futures::stream::iter([Ok::<_, Infallible>(sse_event(stored))]);
        let response = Sse::new(stream).into_response();
        let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();

        assert_eq!(body.matches("event: session_event").count(), 1, "{body}");
        assert!(body.contains("id: 42"), "{body}");
        assert!(body.contains(r#""type":"text""#), "{body}");
        assert!(body.contains(&format!(r#""turnId":"{turn_id}""#)), "{body}");
        assert!(body.contains(r#""payload":{"delta":"miku"}"#), "{body}");
        assert!(body.contains(r#""createdAt":"#), "{body}");
        assert!(!body.contains("event: text"), "{body}");
    }
}

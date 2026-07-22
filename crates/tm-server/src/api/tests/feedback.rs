use super::*;
use crate::SessionTurnRecord;
use tm_memory::FeedbackOutcome;

async fn finish_turn(
    store: &InMemoryStore,
    session_id: Uuid,
    client_message_id: &str,
    status: &str,
) -> SessionTurnRecord {
    let turn = store
        .enqueue_turn(session_id, client_message_id, "feedback fixture")
        .await
        .unwrap();
    let worker_id = Uuid::new_v4();
    let claimed = store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .expect("queued feedback fixture turn");
    assert_eq!(claimed.id, turn.id);
    match status {
        "completed" => store
            .complete_turn(turn.id, worker_id, "done", Utc::now())
            .await
            .unwrap(),
        "failed" => store
            .fail_turn(turn.id, worker_id, "fixture failure", Utc::now())
            .await
            .unwrap(),
        other => panic!("unsupported terminal status {other}"),
    }
}

async fn post_feedback(
    app: &Router,
    session_id: Uuid,
    turn_id: Uuid,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/turns/{turn_id}/feedback"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn turn_feedback_records_once_and_appends_one_replay_event() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let turn = finish_turn(
        store.as_ref(),
        session.id,
        "feedback-completed",
        "completed",
    )
    .await;

    let response = post_feedback(
        &app,
        session.id,
        turn.id,
        json!({"outcome": "corrected", "comment": "Use the narrower fix."}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["turnId"], json!(turn.id));
    assert_eq!(response["outcome"], json!("corrected"));
    assert_eq!(response["recorded"], json!(true));

    let feedback = store.turn_feedback(turn.id).await.unwrap().unwrap();
    assert_eq!(feedback.0, FeedbackOutcome::Corrected);
    assert_eq!(feedback.1.as_deref(), Some("Use the narrower fix."));
    let events = store
        .events_by_type(session.id, "turn_feedback", 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].turn_id, Some(turn.id));
    assert_eq!(
        events[0].payload_json,
        json!({"turnId": turn.id, "outcome": "corrected"})
    );

    let duplicate = post_feedback(
        &app,
        session.id,
        turn.id,
        json!({"outcome": "corrected", "comment": "Use the narrower fix."}),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::OK);
    assert_eq!(response_json(duplicate).await["recorded"], json!(false));
    assert_eq!(
        store
            .events_by_type(session.id, "turn_feedback", 10)
            .await
            .unwrap()
            .len(),
        1
    );

    let conflict = post_feedback(&app, session.id, turn.id, json!({"outcome": "rejected"})).await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn turn_feedback_accepts_failed_turns_and_rejects_unfinished_or_foreign_turns() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let failed = finish_turn(store.as_ref(), session.id, "feedback-failed", "failed").await;
    let accepted = post_feedback(&app, session.id, failed.id, json!({"outcome": "rejected"})).await;
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(response_json(accepted).await["recorded"], json!(true));

    let unfinished = store
        .enqueue_turn(session.id, "feedback-unfinished", "still queued")
        .await
        .unwrap();
    let unfinished_response = post_feedback(
        &app,
        session.id,
        unfinished.id,
        json!({"outcome": "accepted"}),
    )
    .await;
    assert_eq!(unfinished_response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(unfinished_response).await["error"],
        json!("conflict: turn is not finished")
    );

    let other_session = create(&app).await;
    let foreign = post_feedback(
        &app,
        other_session.id,
        failed.id,
        json!({"outcome": "rejected"}),
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn conflicting_turn_feedback_does_not_create_a_missing_replay_event() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let turn = finish_turn(store.as_ref(), session.id, "feedback-conflict", "completed").await;
    assert!(
        store
            .record_turn_feedback(session.id, turn.id, FeedbackOutcome::Accepted, None)
            .await
            .unwrap()
    );

    let conflict = post_feedback(&app, session.id, turn.id, json!({"outcome": "rejected"})).await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert!(
        store
            .event_for_turn(session.id, turn.id, "turn_feedback")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn duplicate_turn_feedback_repairs_a_missing_replay_event() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let turn = finish_turn(store.as_ref(), session.id, "feedback-repair", "completed").await;
    assert!(
        store
            .record_turn_feedback(
                session.id,
                turn.id,
                FeedbackOutcome::Accepted,
                Some("persisted before event"),
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .event_for_turn(session.id, turn.id, "turn_feedback")
            .await
            .unwrap()
            .is_none()
    );

    let repaired = post_feedback(
        &app,
        session.id,
        turn.id,
        json!({"outcome": "accepted", "comment": "persisted before event"}),
    )
    .await;
    assert_eq!(repaired.status(), StatusCode::OK);
    assert_eq!(response_json(repaired).await["recorded"], json!(false));
    let event = store
        .event_for_turn(session.id, turn.id, "turn_feedback")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.payload_json,
        json!({"turnId": turn.id, "outcome": "accepted"})
    );
    let duplicate = post_feedback(
        &app,
        session.id,
        turn.id,
        json!({"outcome": "accepted", "comment": "persisted before event"}),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::OK);
    assert_eq!(response_json(duplicate).await["recorded"], json!(false));
    assert_eq!(
        store
            .events_by_type(session.id, "turn_feedback", 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn turn_feedback_rejects_comments_over_1024_bytes_without_recording() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let turn = finish_turn(
        store.as_ref(),
        session.id,
        "feedback-comment-limit",
        "completed",
    )
    .await;
    let oversized = "界".repeat(342);
    assert!(oversized.len() > 1024);

    let response = post_feedback(
        &app,
        session.id,
        turn.id,
        json!({"outcome": "accepted", "comment": oversized}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(store.turn_feedback(turn.id).await.unwrap().is_none());
    assert!(
        store
            .events_by_type(session.id, "turn_feedback", 10)
            .await
            .unwrap()
            .is_empty()
    );
}

use super::*;

#[tokio::test]
async fn session_creation_message_append_event_append_and_replay_work() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    assert_eq!(
        session.active_skills,
        vec!["miku-voice", "personal-assistant-state-capture"]
    );
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let reused = response_json(res).await;
    assert_eq!(reused["id"], session.id.to_string());
    assert_eq!(reused["mode"], json!("general"));

    post_user_message(&app, session.id, "hello").await;
    let all = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        all.iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["mode", "text", "final"]
    );
    assert_eq!(
        all[0].payload_json["voice_cap"],
        serde_json::json!("medium")
    );
    assert_eq!(
        all[0].payload_json["activeSkills"],
        json!(["miku-voice", "personal-assistant-state-capture"])
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
async fn ending_session_enqueues_one_dream_and_replays_lifecycle_events() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    post_user_message(&app, session.id, "wrap this session").await;

    let ended = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/end", session.id))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ended.status(), StatusCode::OK);
    let ended = response_json(ended).await;
    assert_eq!(ended["status"], json!("ended"));
    assert_eq!(ended["dream"]["status"], json!("queued"));
    assert_eq!(ended["dream"]["reason"], json!("session_ended"));
    assert_eq!(ended["dream"]["scope"], json!("global"));

    let session_record = store.get_session(session.id).await.unwrap();
    assert_eq!(session_record.status, "ended");
    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams.len(), 1);
    assert_eq!(ended["dream"]["id"], json!(dreams[0].id));
    assert_eq!(dreams[0].status, tm_memory::DreamStatus::Queued);
    assert_eq!(dreams[0].source_event_seq, Some(5));

    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["mode", "text", "final", "dream_queued", "session_end"]
    );
    assert_eq!(
        events[3].payload_json["sourceEventSeq"],
        json!(events[4].seq)
    );

    let duplicate = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/end", session.id))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::OK);
    let duplicate = response_json(duplicate).await;
    assert_eq!(duplicate["dream"]["id"], ended["dream"]["id"]);
    assert_eq!(
        store
            .dream_queue_for_session(session.id)
            .await
            .unwrap()
            .len(),
        1
    );
    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "session_end")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "dream_queued")
            .count(),
        1
    );
    store
        .append_event(
            session.id,
            "post_end_diagnostic",
            json!({ "shouldNotStream": true }),
        )
        .await
        .unwrap();

    let replay = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/events", session.id))
                .header("last-event-id", "2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay = axum::body::to_bytes(replay.into_body(), 64 * 1024)
        .await
        .unwrap();
    let replay = String::from_utf8(replay.to_vec()).unwrap();
    assert_eq!(replay.matches("event: session_event").count(), 3);
    assert!(replay.contains(r#""type":"final""#));
    assert!(replay.contains(r#""type":"session_end""#));
    assert!(replay.contains(r#""type":"dream_queued""#));
    assert!(!replay.contains("post_end_diagnostic"));
    assert!(replay.contains(r#""turnId":"#));
    assert!(replay.contains(r#""createdAt":"#));
    assert_eq!(replay.matches("id: 3").count(), 1);

    let session_end_seq = events
        .iter()
        .find(|event| event.event_type == "session_end")
        .expect("session_end event")
        .seq;
    let post_end_seq = session_end_seq + 1;
    for terminal_cursor in [session_end_seq, post_end_seq] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sessions/{}/events", session.id))
                    .header("last-event-id", terminal_cursor.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = tokio::time::timeout(
            Duration::from_secs(1),
            axum::body::to_bytes(response.into_body(), 1024),
        )
        .await
        .expect("terminal replay must close immediately")
        .unwrap();
        assert!(
            body.is_empty(),
            "terminal cursor {terminal_cursor} streamed unexpected data: {}",
            String::from_utf8_lossy(&body)
        );
    }
}

#[tokio::test]
async fn sessions_history_lists_recent_sessions_and_hydrates_transcript() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let first = create(&app).await;
    post_user_message(&app, first.id, "first session asks for status").await;
    tokio::time::sleep(Duration::from_millis(2)).await;
    let second = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    post_user_message(&app, second.id, "second session asks for code").await;
    store
        .upsert_project_item(NewProjectItem {
            project_id: "tempestmiku".to_string(),
            kind: ProjectItemKind::Summary,
            text: "Condensed code-session summary".to_string(),
            target_uri: format!("project://tempestmiku/summary/{}", second.id),
            source_session_id: second.id,
            source_event_seq: None,
            source_uri: None,
            dedupe_key: format!("test-summary:{}", second.id),
            provenance_json: json!({"source": "test"}),
        })
        .await
        .unwrap();

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/sessions?limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["id"], second.id.to_string());
    assert_eq!(sessions[0]["title"], "Condensed code-session summary");
    assert_eq!(sessions[0]["messageCount"], 2);
    assert!(sessions[0]["lastEventId"].as_i64().unwrap() >= 3);
    assert_eq!(sessions[1]["id"], first.id.to_string());
    assert_eq!(
        sessions[1]["title"],
        "Miku heard: first session asks for status"
    );

    store
        .append_event(
            first.id,
            "write_proposal",
            json!({
                "kind": "memory",
                "proposalId": "proposal-history",
                "status": "pending",
                "text": "Remember history restore."
            }),
        )
        .await
        .unwrap();
    store
        .append_event(
            first.id,
            "approval",
            json!({
                "approvalId": "approval-history",
                "backend": "memory",
                "action": "memory.write profile_fact",
                "scope": {}
            }),
        )
        .await
        .unwrap();

    let transcript = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/messages", first.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(transcript.status(), StatusCode::OK);
    let transcript = response_json(transcript).await;
    assert_eq!(transcript["id"], first.id.to_string());
    assert_eq!(transcript["messages"][0]["role"], "user");
    assert_eq!(
        transcript["messages"][0]["content"],
        "first session asks for status"
    );
    assert_eq!(transcript["messages"][1]["role"], "assistant");
    assert!(
        transcript["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("Miku heard: first session asks for status")
    );
    assert!(transcript["lastEventId"].as_i64().unwrap() >= 5);
    let pending = transcript["pendingEvents"].as_array().unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["write_proposal", "approval"]
    );

    store
        .append_event(
            first.id,
            "approval_resolved",
            json!({
                "approvalId": "approval-history",
                "status": "approved"
            }),
        )
        .await
        .unwrap();
    store
        .append_event(
            first.id,
            "write_proposal",
            json!({
                "kind": "memory",
                "proposalId": "proposal-history",
                "status": "approved"
            }),
        )
        .await
        .unwrap();
    let resolved = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/messages", first.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let resolved = response_json(resolved).await;
    assert!(resolved["pendingEvents"].as_array().unwrap().is_empty());
}

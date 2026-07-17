use super::*;

#[tokio::test]
async fn memory_resource_gateway_reads_dream_queue_and_records() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:resource:{}", session.id),
            source_event_seq: Some(3),
            available_at: Utc::now(),
        })
        .await
        .unwrap();
    let other_scope = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "project:other".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:resource:other:{}", session.id),
            source_event_seq: None,
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let queue = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=memory://dreams",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(queue.status(), StatusCode::OK);
    let queue = response_json(queue).await;
    assert_eq!(queue["uri"], json!("memory://dreams"));
    let queue_content = queue["content"].as_str().unwrap();
    assert!(queue_content.contains(&format!("memory://dreams/{}", dream.id)));
    assert!(queue_content.contains("status=queued"));
    assert!(!queue_content.contains(&other_scope.id.to_string()));

    let record_uri = format!("memory://dreams/{}", dream.id);
    let record = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri={}",
                    session.id, record_uri
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(record.status(), StatusCode::OK);
    let record = response_json(record).await;
    assert_eq!(record["uri"], json!(record_uri));
    let record_content = record["content"].as_str().unwrap();
    assert!(record_content.contains("Source event seq: 3"));
    assert!(record_content.contains(&format!("Status: {}", DreamStatus::Queued)));
    assert!(record_content.contains(&format!("Session: {}", session.id)));

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=memory://dreams",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    let entries = listed.as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry["uri"] == json!(format!("memory://dreams/{}", dream.id)))
    );
    assert!(
        entries
            .iter()
            .all(|entry| entry["uri"] != json!(format!("memory://dreams/{}", other_scope.id)))
    );

    let other_record = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=memory://dreams/{}",
                    session.id, other_scope.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(other_record.status(), StatusCode::NOT_FOUND);
}

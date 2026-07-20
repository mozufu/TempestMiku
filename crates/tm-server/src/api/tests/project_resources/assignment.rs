use super::*;

#[tokio::test]
async fn project_views_grow_from_turn_observations() {
    let (app, _store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create_project_session(&app).await;
    post_user_message(
        &app,
        session.id,
        "capture an open loop and decision for the code TODO",
    )
    .await;

    let overview = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/project", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(overview.status(), StatusCode::OK);
    let overview_json = response_json(overview).await;
    assert_eq!(overview_json["projectUri"], json!("project://tempestmiku"));
    assert!(!overview_json["nextActions"].as_array().unwrap().is_empty());
    assert!(!overview_json["openLoops"].as_array().unwrap().is_empty());
    assert!(!overview_json["decisions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn create_project_is_idempotent_and_lists_in_catalog() {
    let (app, _store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"id":"Roadmap Work","title":"Roadmap Work"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let created = response_json(response).await;
    assert_eq!(created["id"], json!("roadmap-work"));
    assert_eq!(created["status"], json!("active"));
    assert_eq!(created["memoryScope"], json!("project:roadmap-work"));

    // Idempotent on id.
    let again = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/projects")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"id":"roadmap-work"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(response_json(again).await["id"], json!("roadmap-work"));
}

#[tokio::test]
async fn archived_project_scope_fails_closed_for_reads_and_new_sessions() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create_project_session(&app).await;
    post_user_message(&app, session.id, "capture a decision").await;

    // Archive the project entity; §30.4 tombstones the memory scope in the same transition.
    let archived = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/projects/tempestmiku/archive")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"reason":"shipped"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::OK);
    assert_eq!(response_json(archived).await["status"], json!("archived"));

    // The archived scope no longer appears in the catalog.
    let catalog = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response_json(catalog).await, json!({"projects": []}));

    // A new session cannot enter the archived scope.
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mode":"serious_engineer","scope":"project:tempestmiku"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);

    // The durable tombstone survives (memory scope revoked).
    assert!(
        store
            .memory_scope_tombstone("brian", "project:tempestmiku")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn assigning_a_closed_session_replays_observations() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    // A global session with a decision-bearing exchange, then closed.
    let session = create(&app).await;
    post_user_message(
        &app,
        session.id,
        "we decided to keep SSE; open loop: wire mobile resume",
    )
    .await;
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/end", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Create the target project, then assign the closed session to it.
    store
        .ensure_project("archive-target", "Archive Target")
        .await
        .unwrap();
    let assigned = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/projects/archive-target/sessions/{}", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(assigned.status(), StatusCode::OK);
    let body = response_json(assigned).await;
    assert_eq!(body["projectId"], json!("archive-target"));
    assert_eq!(body["projectUri"], json!("project://archive-target"));

    // Observation catch-up grew project items for the assigned project.
    let items = store.project_items("archive-target", None).await.unwrap();
    assert!(
        !items.is_empty(),
        "assignment must replay observations into project items"
    );
    assert_eq!(body["assigned"], json!(items.len()));
}

#[tokio::test]
async fn assigning_an_active_session_is_rejected() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    store.ensure_project("live", "Live").await.unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/projects/live/sessions/{}", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

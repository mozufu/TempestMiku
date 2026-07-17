use super::*;

#[tokio::test]
async fn memory_resource_gateway_denies_unknown_and_ungranted_reads() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let unknown = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=memory://secret",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::FORBIDDEN);

    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(crate::memory::MemoryResourceHandler::new(
        Arc::clone(&store),
        "brian",
        "global",
    )));
    let ctx = InvocationCtx::new(CapabilityGrants::default());
    let err = registry
        .read("memory://root", None, &ctx)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        HostError::CapabilityDenied(capability) if capability == "resources.read:memory"
    ));
}

#[tokio::test]
async fn memory_resource_gateway_denies_reads_and_lists_after_scope_revocation() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create_project_session(&app).await;

    let before = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=memory://root",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(before.status(), StatusCode::OK);

    store
        .revoke_memory_scope("brian", "project:tempestmiku", "project unlinked")
        .await
        .unwrap();

    for endpoint in ["resolve", "list"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/sessions/{}/resources/{endpoint}?uri=memory://root",
                        session.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{endpoint}");
    }
}

#[tokio::test]
async fn session_resource_preview_returns_compact_bounded_memory_preview() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "global".to_string(),
            text: "long memory ".repeat(200),
            source: "test".to_string(),
            importance: 0.65,
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/preview?uri=memory://root",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response).await;
    assert_eq!(json["content"], json!(""));
    assert_eq!(json["uri"], json!("memory://root"));
    let preview = json["preview"].as_str().unwrap();
    assert!(preview.contains("long memory"));
    assert!(preview.len() <= SESSION_RESOURCE_PREVIEW_BYTES + 64);
    assert_eq!(json["has_more"], json!(true));
}

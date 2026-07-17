use super::*;

#[tokio::test]
async fn readiness_fails_closed_for_worker_without_postgres() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    let state = AppState::new(
        store,
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_runtime_status(Arc::new(crate::RuntimeStatus::new(
        crate::ServerRole::Worker,
        false,
        false,
    )))
    .with_auto_turn_dispatcher(false);
    let response = app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response_json(response).await;
    assert_eq!(body["status"], json!("not_ready"));
    assert_eq!(body["selfEvolution"]["tier"], json!("conservative"));
}

#[tokio::test]
async fn readiness_fails_closed_for_corrupt_durable_memory_schema() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    let state = AppState::new(
        store,
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_runtime_status(Arc::new(
        crate::RuntimeStatus::new(crate::ServerRole::All, true, true).with_memory_readiness(
            tm_memory::DurableMemoryReadiness {
                schema: tm_memory::MemorySchemaReadiness::Corrupt {
                    reason: "missing memory_records".to_string(),
                },
                pgvector: tm_memory::PgVectorReadiness::Disabled,
                embeddings: tm_memory::EmbeddingReadiness::Disabled,
            },
        ),
    ))
    .with_auto_turn_dispatcher(false);
    let response = app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response_json(response).await;
    assert_eq!(body["status"], json!("not_ready"));
    assert_eq!(
        body["runtime"]["memoryReadiness"]["schema"]["corrupt"]["reason"],
        json!("missing memory_records")
    );
}

#[tokio::test]
async fn readiness_reports_only_the_effective_self_evolution_tier() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    let state = AppState::new(
        store,
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_self_evolution_tier(tm_host::SelfEvolutionTier::Off)
    .with_auto_turn_dispatcher(false);
    let response = app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["selfEvolution"], json!({ "tier": "off" }));
}

use super::*;

#[tokio::test]
async fn device_push_registration_is_authenticated_encrypted_and_revoked() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    let auth_store = Arc::new(InMemoryAuthDeviceStore::default());
    let push_store = Arc::new(InMemoryPushStore::default());
    let push = Arc::new(PushService::new(
        push_store.clone(),
        Arc::new(FakePushProvider::default()),
        PushCipher::generate_for_tests(),
    ));
    let state = AppState::new(
        store,
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::Device(DeviceAuthConfig {
            cookie_name: "tm_device".to_string(),
            secure_cookie: false,
            owner_subject: "brian".to_string(),
            bootstrap_token_hash: None,
            allow_loopback_pairing: true,
            allowed_origin: Some("http://127.0.0.1:8787".to_string()),
        }),
    )
    .with_auth_store(auth_store)
    .with_push_service(Arc::clone(&push));
    let app = app(state);
    let loopback = axum::extract::ConnectInfo(SocketAddr::from((Ipv4Addr::LOCALHOST, 4242)));

    let issued = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/pairing-codes")
                .header("host", "127.0.0.1:8787")
                .extension(loopback)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response_json(issued).await["code"]
        .as_str()
        .unwrap()
        .to_string();
    let paired = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/pair")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "code": code,
                        "deviceName": "Push test",
                        "platform": "android",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let paired = response_json(paired).await;
    let token = paired["token"].as_str().unwrap();
    let secret = "future-provider-registration-secret";

    let registered = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/auth/push-registration")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "provider": "fake",
                        "registration": secret,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(registered.status(), StatusCode::OK);
    let registered = response_json(registered).await;
    assert_eq!(registered["registration"]["provider"], json!("fake"));
    assert!(!registered.to_string().contains(secret));

    let metrics = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/metrics")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response_json(metrics).await["queues"]["push"]["depth"],
        json!(0)
    );

    let unregistered = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/auth/push-registration")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unregistered.status(), StatusCode::OK);
    assert_eq!(
        push.runtime_metrics().await.unwrap().disabled_registrations,
        1
    );
}

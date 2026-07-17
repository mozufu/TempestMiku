use super::*;

#[tokio::test]
async fn bearer_and_forwarded_auth_are_enforced() {
    let (app, _) = test_app(
        ModesConfig::default(),
        AuthConfig::BearerToken("secret".to_string()),
    );
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let allowed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("authorization", "Bearer secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    let session = response_json(allowed).await;
    let session_id = session["id"].as_str().unwrap();

    for uri in [
        "/sessions".to_string(),
        "/ready".to_string(),
        "/metrics".to_string(),
        format!("/sessions/{session_id}/messages"),
    ] {
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    }

    for uri in ["/ready", "/metrics"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(response_json(health).await, json!({"status": "ok"}));

    let (forwarded, _) = test_app(
        ModesConfig::default(),
        AuthConfig::Forwarded(ForwardedAuthConfig {
            user_header: "x-forwarded-user".to_string(),
            expected_user: Some("brian".to_string()),
            trusted_proxy_cidrs: vec!["127.0.0.0/8".parse().unwrap()],
        }),
    );
    let wrong = forwarded
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("x-forwarded-user", "not-brian")
                .extension(axum::extract::ConnectInfo(SocketAddr::from((
                    Ipv4Addr::LOCALHOST,
                    4242,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
    let untrusted = forwarded
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("x-forwarded-user", "brian")
                .extension(axum::extract::ConnectInfo(SocketAddr::from((
                    Ipv4Addr::new(10, 0, 0, 7),
                    4242,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(untrusted.status(), StatusCode::FORBIDDEN);
    let ok = forwarded
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("x-forwarded-user", "brian")
                .extension(axum::extract::ConnectInfo(SocketAddr::from((
                    Ipv4Addr::new(127, 42, 0, 9),
                    4242,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
}

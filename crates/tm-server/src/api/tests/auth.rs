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

    let (forwarded, _) = test_app(
        ModesConfig::default(),
        AuthConfig::Forwarded(ForwardedAuthConfig {
            user_header: "x-forwarded-user".to_string(),
            expected_user: Some("brian".to_string()),
        }),
    );
    let wrong = forwarded
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("x-forwarded-user", "not-brian")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
    let ok = forwarded
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("x-forwarded-user", "brian")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
}

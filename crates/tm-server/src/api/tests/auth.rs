use super::*;
use futures::StreamExt as _;
use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use crate::{
    DeviceAuthConfig, FakePushProvider, InMemoryAuthDeviceStore, InMemoryPushStore, PushCipher,
    PushService,
};

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
    assert_eq!(response_json(response).await["status"], json!("not_ready"));
}

#[tokio::test]
async fn one_time_pairing_issues_device_bearer_and_cookie_then_revokes_them() {
    let (app, _) = test_app(
        ModesConfig::default(),
        AuthConfig::Device(DeviceAuthConfig {
            cookie_name: "tm_device".to_string(),
            secure_cookie: false,
            owner_subject: "brian".to_string(),
            bootstrap_token_hash: None,
            allow_loopback_pairing: true,
            allowed_origin: Some("http://127.0.0.1:8787".to_string()),
        }),
    );
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
    assert_eq!(issued.status(), StatusCode::OK);
    let issued = response_json(issued).await;
    let code = issued["code"].as_str().unwrap();
    assert!(
        issued["pairingLink"]
            .as_str()
            .unwrap()
            .starts_with("tempestmiku://pair?v=1&server=")
    );
    assert!(issued["pairingLink"].as_str().unwrap().contains("&code="));

    let body = serde_json::to_vec(&json!({
        "code": code,
        "deviceName": "Pixel test",
        "platform": "android",
    }))
    .unwrap();
    let paired = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/pair")
                .header("content-type", "application/json")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(paired.status(), StatusCode::OK);
    let cookie = paired
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let paired = response_json(paired).await;
    let token = paired["token"].as_str().unwrap();
    let device_id = paired["device"]["id"].as_str().unwrap();
    assert_eq!(paired["device"]["ownerSubject"], json!("brian"));
    assert!(token.starts_with("tmk_dev_"));

    let replay = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/pair")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::FORBIDDEN);

    let bearer_session = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bearer_session.status(), StatusCode::OK);
    let bearer_session = response_json(bearer_session).await;
    let session_id = bearer_session["id"].as_str().unwrap().to_string();
    let cookie_modes = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/modes")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cookie_modes.status(), StatusCode::OK);

    let csrf_denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(csrf_denied.status(), StatusCode::FORBIDDEN);
    let cookie_mutation = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("host", "127.0.0.1:8787")
                .header("origin", "http://127.0.0.1:8787")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cookie_mutation.status(), StatusCode::OK);

    let web_code = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/pairing-codes")
                .header("host", "127.0.0.1:8787")
                .extension(axum::extract::ConnectInfo(SocketAddr::from((
                    Ipv4Addr::LOCALHOST,
                    4242,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let web_code = response_json(web_code).await["code"]
        .as_str()
        .unwrap()
        .to_string();
    let web_pair = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/pair")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "code": web_code,
                        "deviceName": "Web test",
                        "platform": "web",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(web_pair.status(), StatusCode::OK);
    let web_cookie = web_pair
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let web_pair = response_json(web_pair).await;
    assert!(web_pair.get("token").is_none());
    assert_eq!(web_pair["device"]["ownerSubject"], json!("brian"));

    let open_stream = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{session_id}/events"))
                .header("cookie", &web_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(open_stream.status(), StatusCode::OK);
    let mut open_stream = open_stream.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(1), open_stream.next())
        .await
        .expect("initial session event should arrive")
        .expect("stream should remain open before revocation")
        .unwrap();
    assert!(
        String::from_utf8_lossy(&first).contains("event: session_event"),
        "{first:?}"
    );

    let devices = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/auth/devices")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(devices.status(), StatusCode::OK);
    let devices = response_json(devices).await;
    assert_eq!(devices["devices"].as_array().unwrap().len(), 2);
    assert!(devices.to_string().contains("Pixel test"));
    assert!(!devices.to_string().contains(token));

    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/logout")
                .header("host", "127.0.0.1:8787")
                .header("origin", "http://127.0.0.1:8787")
                .header("cookie", &web_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::OK);
    assert!(
        logout
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let closed = tokio::time::timeout(Duration::from_secs(4), open_stream.next())
        .await
        .expect("revoked idle stream should close within the auth interval");
    assert!(closed.is_none(), "revoked stream yielded another frame");
    let web_denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/modes")
                .header("cookie", &web_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(web_denied.status(), StatusCode::FORBIDDEN);

    let revoked = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/auth/devices/{device_id}"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::OK);
    let denied = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/modes")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

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

#[tokio::test]
async fn approval_detail_endpoint_returns_redacted_prompt_without_effect_payload() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let approval_id = Uuid::new_v4();
    let now = Utc::now();
    store
        .create_approval_request(crate::NewApprovalRequest {
            id: approval_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "native-deno".to_string(),
            action: "proc.run cargo test".to_string(),
            scope_json: json!({"capability": "proc.run"}),
            options_json: json!([
                {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
            ]),
            effect_type: "approval_continuation".to_string(),
            effect_payload_json: json!({"secretInternalValue": "must-not-leak"}),
            resumable: true,
            created_at: now,
            expires_at: now + chrono::Duration::minutes(5),
        })
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/approvals/{approval_id}", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["status"], json!("pending"));
    assert_eq!(response["action"], json!("proc.run cargo test"));
    assert_eq!(response["options"][0]["kind"], json!("allow_once"));
    assert!(!response.to_string().contains("secretInternalValue"));
    assert!(!response.to_string().contains("must-not-leak"));
}

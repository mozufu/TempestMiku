use super::*;
use crate::{DeviceAuthConfig, ForwardedAuthConfig, api::pairing::derive_public_base_url};
use std::net::{Ipv4Addr, SocketAddr};

fn device_auth() -> AuthConfig {
    AuthConfig::Device(DeviceAuthConfig {
        cookie_name: "tm_device".to_string(),
        secure_cookie: false,
        owner_subject: "brian".to_string(),
        bootstrap_token_hash: None,
        allow_loopback_pairing: true,
        allowed_origin: Some("http://miku.local:8787".to_string()),
    })
}

#[tokio::test]
async fn pairing_page_issues_an_inline_one_time_qr_from_loopback() {
    let (app, _) = test_app(ModesConfig::default(), device_auth());
    let page = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/pair")
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
    assert_eq!(page.status(), StatusCode::OK);
    assert!(
        page.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("text/html")
    );
    let body = response_text(page).await;
    assert!(body.contains("Secure pairing"));
    assert!(body.contains("<svg"));
    assert!(body.contains("Pair this web browser"));
    assert!(!body.contains("tempestmiku://pair"));
    assert!(!body.contains("Open Android app"));
    assert!(!body.contains("Copy Android link"));
    assert!(!body.contains("Manual fallback"));
}

#[test]
fn pairing_base_url_uses_env_or_direct_host_and_ignores_untrusted_forwarding() {
    let mut headers = HeaderMap::new();
    headers.insert("host", "internal:8787".parse().unwrap());
    headers.insert("x-forwarded-proto", "https".parse().unwrap());
    headers.insert("x-forwarded-host", "miku.example.test".parse().unwrap());

    assert_eq!(
        derive_public_base_url(&headers, Some("https://tailnet.example/"), false),
        "https://tailnet.example"
    );
    assert_eq!(
        derive_public_base_url(&headers, None, false),
        "http://internal:8787"
    );
    assert_eq!(
        derive_public_base_url(&headers, None, true),
        "https://miku.example.test"
    );
}

#[test]
fn pairing_forwarded_authority_requires_proxy_cidr_membership() {
    let config = ForwardedAuthConfig {
        user_header: "x-forwarded-user".to_string(),
        expected_user: Some("brian".to_string()),
        trusted_proxy_cidrs: vec!["10.24.0.0/16".parse().unwrap()],
    };
    let mut headers = HeaderMap::new();
    headers.insert("host", "internal:8787".parse().unwrap());
    headers.insert("x-forwarded-proto", "https".parse().unwrap());
    headers.insert("x-forwarded-host", "miku.example.test".parse().unwrap());

    let trusted_peer = "10.24.3.9".parse().unwrap();
    assert_eq!(
        derive_public_base_url(&headers, None, config.trusts(trusted_peer)),
        "https://miku.example.test"
    );

    let untrusted_peer = "10.25.0.1".parse().unwrap();
    assert_eq!(
        derive_public_base_url(&headers, None, config.trusts(untrusted_peer)),
        "http://internal:8787"
    );
}

#[tokio::test]
async fn first_pairing_code_is_forbidden_from_a_remote_peer_without_bootstrap() {
    let (app, _) = test_app(ModesConfig::default(), device_auth());
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/pairing-codes")
                .header("host", "miku.example.test")
                .extension(axum::extract::ConnectInfo(SocketAddr::from((
                    Ipv4Addr::new(192, 0, 2, 50),
                    4242,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn loopback_bootstrap_requires_a_loopback_host_not_a_public_proxy_host() {
    let (app, _) = test_app(ModesConfig::default(), device_auth());
    let public_host = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/pairing-codes")
                .header("host", "miku.example.test")
                .extension(axum::extract::ConnectInfo(SocketAddr::from((
                    Ipv4Addr::LOCALHOST,
                    4242,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(public_host.status(), StatusCode::FORBIDDEN);

    let local_host = app
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
    assert_eq!(local_host.status(), StatusCode::OK);
}

#[tokio::test]
async fn deployment_bootstrap_token_can_issue_only_the_first_remote_pairing_code() {
    let (app, _) = test_app(
        ModesConfig::default(),
        AuthConfig::Device(DeviceAuthConfig {
            cookie_name: "tm_device".to_string(),
            secure_cookie: true,
            owner_subject: "brian".to_string(),
            bootstrap_token_hash: Some(crate::auth::hash_secret("bootstrap-secret")),
            allow_loopback_pairing: false,
            allowed_origin: Some("https://miku.example.test".to_string()),
        }),
    );
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/pairing-codes")
                .header("host", "miku.example.test")
                .header(crate::auth::BOOTSTRAP_HEADER, "bootstrap-secret")
                .extension(axum::extract::ConnectInfo(SocketAddr::from((
                    Ipv4Addr::new(192, 0, 2, 50),
                    4242,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let issued = response_json(response).await;
    assert!(issued["code"].as_str().unwrap().len() >= 48);
}

async fn response_text(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

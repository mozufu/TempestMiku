use super::*;
use crate::api::pairing::derive_public_base_url;

#[tokio::test]
async fn pairing_page_and_qr_are_served_before_web_fallback() {
    let (app, _) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let page = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/pair")
                .header("host", "miku.local:8787")
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
    assert!(body.contains("TempestMiku Pairing"));
    assert!(body.contains("tempestmiku://pair?server=http%3A%2F%2Fmiku.local%3A8787"));
    assert!(body.contains("Open Web App"));

    let qr = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/pair/qr.svg")
                .header("host", "miku.local:8787")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(qr.status(), StatusCode::OK);
    assert!(
        qr.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("image/svg+xml")
    );
    assert!(response_text(qr).await.contains("<svg"));
}

#[test]
fn pairing_base_url_prefers_env_then_forwarded_headers_then_host() {
    let mut headers = HeaderMap::new();
    headers.insert("host", "internal:8787".parse().unwrap());
    headers.insert("x-forwarded-proto", "https".parse().unwrap());
    headers.insert("x-forwarded-host", "miku.example.test".parse().unwrap());

    assert_eq!(
        derive_public_base_url(&headers, Some("https://tailnet.example/")),
        "https://tailnet.example"
    );
    assert_eq!(
        derive_public_base_url(&headers, None),
        "https://miku.example.test"
    );

    let mut forwarded = HeaderMap::new();
    forwarded.insert(
        "forwarded",
        r#"for=192.0.2.1;proto=https;host="proxy.example:9443""#
            .parse()
            .unwrap(),
    );
    assert_eq!(
        derive_public_base_url(&forwarded, None),
        "https://proxy.example:9443"
    );
}

#[tokio::test]
async fn pairing_page_warns_for_loopback_targets() {
    let (app, _) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/pair")
                .header("host", "127.0.0.1:8787")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_text(response).await;
    assert!(body.contains("Loopback target"));
    assert!(body.contains("physical Android device"));
}

async fn response_text(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

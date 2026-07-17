use super::*;

#[tokio::test]
async fn skill_resource_gateway_fails_closed_when_managed_catalog_is_unconfigured() {
    let (app, _) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    for endpoint in ["resolve", "preview", "list"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/sessions/{}/resources/{endpoint}?uri=skill://miku-voice",
                        session.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{endpoint}");
        let json = response_json(response).await;
        let error = json["error"].as_str().unwrap();
        assert!(
            error.contains("unknown resource scheme skill"),
            "{endpoint}: {error}"
        );
        assert!(
            error.contains(
                "registered: artifact, linked, workspace, project, memory, agent, history"
            ),
            "{endpoint}: {error}"
        );
    }
}

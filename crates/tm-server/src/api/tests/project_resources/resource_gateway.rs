use super::*;

#[tokio::test]
async fn resource_gateway_reads_supported_schemes_and_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("linked");
    std::fs::create_dir_all(&linked_root).unwrap();
    std::fs::write(linked_root.join("README.md"), "linked readme").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: linked_root.clone(),
        mode: FsMode::Rw,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone())
    .with_linked_folders(linked);
    let (app, store) = test_app_with_state(state);
    let session = create_project_session(&app).await;
    let artifact_store =
        tm_artifacts::ArtifactStore::open(&artifact_root, session.id.to_string()).unwrap();
    artifact_store
        .put_text("artifact line", Some("artifact".to_string()), "text/plain")
        .unwrap();
    let workspace = artifact_root
        .join("sessions")
        .join(session.id.to_string())
        .join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("notes.md"), "one\ntwo").unwrap();
    store
        .upsert_project_item(NewProjectItem {
            project_id: "tempestmiku".to_string(),
            kind: ProjectItemKind::Artifact,
            text: "artifact://0".to_string(),
            target_uri: "project://tempestmiku/artifacts/0".to_string(),
            source_session_id: session.id,
            source_event_seq: None,
            source_uri: Some("artifact://0".to_string()),
            dedupe_key: "test-artifact".to_string(),
            provenance_json: json!({"sourceSession": session.id}),
        })
        .await
        .unwrap();

    for (uri, expected) in [
        ("artifact://0", "artifact line"),
        ("workspace://session/notes.md", "two"),
        ("linked://tempestmiku/README.md", "linked readme"),
        ("project://tempestmiku/resources", "artifact://0"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/sessions/{}/resources/resolve?uri={}",
                        session.id, uri
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = response_json(response).await;
        assert!(
            json["content"].as_str().unwrap().contains(expected),
            "content for {uri}: {json}"
        );
    }

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=workspace://session/../secret",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let unknown = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=drive://later",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn project_environment_gateway_is_scoped_and_fails_closed_after_archive() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create_project_session(&app).await;
    let environment_uri = "project://tempestmiku/environment";

    let empty = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri={environment_uri}",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(empty.status(), StatusCode::OK);
    let empty = response_json(empty).await;
    assert_eq!(
        serde_json::from_str::<Value>(empty["content"].as_str().unwrap()).unwrap(),
        json!({"status": "empty"})
    );

    let now = Utc::now();
    let cognition = store
        .upsert_environment_cognition(EnvironmentCognitionRecord {
            id: Uuid::new_v4(),
            owner_subject: "brian".to_string(),
            memory_scope: "project:tempestmiku".to_string(),
            title: "Environment cognition for project:tempestmiku".to_string(),
            body:
                "Capability families in active use: fs.\nRecurring failure families: none observed."
                    .to_string(),
            source_policy_ids: vec![Uuid::new_v4()],
            confidence: 0.4,
            version: 1,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();

    let populated = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri={environment_uri}",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(populated.status(), StatusCode::OK);
    let populated = response_json(populated).await;
    let content: Value = serde_json::from_str(populated["content"].as_str().unwrap()).unwrap();
    assert_eq!(content["id"], json!(cognition.id));
    assert_eq!(content["memoryScope"], json!("project:tempestmiku"));
    assert_eq!(content["version"], json!(1));

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=project://tempestmiku",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert!(
        response_json(listed)
            .await
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["uri"] == json!(environment_uri))
    );

    let other = create_with_body(
        &app,
        Body::from(r#"{"mode":"serious_engineer","projectId":"other","memoryPolicy":"project"}"#),
    )
    .await;
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri={environment_uri}",
                    other.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);

    store
        .archive_project("brian", "tempestmiku", "completed")
        .await
        .unwrap();
    let archived = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri={environment_uri}",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::NOT_FOUND);
}

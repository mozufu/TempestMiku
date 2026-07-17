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

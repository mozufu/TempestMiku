use super::*;

#[tokio::test]
async fn project_views_and_promotion_are_idempotent() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create_project_session(&app).await;
    post_user_message(
        &app,
        session.id,
        "capture an open loop and decision for the code TODO",
    )
    .await;

    let overview = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/project", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(overview.status(), StatusCode::OK);
    let overview_json = response_json(overview).await;
    assert_eq!(overview_json["projectUri"], json!("project://tempestmiku"));
    assert!(!overview_json["nextActions"].as_array().unwrap().is_empty());
    assert!(!overview_json["openLoops"].as_array().unwrap().is_empty());
    assert!(!overview_json["decisions"].as_array().unwrap().is_empty());

    let body = Body::from(
        r#"{"summary":"ship P1 slice","openLoops":["wire mobile resume"],"decisions":["keep SSE"],"resources":["artifact://0","workspace://session/notes.md"]}"#,
    );
    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/promote", session.id))
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_json = response_json(first).await;
    assert_eq!(first_json["projectUri"], json!("project://tempestmiku"));
    let first_promoted = first_json["promoted"].as_array().unwrap().clone();
    assert_eq!(first_promoted.len(), 5);
    assert_eq!(first_promoted[0]["provenanceJson"]["actor"], json!("user"));

    let second = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/promote", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"summary":"ship P1 slice","openLoops":["wire mobile resume"],"decisions":["keep SSE"],"resources":["artifact://0","workspace://session/notes.md"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_json = response_json(second).await;
    assert_eq!(
        first_promoted[0]["id"],
        second_json["promoted"].as_array().unwrap()[0]["id"]
    );
    let project_resources = store
        .project_items("tempestmiku", Some(ProjectItemKind::Artifact))
        .await
        .unwrap();
    assert_eq!(project_resources.len(), 1);
}

#[tokio::test]
async fn promotion_can_import_project_workspace_attachment_into_drive() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("linked");
    std::fs::create_dir_all(&linked_root).unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let drive_store = InMemoryDriveStore::new(ArtifactStore::open(temp.path(), "drive").unwrap());
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
    .with_linked_folders(linked)
    .with_drive_store(drive_store.clone());
    let app = app(state);
    let session = create_project_session(&app).await;
    let workspace = artifact_root
        .join("sessions")
        .join(session.id.to_string())
        .join("workspace")
        .join("notes");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("import.md"),
        "# Imported\nproject attachment",
    )
    .unwrap();

    let source_uri = "project://tempestmiku/workspace/notes/import.md";
    let promoted = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/promote", session.id))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"projectId":"tempestmiku","resources":["{source_uri}"],"importResourcesToDrive":true}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(promoted.status(), StatusCode::OK);
    let promoted = response_json(promoted).await;
    let item = &promoted["promoted"].as_array().unwrap()[0];
    assert_eq!(item["kind"], json!("workspace"));
    assert_eq!(item["sourceUri"], json!(source_uri));
    assert_eq!(
        item["targetUri"],
        json!("drive://projects/tempestmiku/attachments/notes/import.md")
    );

    let entry = drive_store
        .get("drive://projects/tempestmiku/attachments/notes/import.md")
        .unwrap();
    assert_eq!(entry.project.as_deref(), Some("tempestmiku"));
    assert_eq!(entry.source_uri.as_deref(), Some(source_uri));
    let session_id = session.id.to_string();
    assert_eq!(
        entry.provenance[0].session_id.as_deref(),
        Some(session_id.as_str())
    );

    let resolved = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri={}",
                    session.id,
                    item["targetUri"].as_str().unwrap()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status(), StatusCode::OK);
    let resolved = response_json(resolved).await;
    assert_eq!(resolved["content"], json!("# Imported\nproject attachment"));
}

#[tokio::test]
async fn miku_initiated_promotion_denies_by_default_on_timeout() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create_project_session(&app).await;
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/promote", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"summary":"proposed write","initiatedBy":"miku","timeoutMs":1}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().any(|event| event.event_type == "approval"));
    let resolved = events
        .iter()
        .find(|event| event.event_type == "approval_resolved")
        .unwrap();
    assert_eq!(resolved.payload_json["backend"], json!("project-promotion"));
    assert_eq!(resolved.payload_json["optionId"], json!("reject"));
}

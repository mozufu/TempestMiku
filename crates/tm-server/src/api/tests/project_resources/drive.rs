use super::*;

#[tokio::test]
async fn drive_resource_gateway_reads_lists_and_previews_when_configured() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let artifact_root = tempfile::tempdir().unwrap();
    let linked_root = artifact_root.path().join("linked");
    std::fs::create_dir_all(&linked_root).unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: linked_root,
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let drive_store =
        InMemoryDriveStore::new(ArtifactStore::open(artifact_root.path(), "drive").unwrap());
    let filed = drive_store
        .put_bytes(
            b"# Drive Note\nhello from drive",
            DrivePutOptions {
                auto: true,
                project: Some("TempestMiku".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.path().to_path_buf())
    .with_linked_folders(linked)
    .with_drive_store(drive_store.clone());
    let app = app(state);
    let session = create_project_session(&app).await;

    let resolved = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri={}&selector=2-2",
                    session.id, filed.uri
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status(), StatusCode::OK);
    let json = response_json(resolved).await;
    assert_eq!(json["content"], json!("hello from drive"));
    assert_eq!(json["uri"], json!(filed.uri));

    let missing_parent = filed
        .entry
        .path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    let missing = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=drive://{}/missing.md",
                    session.id, missing_parent
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let missing = response_json(missing).await;
    let error = missing["error"].as_str().unwrap();
    assert!(!error.contains("nearby paths"));
    assert!(!error.contains(&filed.entry.path));
    assert!(!error.contains(artifact_root.path().to_str().unwrap()));

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=drive://by-project/TempestMiku",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let entries = response_json(listed).await;
    assert_eq!(entries.as_array().unwrap().len(), 1);

    let feed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/drive/feed?project=tempestmiku&limit=5",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(feed.status(), StatusCode::OK);
    let feed = response_json(feed).await;
    assert_eq!(feed["recent"].as_array().unwrap().len(), 1);
    assert_eq!(feed["recent"][0]["uri"], json!(filed.uri));
    assert_eq!(feed["virtualDirs"].as_array().unwrap().len(), 5);
    assert!(feed["pendingApprovals"].as_array().unwrap().is_empty());

    let preview = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/preview?uri={}",
                    session.id, filed.uri
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    let json = response_json(preview).await;
    assert_eq!(json["content"], json!(""));
    assert!(json["preview"].as_str().unwrap().contains("Drive Note"));

    let project_entries = drive_store
        .list(DriveListOptions {
            path: Some("/by-project/TempestMiku".to_string()),
            recursive: true,
            ..DriveListOptions::default()
        })
        .unwrap();
    assert_eq!(project_entries.len(), 1);
}

#[tokio::test]
async fn drive_feed_is_scope_relative_playground() {
    // §30: drive is Miku's playground. A global session sees the unprojected shelf; a project
    // session sees only that project's shelf. Neither leaks the other's content.
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let artifact_root = tempfile::tempdir().unwrap();
    let drive_store =
        InMemoryDriveStore::new(ArtifactStore::open(artifact_root.path(), "drive").unwrap());
    let global_doc = drive_store
        .put_bytes(
            b"# Playground Note\nfiled without a project",
            DrivePutOptions {
                auto: true,
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let project_doc = drive_store
        .put_bytes(
            b"# Project Note\nfiled under tempestmiku",
            DrivePutOptions {
                auto: true,
                project: Some("TempestMiku".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.path().to_path_buf())
    .with_drive_store(drive_store.clone());
    let app = app(state);

    // Global session: the unprojected playground, never the projected doc.
    let global = create(&app).await;
    let feed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/drive/feed?limit=10", global.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(feed.status(), StatusCode::OK);
    let feed = response_json(feed).await;
    let uris = feed["recent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["uri"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(uris.contains(&global_doc.uri));
    assert!(!uris.contains(&project_doc.uri));

    // A global session may not request a project feed (object-capability boundary).
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/drive/feed?project=tempestmiku",
                    global.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);

    // Project session: only that project's shelf.
    let scoped = create_project_session(&app).await;
    let feed = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/drive/feed?limit=10", scoped.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(feed.status(), StatusCode::OK);
    let feed = response_json(feed).await;
    let uris = feed["recent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["uri"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(uris.contains(&project_doc.uri));
    assert!(!uris.contains(&global_doc.uri));
}

use super::*;

#[tokio::test]
async fn project_catalog_lists_live_local_and_remote_aliases_without_host_details() {
    let temp = tempfile::tempdir().unwrap();
    let local_root = temp.path().join("local-project");
    std::fs::create_dir_all(&local_root).unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "zeta".to_string(),
        path: local_root.clone(),
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: Vec::new(),
    }])
    .unwrap()
    .with_virtual_aliases(["alpha".to_string()])
    .unwrap();
    let store = Arc::new(InMemoryStore::default());
    let state = AppState::new(
        store.clone(),
        Arc::new(StoreMemoryProvider::new(store)),
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_linked_folders(linked.clone());
    let (app, _) = test_app_with_state(state);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let catalog = response_json(response).await;
    assert_eq!(
        catalog,
        json!({
            "projects": [
                {
                    "id": "alpha",
                    "memoryScope": "project:alpha",
                    "projectUri": "project://alpha",
                    "linkedFoldersUri": "project://alpha/linked-folders"
                },
                {
                    "id": "zeta",
                    "memoryScope": "project:zeta",
                    "projectUri": "project://zeta",
                    "linkedFoldersUri": "project://zeta/linked-folders"
                }
            ]
        })
    );
    let encoded = catalog.to_string();
    assert!(!encoded.contains(local_root.to_string_lossy().as_ref()));
    assert!(!encoded.contains("cargo"));
    assert!(!encoded.contains("worker"));

    linked.remove_policy("zeta").unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response_json(response).await,
        json!({
            "projects": [{
                "id": "alpha",
                "memoryScope": "project:alpha",
                "projectUri": "project://alpha",
                "linkedFoldersUri": "project://alpha/linked-folders"
            }]
        })
    );
}

#[tokio::test]
async fn empty_catalog_and_project_resource_root_are_navigable() {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemoryStore::default());
    let linked = LinkedFolders::default();
    let state = AppState::new(
        store.clone(),
        Arc::new(StoreMemoryProvider::new(store)),
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_linked_folders(linked.clone());
    let (app, _) = test_app_with_state(state);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, json!({"projects": []}));

    let session = create(&app).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=project://",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, json!([]));

    linked
        .insert_policy(tm_host::FsPolicy {
            alias: "runtime".to_string(),
            root: temp.path().canonicalize().unwrap(),
            mode: FsMode::Ro,
            commands: std::collections::BTreeSet::new(),
            safe_args: Vec::new(),
        })
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response_json(response).await,
        json!({
            "projects": [{
                "id": "runtime",
                "memoryScope": "project:runtime",
                "projectUri": "project://runtime",
                "linkedFoldersUri": "project://runtime/linked-folders"
            }]
        })
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=project://",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response_json(response).await,
        json!([{
            "uri": "project://runtime",
            "name": "runtime",
            "kind": "project",
            "title": "runtime",
            "sizeBytes": null,
            "modifiedAt": null
        }])
    );

    linked.remove_policy("runtime").unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response_json(response).await, json!({"projects": []}));
}

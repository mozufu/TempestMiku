use super::*;

#[tokio::test]
async fn project_catalog_lists_entities_with_attached_folders_without_host_details() {
    let temp = tempfile::tempdir().unwrap();
    let local_root = temp.path().join("local-project");
    std::fs::create_dir_all(&local_root).unwrap();
    // A linked folder aliased `zeta` is attached to the `zeta` project entity; `alpha` is a
    // folderless project. §30: the catalog is the entity list, and folders resolve as attachments.
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "zeta".to_string(),
        path: local_root.clone(),
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: Vec::new(),
    }])
    .unwrap();
    let store = Arc::new(InMemoryStore::default());
    store
        .ensure_project("alpha", "Alpha", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    store
        .ensure_project("zeta", "Zeta", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    let state = AppState::new(
        store.clone(),
        Arc::new(StoreMemoryProvider::new(store.clone())),
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
                    "title": "Alpha",
                    "status": "active",
                    "memoryScope": "project:alpha",
                    "defaultMemoryPolicy": "project",
                    "projectUri": "project://alpha",
                    "linkedFoldersUri": "project://alpha/linked-folders",
                    "linkedFolderUris": []
                },
                {
                    "id": "zeta",
                    "title": "Zeta",
                    "status": "active",
                    "memoryScope": "project:zeta",
                    "defaultMemoryPolicy": "project",
                    "projectUri": "project://zeta",
                    "linkedFoldersUri": "project://zeta/linked-folders",
                    "linkedFolderUris": ["project://zeta/linked-folders/zeta/"]
                }
            ]
        })
    );
    let encoded = catalog.to_string();
    assert!(!encoded.contains(local_root.to_string_lossy().as_ref()));
    assert!(!encoded.contains("cargo"));
    assert!(!encoded.contains("worker"));

    // §30: detaching the folder does not remove the project; it keeps its memory scope and simply
    // shows no attached folder.
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
    let catalog = response_json(response).await;
    let zeta = catalog["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == json!("zeta"))
        .expect("zeta project survives folder detach");
    assert_eq!(zeta["linkedFolderUris"], json!([]));
    assert_eq!(zeta["status"], json!("active"));
}

#[tokio::test]
async fn empty_catalog_and_project_resource_root_are_navigable() {
    let store = Arc::new(InMemoryStore::default());
    let linked = LinkedFolders::default();
    let state = AppState::new(
        store.clone(),
        Arc::new(StoreMemoryProvider::new(store.clone())),
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

    // §30: a folderless project entity appears in the catalog and the resource root.
    store
        .ensure_project("runtime", "Runtime", crate::MemoryPolicy::Project)
        .await
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
                "title": "Runtime",
                "status": "active",
                "memoryScope": "project:runtime",
                "defaultMemoryPolicy": "project",
                "projectUri": "project://runtime",
                "linkedFoldersUri": "project://runtime/linked-folders",
                "linkedFolderUris": []
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
            "title": "Runtime",
            "sizeBytes": null,
            "modifiedAt": null
        }])
    );

    // Archiving the project removes it from the active catalog (§30.4).
    store
        .archive_project("brian", "runtime", "done")
        .await
        .unwrap();
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

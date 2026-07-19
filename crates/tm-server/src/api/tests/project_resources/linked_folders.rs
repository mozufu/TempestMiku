use super::*;

struct RemoteLinkedFixture;

#[async_trait]
impl ResourceHandler for RemoteLinkedFixture {
    fn scheme(&self) -> &str {
        "linked"
    }

    fn capability(&self) -> &str {
        "resources.read:linked"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<tm_artifacts::ResourceContent> {
        assert_eq!(ctx.session_scope.as_deref(), Some("project:tempestmiku"));
        Ok(tm_artifacts::ResourceContent {
            uri: uri.to_string(),
            kind: "text".to_string(),
            mime: "text/plain".to_string(),
            title: Some("README.md".to_string()),
            size_bytes: 13,
            selector: selector.map(str::to_string),
            has_more: false,
            preview: "remote worker".to_string(),
            content: "remote worker".to_string(),
        })
    }

    async fn list(
        &self,
        _uri: Option<&str>,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Vec<ResourceEntry>> {
        assert_eq!(ctx.session_scope.as_deref(), Some("project:tempestmiku"));
        Ok(vec![ResourceEntry {
            uri: "linked://tempestmiku/README.md".to_string(),
            name: "README.md".to_string(),
            kind: "text".to_string(),
            title: None,
            size_bytes: Some(13),
            modified_at: None,
        }])
    }
}

#[tokio::test]
async fn virtual_linked_alias_uses_remote_resource_handler() {
    let linked = LinkedFolders::default()
        .with_virtual_aliases(["tempestmiku".to_string()])
        .unwrap();
    let store = Arc::new(InMemoryStore::default());
    let state = AppState::new(
        store.clone(),
        Arc::new(StoreMemoryProvider::new(store)),
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_linked_folders(linked)
    .with_linked_resource_handler(Arc::new(RemoteLinkedFixture));
    let (app, _) = test_app_with_state(state);
    let session = create_project_session(&app).await;

    let resolved = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=linked://tempestmiku/README.md&selector=1-1",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status(), StatusCode::OK);
    assert_eq!(
        response_json(resolved).await["content"],
        json!("remote worker")
    );

    let listed = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=linked://tempestmiku/",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(
        response_json(listed).await.as_array().unwrap()[0]["uri"],
        json!("linked://tempestmiku/README.md")
    );
}

#[tokio::test]
async fn project_linked_folder_view_lists_and_reads_shared_links() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("linked");
    std::fs::create_dir_all(&linked_root).unwrap();
    std::fs::write(linked_root.join("README.md"), "one\ntwo\nthree").unwrap();
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
    .with_artifact_root(artifact_root)
    .with_linked_folders(linked.clone());
    let (app, _) = test_app_with_state(state);
    let session = create_project_session(&app).await;

    let root = app
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
    assert_eq!(root.status(), StatusCode::OK);
    let root_entries = response_json(root).await;
    assert!(
        root_entries
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["uri"] == json!("project://tempestmiku/linked-folders"))
    );
    assert!(
        root_entries
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["uri"] == json!("project://tempestmiku/memory"))
    );

    let memory = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=project://tempestmiku/memory",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(memory.status(), StatusCode::OK);
    let memory = response_json(memory).await;
    let memory_view: Value = serde_json::from_str(memory["content"].as_str().unwrap()).unwrap();
    assert_eq!(memory_view["scope"], json!("project:tempestmiku"));
    assert_eq!(
        memory_view["chunksUri"],
        json!("memory://scopes/project:tempestmiku/chunks")
    );
    assert_eq!(memory_view["mode"], json!("rw"));
    assert_eq!(memory_view["linkedUri"], json!("linked://tempestmiku/"));

    linked
        .insert_policy(tm_host::FsPolicy {
            alias: "tempestmiku".to_string(),
            root: linked_root.canonicalize().unwrap(),
            mode: FsMode::Ro,
            commands: std::collections::BTreeSet::new(),
            safe_args: Vec::new(),
        })
        .unwrap();
    let narrowed_memory = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=project://tempestmiku/memory",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(narrowed_memory.status(), StatusCode::OK);
    let narrowed_memory = response_json(narrowed_memory).await;
    let narrowed_view: Value =
        serde_json::from_str(narrowed_memory["content"].as_str().unwrap()).unwrap();
    assert_eq!(narrowed_view["mode"], json!("ro"));

    let other_memory = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=project://other/memory",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(other_memory.status(), StatusCode::NOT_FOUND);

    let memory_entries = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=project://tempestmiku/memory",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(memory_entries.status(), StatusCode::OK);
    let memory_entries = response_json(memory_entries).await;
    assert_eq!(
        memory_entries.as_array().unwrap()[0]["uri"],
        json!("memory://scopes/project:tempestmiku/chunks")
    );

    let links = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=project://tempestmiku/linked-folders",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(links.status(), StatusCode::OK);
    let link_entries = response_json(links).await;
    assert_eq!(
        link_entries.as_array().unwrap()[0]["uri"],
        json!("project://tempestmiku/linked-folders/tempestmiku/")
    );
    let other_links = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=project://other/linked-folders",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(other_links.status(), StatusCode::NOT_FOUND);

    let files = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=project://tempestmiku/linked-folders/tempestmiku/",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(files.status(), StatusCode::OK);
    let file_entries = response_json(files).await;
    assert!(
        file_entries
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["uri"]
                == json!("project://tempestmiku/linked-folders/tempestmiku/README.md"))
    );

    let resolved = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=project://tempestmiku/linked-folders/tempestmiku/README.md&selector=2-2",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status(), StatusCode::OK);
    let resolved = response_json(resolved).await;
    assert_eq!(
        resolved["uri"],
        json!("project://tempestmiku/linked-folders/tempestmiku/README.md")
    );
    assert_eq!(resolved["selector"], json!("2-2"));
    assert_eq!(resolved["content"], json!("two"));

    let scoped_recall = "revoked linked memory should not leak this recall";
    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "project:tempestmiku".to_string(),
            text: scoped_recall.to_string(),
            source: "linked-revocation-test".to_string(),
            importance: 0.8,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    linked.remove_policy("tempestmiku").unwrap();
    let revoked_memory = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=project://tempestmiku/memory",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked_memory.status(), StatusCode::NOT_FOUND);
    let revoked_body = response_json(revoked_memory).await;
    assert!(!revoked_body.to_string().contains(scoped_recall));
    assert!(
        revoked_body
            .to_string()
            .contains("active linked project scope project:tempestmiku")
    );

    let revoked_file = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=project://tempestmiku/linked-folders/tempestmiku/README.md",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked_file.status(), StatusCode::NOT_FOUND);
}

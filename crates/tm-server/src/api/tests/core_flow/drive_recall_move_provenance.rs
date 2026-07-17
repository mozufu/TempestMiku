use super::*;
use tm_artifacts::ArtifactStore;
use tm_drive::{DrivePutOptions, InMemoryDriveStore};

#[tokio::test]
async fn chat_turn_prompt_includes_bounded_drive_recall_for_project_scope() {
    let artifact_root = tempfile::tempdir().unwrap();
    let drive_store =
        InMemoryDriveStore::new(ArtifactStore::open(artifact_root.path(), "drive").unwrap());
    let filed = drive_store
        .put_bytes(
            b"# P5 Drive Brief\nDrive documents should be recalled by summary only.",
            DrivePutOptions {
                auto: true,
                project: Some("TempestMiku".to_string()),
                source_uri: Some("artifact://drive-source".to_string()),
                session_id: Some("session-a".to_string()),
                event_seq: Some(12),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let store = Arc::new(InMemoryStore::default());
    let store_for_assert = Arc::clone(&store);
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(RecordingChatRunner::default());
    let turns = Arc::clone(&chat.turns);
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: artifact_root.path().to_path_buf(),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.path().to_path_buf())
    .with_linked_folders(linked)
    .with_drive_store(drive_store);
    let app = app(state);
    let session = create_with_body(&app, Body::from(r#"{"scope":"project:tempestmiku"}"#)).await;

    post_user_message(&app, session.id, "recall local drive docs").await;

    let rejected_scope_override = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "clientMessageId": Uuid::new_v4(),
                        "content": "recall other docs",
                        "scope": "project:other",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        rejected_scope_override.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    {
        let recorded = turns.lock();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].user_prompt.contains("Drive recall"));
        assert!(recorded[0].user_prompt.contains(&filed.uri));
        assert!(
            recorded[0]
                .user_prompt
                .contains("raw content remains behind drive://")
        );
    }

    let chunks = store_for_assert
        .recall_chunks("project:tempestmiku", "Drive document", 10)
        .await
        .unwrap();
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].text.contains(&filed.uri));
    assert!(chunks[0].text.contains(&filed.entry.content_hash));
    let doc_kind_attr = filed
        .entry
        .attributes
        .iter()
        .find(|attr| attr.key == "doc_kind")
        .unwrap();
    assert!(
        chunks[0]
            .text
            .contains(&format!("doc_kind={}", doc_kind_attr.value))
    );
    assert!(chunks[0].source.contains("drive://"));
    assert!(chunks[0].source.contains(&filed.entry.content_hash));
    assert!(
        chunks[0]
            .source
            .contains("extractor:tm-drive:fallback-transducer:v1")
    );
    assert!(chunks[0].source.contains("source_session:session-a"));
    assert!(chunks[0].source.contains("source_event:12"));
    let memory_preview = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/preview?uri=memory://scopes/project:tempestmiku/chunks/{}",
                    session.id, chunks[0].id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(memory_preview.status(), StatusCode::OK);
    let memory_preview = response_json(memory_preview).await;
    assert_eq!(memory_preview["kind"], json!("memory_recall_chunk"));
    assert_eq!(memory_preview["content"], json!(""));
    assert!(
        memory_preview["preview"]
            .as_str()
            .unwrap()
            .contains("Drive document")
    );

    let scope_change = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/scope", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"scope":"global"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(scope_change.status(), StatusCode::OK);
    post_user_message(&app, session.id, "recall global docs").await;
    {
        let recorded = turns.lock();
        assert_eq!(recorded.len(), 2);
        assert!(!recorded[1].user_prompt.contains("Drive recall"));
    }

    let chunks_after = store_for_assert
        .recall_chunks("project:tempestmiku", "Drive document", 10)
        .await
        .unwrap();
    assert_eq!(chunks_after.len(), 1);
    let global_chunks = store_for_assert
        .recall_chunks("global", "Drive document", 10)
        .await
        .unwrap();
    assert!(global_chunks.is_empty());
}

#[tokio::test]
async fn drive_recall_chunk_updates_after_move_and_tag_without_losing_provenance() {
    let artifact_root = tempfile::tempdir().unwrap();
    let drive_store =
        InMemoryDriveStore::new(ArtifactStore::open(artifact_root.path(), "drive").unwrap());
    let filed = drive_store
        .put_bytes(
            b"# P5 Drive Brief\nDrive documents should update recall metadata.",
            DrivePutOptions {
                auto: true,
                project: Some("TempestMiku".to_string()),
                source_uri: Some("artifact://drive-source".to_string()),
                session_id: Some("session-a".to_string()),
                event_seq: Some(12),
                suggested_path: Some("projects/tempestmiku/notes/original.md".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let store = Arc::new(InMemoryStore::default());
    let store_for_assert = Arc::clone(&store);
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(RecordingChatRunner::default());
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: artifact_root.path().to_path_buf(),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.path().to_path_buf())
    .with_linked_folders(linked)
    .with_drive_store(drive_store.clone());
    let app = app(state);
    let session = create_with_body(&app, Body::from(r#"{"scope":"project:tempestmiku"}"#)).await;

    post_user_message(&app, session.id, "recall local drive docs").await;
    let original_chunks = store_for_assert
        .recall_chunks("project:tempestmiku", "Drive document", 10)
        .await
        .unwrap();
    assert_eq!(original_chunks.len(), 1);
    assert!(original_chunks[0].text.contains(&filed.uri));

    drive_store
        .move_entry(
            &filed.uri,
            "projects/tempestmiku/notes/moved.md",
            tm_drive::DriveCollisionStrategy::Reject,
        )
        .unwrap();
    drive_store
        .tag_entry(
            "projects/tempestmiku/notes/moved.md",
            vec!["urgent".to_string()],
        )
        .unwrap();
    post_user_message(&app, session.id, "recall moved drive docs").await;

    let updated_chunks = store_for_assert
        .recall_chunks("project:tempestmiku", "Drive document", 10)
        .await
        .unwrap();
    assert_eq!(updated_chunks.len(), 1);
    assert_eq!(updated_chunks[0].id, original_chunks[0].id);
    assert!(
        updated_chunks[0]
            .text
            .contains("drive://projects/tempestmiku/notes/moved.md")
    );
    assert!(updated_chunks[0].text.contains("Tags: note, urgent"));
    assert!(updated_chunks[0].source.contains(&filed.entry.content_hash));
    assert!(
        updated_chunks[0]
            .source
            .contains("source_session:session-a")
    );
    assert!(updated_chunks[0].source.contains("source_event:12"));
}

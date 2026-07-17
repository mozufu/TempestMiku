use super::super::*;
use super::support::native_tool_result;

#[tokio::test]
async fn native_tm_drive_organizer_events_are_persisted_for_replay() {
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
    let drive_store = tm_drive::InMemoryDriveStore::new(
        tm_artifacts::ArtifactStore::open(temp.path(), "drive").unwrap(),
    );
    let code = r##"
let filed = @drive.put {content: "# Raw\norganizer should move this into project notes", options: {
  suggestedPath: "inbox/raw.md",
  project: "TempestMiku",
  docKind: "note",
  approvalMode: "auto"
}};
let proposals = @drive.organize {};
let proposal = match proposals { | first :: _ -> first | [] -> null };
{
  proposals: length proposals,
  status: proposal.status,
  source: proposal.sourcePath,
  target: proposal.proposedPath
} |> display {kind: "json"}
"##;
    let tool_args = json!({ "code": code }).to_string();
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_drive_organize".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(tool_args),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("drive organizer checked".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
    ]));
    let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
    let cfg = AgentConfig {
        model: "fake".to_string(),
        max_turns: 3,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone())
    .with_linked_folders(linked.clone())
    .with_drive_store(drive_store.clone());
    let backend = NativeTmBackend::new(
        llm_for_backend,
        cfg,
        TmSandboxOptions {
            artifact_root,
            drive_store: Some(Arc::new(drive_store)),
            linked_folders: Some(linked),
            approval_timeout: Duration::from_secs(1),
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Deny,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);
    let session = create_with_body(
        &router,
        Body::from(r#"{"mode":"serious_engineer","scope":"project:tempestmiku"}"#),
    )
    .await;

    post_user_message(&router, session.id, "organize drive docs").await;

    let tool_result = native_tool_result(&llm);
    assert!(tool_result.contains("\"proposals\": 1"), "{tool_result}");
    assert!(
        tool_result.contains("\"status\": \"pending\""),
        "{tool_result}"
    );
    let events = store.events_after(session.id, None).await.unwrap();
    let filed = events
        .iter()
        .find(|event| event.event_type == "drive_put")
        .expect("drive put event");
    assert_eq!(filed.payload_json["uri"], json!("drive://inbox/raw.md"));
    assert_eq!(
        filed.payload_json["preview"]["title"],
        json!("Filed drive document")
    );
    assert_eq!(
        filed.payload_json["resourceRefs"][0]["uri"],
        json!("drive://inbox/raw.md")
    );
    let started = events
        .iter()
        .find(|event| event.event_type == "drive_organizer_started")
        .expect("drive organizer start event");
    assert_eq!(started.payload_json["apply"], json!(false));
    let pending = events
        .iter()
        .find(|event| event.event_type == "write_proposal")
        .expect("drive write proposal event");
    assert_eq!(pending.payload_json["kind"], json!("drive"));
    assert_eq!(pending.payload_json["status"], json!("pending"));
    assert_eq!(
        pending.payload_json["sourceUri"],
        json!("drive://inbox/raw.md")
    );
    assert_eq!(
        pending.payload_json["proposedUri"],
        json!("drive://projects/tempestmiku/note/raw.md")
    );
    let completed = events
        .iter()
        .find(|event| event.event_type == "drive_organizer_completed")
        .expect("drive organizer completion event");
    assert_eq!(completed.payload_json["proposalCount"], json!(1));
    assert_eq!(
        completed.payload_json["proposals"][0]["sourceUri"],
        json!("drive://inbox/raw.md")
    );
    assert_eq!(
        completed.payload_json["proposals"][0]["proposedUri"],
        json!("drive://projects/tempestmiku/note/raw.md")
    );
    assert_eq!(
        completed.payload_json["resourceRefs"][0]["uri"],
        json!("drive://inbox/raw.md")
    );
    let replay = store
        .events_after(session.id, Some(started.seq - 1))
        .await
        .unwrap();
    assert!(
        replay
            .iter()
            .any(|event| event.event_type == "drive_organizer_started")
    );
    assert!(
        replay
            .iter()
            .any(|event| event.event_type == "drive_organizer_completed")
    );
    let transcript = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/messages", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(transcript.status(), StatusCode::OK);
    let transcript = response_json(transcript).await;
    let pending_events = transcript["pendingEvents"].as_array().unwrap();
    assert!(pending_events.iter().any(|event| {
        event["type"] == json!("write_proposal")
            && event["data"]["kind"] == json!("drive")
            && event["data"]["sourceUri"] == json!("drive://inbox/raw.md")
            && event["data"]["proposedUri"] == json!("drive://projects/tempestmiku/note/raw.md")
    }));
}

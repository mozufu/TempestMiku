use super::*;
use crate::scheduler::{WEEKLY_SHIP_LEDGER_JOB_ID, WEEKLY_SHIP_LEDGER_SCHEDULE};
use crate::{NewCronJobRecord, NewCronRunRecord};
use tm_memory::{
    DreamReason, DreamStatus, MemoryEvidenceRef, MemorySummaryKind, NewDreamQueueRecord,
    NewMemorySummaryRecord, NewSkillProposalRecord, SkillVerification,
};

#[tokio::test]
async fn project_views_and_promotion_are_idempotent() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"content":"capture an open loop and decision for the code TODO"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

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
async fn resource_gateway_reads_supported_schemes_and_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("linked");
    std::fs::create_dir_all(&linked_root).unwrap();
    std::fs::write(linked_root.join("README.md"), "linked readme").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: linked_root,
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone())
    .with_linked_folders(linked);
    let (app, store) = test_app_with_state(state);
    let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
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
async fn cron_resource_gateway_reads_jobs_and_run_history() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let now = Utc::now();
    store
        .upsert_cron_job(NewCronJobRecord {
            id: WEEKLY_SHIP_LEDGER_JOB_ID.to_string(),
            name: "Weekly ship ledger".to_string(),
            schedule: WEEKLY_SHIP_LEDGER_SCHEDULE.to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 8,
            script_timeout_seconds: 120,
            next_run_at: Some(now),
        })
        .await
        .unwrap();
    let run = store
        .record_cron_run(NewCronRunRecord {
            job_id: WEEKLY_SHIP_LEDGER_JOB_ID.to_string(),
            scheduled_for: now,
            status: "completed".to_string(),
            session_id: Some(session.id),
            result_json: json!({"sessionId": session.id}),
        })
        .await
        .unwrap();

    for (uri, expected) in [
        ("cron://".to_string(), "weekly-ship-ledger".to_string()),
        (
            format!("cron://{WEEKLY_SHIP_LEDGER_JOB_ID}"),
            "cronMode".to_string(),
        ),
        (
            format!("cron://{WEEKLY_SHIP_LEDGER_JOB_ID}/runs"),
            run.id.to_string(),
        ),
        (
            format!("cron://{WEEKLY_SHIP_LEDGER_JOB_ID}/runs/{}", run.id),
            "completed".to_string(),
        ),
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
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let json = response_json(response).await;
        assert!(
            json["content"].as_str().unwrap().contains(&expected),
            "content for {uri}: {json}"
        );
    }

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=cron://{}",
                    session.id, WEEKLY_SHIP_LEDGER_JOB_ID
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
            .any(|entry| entry["uri"]
                == json!(format!(
                    "cron://{WEEKLY_SHIP_LEDGER_JOB_ID}/runs/{}",
                    run.id
                )))
    );
}

#[tokio::test]
async fn skill_resource_gateway_is_prompt_composition_only_until_p4() {
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

#[tokio::test]
async fn memory_resource_gateway_reads_root_user_model_and_records() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let fact_id = Uuid::new_v4();
    let chunk_id = Uuid::new_v4();
    store
        .add_profile_fact(ProfileFactRecord {
            id: fact_id,
            subject: "brian".to_string(),
            predicate: "prefers".to_string(),
            object: "memory resource tests".to_string(),
            confidence: 0.95,
            importance: 0.72,
            provenance: "test".to_string(),
            valid_from: Utc::now(),
            valid_to: None,
        })
        .await
        .unwrap();
    store
        .add_recall_chunk(RecallChunkRecord {
            id: chunk_id,
            scope: "global".to_string(),
            text: "scoped memory resource recall".to_string(),
            source: "test".to_string(),
            importance: 0.65,
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let fact_uri = format!("memory://profile/brian/facts/{fact_id}");
    let chunk_uri = format!("memory://scopes/global/chunks/{chunk_id}");
    for (uri, expected) in [
        ("memory://root".to_string(), "memory://user-model"),
        ("memory://user-model".to_string(), "memory resource tests"),
        (fact_uri, "Predicate: prefers"),
        (chunk_uri, "scoped memory resource recall"),
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
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let json = response_json(response).await;
        assert_eq!(json["uri"], json!(uri));
        assert!(
            json["content"].as_str().unwrap().contains(expected),
            "content for {uri}: {json}"
        );
    }
}

#[tokio::test]
async fn memory_resource_gateway_reads_dream_summaries_and_skill_proposals() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let dream_id = Uuid::new_v4();
    let evidence = vec![MemoryEvidenceRef {
        session_id: session.id,
        event_seq: Some(2),
        message_seq: Some(1),
        uri: Some("artifact://0".to_string()),
        label: "test-evidence".to_string(),
    }];
    let summary = store
        .upsert_memory_summary(NewMemorySummaryRecord {
            kind: MemorySummaryKind::Session,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            title: "Dream summary resource".to_string(),
            body: "Summary body with provenance evidence.".to_string(),
            evidence: evidence.clone(),
            source_dream_id: dream_id,
            source_session_id: Some(session.id),
            dedupe_key: format!("summary:test:{}", session.id),
        })
        .await
        .unwrap();
    let skill = store
        .upsert_skill_proposal(NewSkillProposalRecord {
            name: "dream-test-workflow".to_string(),
            description: "Test skill proposal".to_string(),
            body: "# dream-test-workflow\n\n## Trigger\nTest\n\n## Procedure\n- Do it.\n"
                .to_string(),
            trigger: "Test recurring workflow".to_string(),
            use_criteria: "Only in tests".to_string(),
            evidence,
            self_critique: "Narrow enough for review.".to_string(),
            verification: SkillVerification {
                passed: true,
                checks: vec!["shape:pass".to_string()],
            },
            dedupe_key: format!("skill:test:{}", session.id),
            source_dream_id: dream_id,
            source_session_id: session.id,
        })
        .await
        .unwrap();

    for (uri, expected) in [
        (
            format!("memory://summaries/{}", summary.id),
            "Summary body with provenance evidence.",
        ),
        (
            format!("memory://skill-proposals/{}", skill.id),
            "Test skill proposal",
        ),
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
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let json = response_json(response).await;
        assert_eq!(json["uri"], json!(uri));
        assert!(
            json["content"].as_str().unwrap().contains(expected),
            "content for {uri}: {json}"
        );
    }

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=memory://summaries",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["uri"] == json!(format!("memory://summaries/{}", summary.id)))
    );
}

#[tokio::test]
async fn memory_resource_gateway_reads_dream_queue_and_records() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:resource:{}", session.id),
            source_event_seq: Some(3),
            available_at: Utc::now(),
        })
        .await
        .unwrap();
    let other_scope = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "project:other".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:resource:other:{}", session.id),
            source_event_seq: None,
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let queue = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=memory://dreams",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(queue.status(), StatusCode::OK);
    let queue = response_json(queue).await;
    assert_eq!(queue["uri"], json!("memory://dreams"));
    let queue_content = queue["content"].as_str().unwrap();
    assert!(queue_content.contains(&format!("memory://dreams/{}", dream.id)));
    assert!(queue_content.contains("status=queued"));
    assert!(!queue_content.contains(&other_scope.id.to_string()));

    let record_uri = format!("memory://dreams/{}", dream.id);
    let record = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri={}",
                    session.id, record_uri
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(record.status(), StatusCode::OK);
    let record = response_json(record).await;
    assert_eq!(record["uri"], json!(record_uri));
    let record_content = record["content"].as_str().unwrap();
    assert!(record_content.contains("Source event seq: 3"));
    assert!(record_content.contains(&format!("Status: {}", DreamStatus::Queued)));
    assert!(record_content.contains(&format!("Session: {}", session.id)));

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=memory://dreams",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    let entries = listed.as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry["uri"] == json!(format!("memory://dreams/{}", dream.id)))
    );
    assert!(
        entries
            .iter()
            .all(|entry| entry["uri"] != json!(format!("memory://dreams/{}", other_scope.id)))
    );
}

#[tokio::test]
async fn memory_resource_gateway_denies_unknown_and_ungranted_reads() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let unknown = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri=memory://secret",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::FORBIDDEN);

    let mut registry = ResourceRegistry::new();
    registry.register(Arc::new(crate::memory::MemoryResourceHandler::new(
        Arc::clone(&store),
        "brian",
        "global",
    )));
    let ctx = InvocationCtx::new(CapabilityGrants::default());
    let err = registry
        .read("memory://root", None, &ctx)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        HostError::CapabilityDenied(capability) if capability == "resources.read:memory"
    ));
}

#[tokio::test]
async fn session_resource_preview_returns_compact_bounded_memory_preview() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "global".to_string(),
            text: "long memory ".repeat(200),
            source: "test".to_string(),
            importance: 0.65,
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/preview?uri=memory://root",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response).await;
    assert_eq!(json["content"], json!(""));
    assert_eq!(json["uri"], json!("memory://root"));
    let preview = json["preview"].as_str().unwrap();
    assert!(preview.contains("long memory"));
    assert!(preview.len() <= SESSION_RESOURCE_PREVIEW_BYTES + 64);
    assert_eq!(json["has_more"], json!(true));
}

#[tokio::test]
async fn miku_initiated_promotion_denies_by_default_on_timeout() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
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

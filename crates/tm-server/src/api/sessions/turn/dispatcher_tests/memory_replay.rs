#[derive(Default)]
struct PromptRecordingRunner {
    turns: Arc<Mutex<Vec<ChatTurn>>>,
}

#[async_trait]
impl ChatRunner for PromptRecordingRunner {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        self.turns.lock().unwrap().push(turn);
        sink.on_final("recorded retry");
        Ok("recorded retry".to_string())
    }
}

use super::*;

#[tokio::test]
async fn durable_memory_search_event_is_not_automatically_injected_on_retry() {
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let persona = tm_modes::ModesConfig::default();
    let assets = persona.load_assets();
    let session = store
        .create_session(NewSession {
            mode: assets.modes.default_mode(),
            persona_status: assets.status.clone(),
        })
        .await
        .unwrap();
    let turn = store
        .enqueue_turn(session.id, "memory-retry", "retry the durable turn")
        .await
        .unwrap();
    let memory = MemoryContext::from_records(
        "owner",
        "global",
        Vec::new(),
        vec![RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "global".to_string(),
            text: "persisted search result remains behind memory.search".to_string(),
            source: "memory://fixture/retry".to_string(),
            importance: 0.8,
            created_at: Utc::now(),
        }],
        1_600,
    )
    .mark_lexical_fallback("fixture_restart");
    store
        .append_event_for_turn(
            session.id,
            "memory_recall",
            json!({
                "schemaVersion": 1,
                "resourceUri": format!("memory://recalls/{}", turn.id),
                "context": memory,
            }),
            Some(turn.id),
        )
        .await
        .unwrap();
    let state = AppState::new(
        Arc::clone(&store),
        Arc::new(ReplayMustNotQueryMemory),
        Arc::new(EchoChatRunner),
        persona,
        AuthConfig::NoAuth,
    )
    .with_auto_turn_dispatcher(false);

    let output = execute_turn_body(&state, &turn).await.unwrap();
    assert_eq!(output.response, "Miku heard: retry the durable turn");
    assert_eq!(
        store
            .events_by_type(session.id, "memory_recall", 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn persisted_skill_selection_overrides_new_reliability_suppression() {
    use sha2::{Digest, Sha256};

    let root = tempfile::tempdir().unwrap();
    let body = "# Release workflow\n\nPERSISTED_SKILL_BODY\n".to_string();
    let digest = format!("sha256:{:x}", Sha256::digest(body.as_bytes()));
    let persona = tm_modes::ModesConfig::default().with_managed_skills_path(root.path());
    let active = persona
        .install_managed_skill(tm_modes::ManagedSkillInstall {
            name: "release-workflow".to_string(),
            content_digest: digest,
            body,
            source_proposal_id: Uuid::new_v4().to_string(),
            description: "release workflow".to_string(),
            triggers: vec!["release notes".to_string()],
            use_criteria: "Use for release notes.".to_string(),
        })
        .unwrap()
        .active;
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let assets = persona.load_assets();
    let session = store
        .create_session(NewSession {
            mode: assets.modes.default_mode(),
            persona_status: assets.status.clone(),
        })
        .await
        .unwrap();
    let turn = store
        .enqueue_turn(session.id, "skill-retry", "draft release notes")
        .await
        .unwrap();
    store
        .record_skill_exposures_for_turn(
            session.id,
            turn.id,
            &[(active.name.clone(), active.content_digest.clone())],
        )
        .await
        .unwrap();
    for _ in 0..3 {
        store
            .record_skill_outcome(&active.name, &active.content_digest, false)
            .await
            .unwrap();
    }
    let chat = Arc::new(PromptRecordingRunner::default());
    let turns = Arc::clone(&chat.turns);
    let state = AppState::new(
        Arc::clone(&store),
        Arc::new(StoreMemoryProvider::new(Arc::clone(&store))),
        chat,
        persona,
        AuthConfig::NoAuth,
    )
    .with_evolution_config(crate::EvolutionDreamConfig {
        reliability_archive: 0.3,
        min_trials_archive: 3,
        ..crate::EvolutionDreamConfig::default()
    })
    .with_auto_turn_dispatcher(false);

    execute_turn_body(&state, &turn).await.unwrap();
    let turns = turns.lock().unwrap();
    assert_eq!(turns.len(), 1);
    assert!(turns[0].system_prompt.contains("PERSISTED_SKILL_BODY"));
}

#[tokio::test]
async fn persisted_memory_recall_rechecks_a_revoked_project_scope() {
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let persona = tm_modes::ModesConfig::default();
    let assets = persona.load_assets();
    let session = store
        .create_session(NewSession {
            mode: assets.modes.default_mode(),
            persona_status: assets.status.clone(),
        })
        .await
        .unwrap();
    store
        .ensure_project("tempestmiku", "TempestMiku", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    store
        .set_session_memory_context(
            session.id,
            Some("tempestmiku"),
            crate::MemoryPolicy::Project,
        )
        .await
        .unwrap();
    let turn = store
        .enqueue_turn(session.id, "revoked-memory-retry", "retry the durable turn")
        .await
        .unwrap();
    let memory = MemoryContext::from_records(
        "owner",
        "project:tempestmiku",
        Vec::new(),
        vec![RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "project:tempestmiku".to_string(),
            text: "revoked project memory must not survive retry".to_string(),
            source: "memory://fixture/revoked-retry".to_string(),
            importance: 0.8,
            created_at: Utc::now(),
        }],
        1_600,
    );
    store
        .append_event_for_turn(
            session.id,
            "memory_recall",
            json!({
                "schemaVersion": 1,
                "resourceUri": format!("memory://recalls/{}", turn.id),
                "context": memory,
            }),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .revoke_memory_scope("owner", "project:tempestmiku", "linked folder removed")
        .await
        .unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: std::env::current_dir().unwrap(),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let state = AppState::new(
        Arc::clone(&store),
        Arc::new(ReplayMustNotQueryMemory),
        Arc::new(EchoChatRunner),
        persona,
        AuthConfig::NoAuth,
    )
    .with_linked_folders(linked)
    .with_auto_turn_dispatcher(false);

    let error = execute_turn_body(&state, &turn).await.unwrap_err();
    assert!(
        matches!(error, ServerError::NotFound(message) if message.contains("project:tempestmiku"))
    );
}

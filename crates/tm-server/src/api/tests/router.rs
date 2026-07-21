use super::*;

#[tokio::test]
async fn general_is_the_session_creation_default() {
    let (app, _store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    assert_eq!(session.mode, ModeId::from("general"));
    assert_eq!(session.mode_state.mode, ModeId::from("general"));
    assert_eq!(session.mode_state.lock_source, None);
    assert_eq!(session.mode_state.override_source, None);
}

#[tokio::test]
async fn posting_a_message_never_auto_switches_the_mode() {
    // The keyword-substring router used to silently flip modes per turn (and revert them just
    // as silently on the next non-triggering message). That mechanism is gone: a session's mode
    // only changes via an explicit user action (lock/apply/override) or, once wired, a
    // model-proposed and user-confirmed `modes.suggest`. Posting a message alone must never
    // change it, no matter how code- or distress-flavored the content looks.
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    post_user_message(&app, session.id, "please fix this Rust code bug").await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, ModeId::from("general"));
    assert_eq!(latest.mode_state.lock_source, None);
    assert_eq!(latest.mode_state.override_source, None);
    assert_eq!(latest.mode_state.router_reason, None);

    post_user_message(&app, session.id, "i am overwhelmed and exhausted").await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, ModeId::from("general"));

    post_user_message(
        &app,
        session.id,
        "delegate this whole migration to an agent",
    )
    .await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, ModeId::from("general"));

    let events = store.events_after(session.id, None).await.unwrap();
    let mode_events = events
        .iter()
        .filter(|event| event.event_type == "mode")
        .collect::<Vec<_>>();
    assert_eq!(
        mode_events.len(),
        1,
        "only the initial session-creation mode event; no message ever adds another"
    );
}

#[tokio::test]
async fn legacy_suggest_route_does_not_apply_a_mode_switch() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    let suggest = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/suggest", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mode":"serious_engineer","reason":"old keyword router hint"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(suggest.status(), StatusCode::OK);
    let suggest_json = response_json(suggest).await;
    assert_eq!(suggest_json["changed"], json!(false));
    assert_eq!(suggest_json["modeState"]["mode"], json!("general"));
    assert!(
        suggest_json["ignoredReason"]
            .as_str()
            .unwrap()
            .contains("approval-backed"),
        "{suggest_json}"
    );

    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, ModeId::from("general"));
    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "mode")
            .count(),
        1,
        "legacy suggest must not emit a mode event"
    );
}

#[tokio::test]
async fn only_unlocked_normal_chat_turns_receive_modes_suggest_authority() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(RecordingChatRunner::default());
    let turns = Arc::clone(&chat.turns);
    let state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    );
    let app = app(state);
    let session = create(&app).await;

    post_user_message(&app, session.id, "hello").await;
    {
        let turns = turns.lock();
        assert_eq!(turns.len(), 1);
        assert!(
            turns[0]
                .capabilities
                .iter()
                .any(|capability| capability == MODE_SUGGEST_CAPABILITY)
        );
        assert_eq!(
            turns[0]
                .host_functions
                .iter()
                .map(|function| function.name())
                .collect::<Vec<_>>(),
            vec![MODE_SUGGEST_CAPABILITY]
        );
    }

    let lock = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/lock", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"mode":"general","reason":"stay light"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lock.status(), StatusCode::OK);

    post_user_message(&app, session.id, "this says repo but mode is locked").await;
    let turns = turns.lock();
    assert_eq!(turns.len(), 2);
    assert!(
        turns[1]
            .capabilities
            .iter()
            .all(|capability| capability != MODE_SUGGEST_CAPABILITY)
    );
    assert_eq!(
        turns[1]
            .host_functions
            .iter()
            .map(|function| function.name())
            .collect::<Vec<_>>(),
        vec![MODE_SUGGEST_CAPABILITY],
        "handler registration alone must not confer authority"
    );
}

#[tokio::test]
async fn coding_actor_and_scheduler_profiles_never_receive_modes_suggest() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(RecordingChatRunner::default());
    let turns = Arc::clone(&chat.turns);
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    );
    let app = app(state);
    let session = create(&app).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/override", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mode":"serious_engineer","reason":"test coding authority"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Without a native coding backend configured the granted engineering profile fails closed:
    // ChatRunner must never run, and the turn resolves to a durable/replayable backend error.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(message_body("inspect the repository"))
                .unwrap(),
        )
        .await
        .unwrap();
    let turn_id = accepted_turn_id(response).await;
    let turn = wait_for_turn(&app, session.id, turn_id).await;
    assert_eq!(turn["status"], json!("failed"), "{turn}");
    assert_eq!(
        turns.lock().len(),
        0,
        "ChatRunner must not run without a backend"
    );
    let expected = "backend error: backend.coding is granted but no coding backend is configured";
    let latest = store.turn(turn_id).await.unwrap();
    assert_eq!(latest.error.as_deref(), Some(expected));
    let events = store.events_after(session.id, None).await.unwrap();
    let error_event = events
        .iter()
        .find(|event| event.event_type == "error")
        .expect("failed turn emits a replayable error event");
    assert_eq!(
        error_event.payload_json,
        json!({ "message": expected, "turnId": turn_id })
    );

    let assets = ModesConfig::default().load_assets();
    for mode in ["serious_engineer", "handoff"] {
        let profile = assets
            .modes
            .modes
            .iter()
            .find(|profile| profile.mode == ModeId::from(mode))
            .unwrap();
        assert!(
            profile
                .capabilities
                .iter()
                .all(|capability| capability != MODE_SUGGEST_CAPABILITY),
            "actor and scheduler turns inherit static profile capabilities, so {mode} must not contain modes.suggest"
        );
    }
}

#[tokio::test]
async fn message_chat_path_applies_approved_model_mode_suggestion() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_mode".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(
                    serde_json::json!({
                        "code": "@modes.suggest {targetMode: \"serious_engineer\", reason: \"needs repository access\"}"
                    })
                    .to_string(),
                ),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("Switched; I will handle it in Serious Engineer mode.".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
    ]));
    let llm_client: Arc<dyn LlmClient> = llm.clone();
    let temp = tempfile::tempdir().unwrap();
    let chat = Arc::new(AgentChatRunner::tm(
        llm_client,
        AgentConfig {
            model: "fake".to_string(),
            max_turns: 4,
            ..AgentConfig::default()
        },
        TmSandboxOptions {
            artifact_root: temp.path().join("artifacts"),
            ..TmSandboxOptions::default()
        },
    ));
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    );
    let app = app(state);
    let session = create(&app).await;
    let session_id = session.id;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/messages"))
                .header("content-type", "application/json")
                .body(message_body("please switch if this needs repo work"))
                .unwrap(),
        )
        .await
        .unwrap();
    let turn_id = accepted_turn_id(response).await;

    let approval = wait_for_mode_approval(&store, session_id, turn_id).await;
    assert_eq!(approval["backend"], json!("mode"));
    assert_eq!(approval["scope"]["targetMode"], json!("serious_engineer"));
    assert_eq!(approval["scope"]["currentMode"], json!("general"));
    let approval_id: Uuid = serde_json::from_value(approval["approvalId"].clone()).unwrap();

    resolve_test_approval(&app, session_id, approval_id, "approve").await;
    let completed = wait_for_turn(&app, session_id, turn_id).await;
    assert_eq!(completed["status"], json!("completed"));

    let latest = store.get_session(session_id).await.unwrap();
    assert_eq!(latest.mode_state.mode, ModeId::from("serious_engineer"));
    assert_eq!(
        latest.mode_state.override_source.as_deref(),
        Some("model_suggestion")
    );
    assert_eq!(
        latest.mode_state.router_reason.as_deref(),
        Some("needs repository access")
    );

    let events = store.events_after(session_id, None).await.unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "approval_resolved"
                && event.payload_json["status"] == json!("approved"))
    );
    assert!(
        events.iter().any(|event| {
            event.event_type == "mode"
                && event.payload_json["mode"] == json!("serious_engineer")
                && event.payload_json["override_source"] == json!("model_suggestion")
        }),
        "mode change should be persisted as an event: {events:#?}"
    );
    assert!(
        events.iter().any(|event| {
            event.event_type == "tool_call" && event.payload_json["name"] == json!("execute")
        }),
        "chat sink should record only the execute tool call"
    );
    assert!(events.iter().all(|event| {
        event.event_type != "tool_call" || event.payload_json["name"] != json!("mode_suggest")
    }));
    let advertised = llm.tools.lock();
    assert!(
        advertised
            .iter()
            .all(|tools| { tools.len() == 1 && tools[0].function.name == "execute" })
    );
}

async fn wait_for_mode_approval(
    store: &Arc<InMemoryStore>,
    session_id: Uuid,
    turn_id: Uuid,
) -> Value {
    for _ in 0..500 {
        if let Some(event) = store
            .events_after(session_id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|event| event.event_type == "approval")
        {
            return event.payload_json;
        }
        let turn = store.turn(turn_id).await.unwrap();
        if turn.status == "failed" {
            let events = store.events_after(session_id, None).await.unwrap();
            panic!(
                "mode suggestion turn failed before approval: {}; events: {events:#?}",
                turn.error.as_deref().unwrap_or("unknown turn error")
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let events = store.events_after(session_id, None).await.unwrap();
    panic!("mode approval was not persisted; events: {events:#?}")
}

#[tokio::test]
async fn negative_state_grounding_chat_does_not_write_memory_unsolicited() {
    // Distress messages now stay in `general` mode (which has personal-assistant-state-capture
    // active) instead of routing into a capture-free posture mode. The transient-mood guard in
    // state_capture.rs is the only remaining safeguard against turning a bad moment into a
    // memory write, so this is the regression test for that guard.
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    post_user_message(
        &app,
        session.id,
        "I'm useless, stuck, and too exhausted to keep going",
    )
    .await;

    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, ModeId::from("general"));
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "write_proposal"),
        "negative-state chat should not emit memory write proposals"
    );
    assert!(store.profile_facts("brian").await.unwrap().is_empty());
    assert!(
        store
            .recall_chunks("global", "useless", 5)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn unlock_removes_the_lock_but_never_reverts_the_mode() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    let lock = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/lock", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mode":"serious_engineer","reason":"stay in engineering"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lock.status(), StatusCode::OK);
    let lock_json = response_json(lock).await;
    assert_eq!(lock_json["modeState"]["mode"], json!("serious_engineer"));
    assert_eq!(lock_json["modeState"]["lockSource"], json!("user"));

    post_user_message(
        &app,
        session.id,
        "help me plan tomorrow and clean up reminders",
    )
    .await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, ModeId::from("serious_engineer"));
    assert_eq!(latest.mode_state.lock_source.as_deref(), Some("user"));

    let unlock = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/unlock", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"reason":"no longer need the lock"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unlock.status(), StatusCode::OK);
    let unlock_json = response_json(unlock).await;
    assert_eq!(unlock_json["modeState"]["lockSource"], Value::Null);
    assert_eq!(
        unlock_json["modeState"]["mode"],
        json!("serious_engineer"),
        "unlocking removes the lock but must not itself change or revert the mode"
    );

    // Sticky: a plain planning message still doesn't move it, locked or not.
    post_user_message(
        &app,
        session.id,
        "help me plan tomorrow and clean up reminders",
    )
    .await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, ModeId::from("serious_engineer"));
    assert_eq!(latest.mode_state.lock_source, None);

    // Returning to General is a deliberate user action (the picker/override), not automatic.
    let override_res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/override", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mode":"general","reason":"done with the code change"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(override_res.status(), StatusCode::OK);
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, ModeId::from("general"));
}

#[tokio::test]
async fn user_override_can_switch_to_serious_engineer_through_lock() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    let lock = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/lock", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"mode":"general","reason":"stay light"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lock.status(), StatusCode::OK);

    let override_res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/override", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mode":"serious_engineer","reason":"user override"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(override_res.status(), StatusCode::OK);
    let override_json = response_json(override_res).await;
    assert_eq!(
        override_json["modeState"]["mode"],
        json!("serious_engineer")
    );
    assert_eq!(override_json["modeState"]["lockSource"], Value::Null);
    assert_eq!(override_json["modeState"]["overrideSource"], json!("user"));

    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, ModeId::from("serious_engineer"));
    assert_eq!(latest.mode_state.lock_source, None);
    assert_eq!(latest.mode_state.override_source.as_deref(), Some("user"));

    post_user_message(
        &app,
        session.id,
        "help me plan tomorrow and clean up reminders",
    )
    .await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, ModeId::from("serious_engineer"));
    assert_eq!(latest.mode_state.override_source.as_deref(), Some("user"));
}

#[tokio::test]
async fn mode_events_only_come_from_explicit_actions_not_from_messages() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    post_user_message(&app, session.id, "please fix this Rust code bug").await;
    let events = store.events_after(session.id, None).await.unwrap();
    let mode_events = events
        .iter()
        .filter(|event| event.event_type == "mode")
        .collect::<Vec<_>>();
    assert_eq!(
        mode_events.len(),
        1,
        "no mode event should be emitted just from posting a message"
    );
    let seq_before_override = events.last().map(|event| event.seq).unwrap_or(0);

    let override_res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/override", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mode":"serious_engineer","reason":"user picked it"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(override_res.status(), StatusCode::OK);

    let events = store.events_after(session.id, None).await.unwrap();
    let mode_events = events
        .iter()
        .filter(|event| event.event_type == "mode")
        .collect::<Vec<_>>();
    assert_eq!(mode_events.len(), 2);
    assert_eq!(mode_events[1].payload_json["event"], json!("mode_changed"));
    assert_eq!(
        mode_events[1].payload_json["mode"],
        json!("serious_engineer")
    );
    assert_eq!(
        mode_events[1].payload_json["activeSkills"],
        json!(["serious-engineer-ops"])
    );
    assert!(
        mode_events[1].payload_json["router_reason"]
            .as_str()
            .unwrap()
            .contains("user picked it")
    );

    let replay = store
        .events_after(session.id, Some(seq_before_override))
        .await
        .unwrap();
    assert_eq!(replay.len(), 1, "only the override's mode event is new");
    assert_eq!(replay[0].event_type, "mode");
    assert_eq!(replay[0].payload_json["mode"], json!("serious_engineer"));

    let lock = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/lock", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"mode":"general","reason":"stay light"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lock.status(), StatusCode::OK);
    let lock_json = response_json(lock).await;
    assert_eq!(lock_json["modeState"]["lockSource"], json!("user"));
    assert_eq!(
        lock_json["activeSkills"],
        json!(["miku-voice", "personal-assistant-state-capture"])
    );

    post_user_message(&app, session.id, "fix another code bug").await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(
        latest.mode_state.mode,
        ModeId::from("general"),
        "locked mode must not change even for a code-flavored message"
    );

    let unlock = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/unlock", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"reason":"no longer need the lock"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unlock.status(), StatusCode::OK);

    post_user_message(&app, session.id, "fix the Rust code now").await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(
        latest.mode_state.mode,
        ModeId::from("general"),
        "unlocking must not itself trigger a switch; messages never auto-switch anymore"
    );
}

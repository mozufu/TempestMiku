use super::*;

#[tokio::test]
async fn router_defaults_unlocked_non_coding_prompts_to_personal_assistant() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    post_user_message(&app, session.id, "please fix this Rust code bug").await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, Mode::SeriousEngineer);
    assert_eq!(latest.mode_state.voice_cap(), "off");

    post_user_message(
        &app,
        session.id,
        "help me plan tomorrow and clean up reminders",
    )
    .await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, Mode::PersonalAssistant);
    assert_eq!(latest.mode_state.voice_cap(), "medium");
    assert_eq!(latest.mode_state.lock_source, None);
    assert_eq!(latest.mode_state.override_source, None);
    assert!(
        latest
            .mode_state
            .router_reason
            .as_deref()
            .unwrap_or_default()
            .contains("default personal-assistant")
    );

    let events = store.events_after(session.id, None).await.unwrap();
    let mode_events = events
        .iter()
        .filter(|event| event.event_type == "mode")
        .collect::<Vec<_>>();
    let last_mode = mode_events.last().unwrap();
    assert_eq!(last_mode.payload_json["from"], json!("serious_engineer"));
    assert_eq!(last_mode.payload_json["mode"], json!("personal_assistant"));
    assert_eq!(last_mode.payload_json["voice_cap"], json!("medium"));
    assert_eq!(
        last_mode.payload_json["activeSkills"],
        json!(["miku-voice", "personal-assistant-state-capture"])
    );
}

#[test]
fn router_triggers_negative_state_grounding_for_negative_language() {
    for (content, trigger) in [
        (
            "everything is too much and I am overwhelmed",
            "overwhelmed language",
        ),
        ("I'm exhausted and have no energy", "exhausted language"),
        (
            "I'm useless and nothing I do counts",
            "self-deprecating language",
        ),
        ("I am spiraling tonight", "spiraling language"),
        (
            "I'm stuck on this Rust bug and can't make progress",
            "stuck language",
        ),
    ] {
        let (mode, reason) = route_mode_for_prompt(content);
        assert_eq!(mode, Mode::NegativeStateGrounding, "{content}");
        assert!(
            reason.contains(trigger),
            "expected {trigger:?} in reason {reason:?}"
        );
    }
}

#[tokio::test]
async fn negative_state_grounding_chat_does_not_write_memory_unsolicited() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    post_user_message(
        &app,
        session.id,
        "I'm useless, stuck, and too exhausted to keep going",
    )
    .await;

    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, Mode::NegativeStateGrounding);
    assert_eq!(latest.mode_state.mode.capability_class(), "conversation");
    assert_eq!(latest.mode_state.voice_cap(), "high");
    assert_eq!(
        latest.mode_state.mode.active_skill_names(),
        ["miku-voice", "negative-state-grounding"]
    );
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
async fn user_lock_keeps_serious_engineer_until_unlock_reenables_default_route() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
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
    assert_eq!(lock_json["voiceCap"], json!("off"));

    post_user_message(
        &app,
        session.id,
        "help me plan tomorrow and clean up reminders",
    )
    .await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, Mode::SeriousEngineer);
    assert_eq!(latest.mode_state.lock_source.as_deref(), Some("user"));

    let unlock = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/unlock", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"reason":"router may choose again"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unlock.status(), StatusCode::OK);

    post_user_message(
        &app,
        session.id,
        "help me plan tomorrow and clean up reminders",
    )
    .await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, Mode::PersonalAssistant);
    assert_eq!(latest.mode_state.lock_source, None);
    assert!(
        latest
            .mode_state
            .router_reason
            .as_deref()
            .unwrap_or_default()
            .contains("default personal-assistant")
    );
}

#[tokio::test]
async fn user_override_can_switch_to_serious_engineer_through_lock() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    let lock = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/lock", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mode":"personal_assistant","reason":"stay light"}"#,
                ))
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
    assert_eq!(override_json["voiceCap"], json!("off"));

    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, Mode::SeriousEngineer);
    assert_eq!(latest.mode_state.lock_source, None);
    assert_eq!(latest.mode_state.override_source.as_deref(), Some("user"));

    post_user_message(
        &app,
        session.id,
        "help me plan tomorrow and clean up reminders",
    )
    .await;
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, Mode::SeriousEngineer);
    assert_eq!(latest.mode_state.override_source.as_deref(), Some("user"));
}

#[tokio::test]
async fn router_lock_unlock_and_replay_mode_events() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"content":"please fix this Rust code bug"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
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
    assert_eq!(mode_events[1].payload_json["voice_cap"], json!("off"));
    assert_eq!(mode_events[1].payload_json["activeSkills"], json!([]));
    assert!(
        mode_events[1].payload_json["router_reason"]
            .as_str()
            .unwrap()
            .contains("coding")
    );

    let replay = store.events_after(session.id, Some(1)).await.unwrap();
    assert_eq!(replay[0].event_type, "mode");
    assert_eq!(replay[0].payload_json["mode"], json!("serious_engineer"));

    let lock = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/lock", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mode":"personal_assistant","reason":"stay light"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lock.status(), StatusCode::OK);
    let lock_json = response_json(lock).await;
    assert_eq!(lock_json["modeState"]["lockSource"], json!("user"));
    assert_eq!(lock_json["voiceCap"], json!("medium"));
    assert_eq!(
        lock_json["activeSkills"],
        json!(["miku-voice", "personal-assistant-state-capture"])
    );

    let locked_message = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"content":"fix another code bug"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(locked_message.status(), StatusCode::OK);
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, Mode::PersonalAssistant);

    let unlock = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/mode/unlock", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"reason":"router may resume"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unlock.status(), StatusCode::OK);

    let reroute = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"content":"fix the Rust code now"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reroute.status(), StatusCode::OK);
    let latest = store.get_session(session.id).await.unwrap();
    assert_eq!(latest.mode_state.mode, Mode::SeriousEngineer);
    assert_eq!(latest.mode_state.mode.voice_cap(), "off");
}

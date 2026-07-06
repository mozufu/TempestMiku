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
    // model-proposed and user-confirmed `mode_suggest`. Posting a message alone must never
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
    assert_eq!(lock_json["voiceCap"], json!("off"));

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
    assert_eq!(override_json["voiceCap"], json!("off"));

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
    assert_eq!(mode_events[1].payload_json["voice_cap"], json!("off"));
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
    assert_eq!(
        latest.mode_state.mode,
        ModeId::from("general"),
        "unlocking must not itself trigger a switch; messages never auto-switch anymore"
    );
}

use super::*;

fn moderate_state() -> AppState<InMemoryStore, StoreMemoryProvider<InMemoryStore>, EchoChatRunner> {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    AppState::new(
        store,
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_self_evolution_tier(tm_host::SelfEvolutionTier::Moderate)
}

fn moderate_state_with_mode_addenda(
    root: &std::path::Path,
) -> AppState<InMemoryStore, StoreMemoryProvider<InMemoryStore>, EchoChatRunner> {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    AppState::new(
        store,
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default().with_managed_mode_addenda_path(root),
        AuthConfig::NoAuth,
    )
    .with_self_evolution_tier(tm_host::SelfEvolutionTier::Moderate)
}

fn moderate_state_with_persona_addenda(
    root: &std::path::Path,
) -> AppState<InMemoryStore, StoreMemoryProvider<InMemoryStore>, EchoChatRunner> {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    AppState::new(
        store,
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default().with_managed_persona_addenda_path(root),
        AuthConfig::NoAuth,
    )
    .with_self_evolution_tier(tm_host::SelfEvolutionTier::Moderate)
}

fn mode_review_request(timeout_ms: u64) -> Value {
    json!({
        "target": { "kind": "mode", "modeId": "serious_engineer" },
        "changes": [{
            "section": "description",
            "before": {
                "label": "Current description",
                "summary": "Coding mode handles focused engineering tasks."
            },
            "after": {
                "label": "Reviewable description addendum",
                "summary": "Prefer explicit verification evidence when closing roadmap gates."
            }
        }],
        "timeoutMs": timeout_ms
    })
}

fn persona_review_request(timeout_ms: u64) -> Value {
    json!({
        "target": { "kind": "persona", "personaId": "miku" },
        "changes": [
            {
                "section": "tone_guidance",
                "before": null,
                "after": {
                    "label": "Tone preference",
                    "summary": "Keep routine status updates concise and direct."
                }
            },
            {
                "section": "address_guidance",
                "before": null,
                "after": {
                    "label": "Address preference",
                    "summary": "Use Brian when a name makes the reply clearer."
                }
            },
            {
                "section": "interaction_preference",
                "before": null,
                "after": {
                    "label": "Interaction preference",
                    "summary": "Lead with the verified outcome before implementation detail."
                }
            }
        ],
        "timeoutMs": timeout_ms
    })
}

async fn post_review_proposal(
    app: &Router,
    session_id: Uuid,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/evolution/review-proposals"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn moderate_review_approval_updates_only_durable_proposal_and_replays() {
    let state = moderate_state();
    let store = Arc::clone(&state.store);
    let before_assets = state.persona.load_assets();
    let (app, _) = test_app_with_state(state);
    let session = create(&app).await;

    let response = post_review_proposal(&app, session.id, mode_review_request(5_000)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["status"], json!("pending"));
    assert_eq!(response["applyEnabled"], json!(false));
    let proposal_id = response["proposalId"].as_str().unwrap().parse().unwrap();
    let approval_id = response["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();

    let pending = store.evolution_review_proposal(proposal_id).await.unwrap();
    assert_eq!(pending.status, tm_modes::ReviewProposalStatus::Pending);
    assert_eq!(
        pending.apply_contract,
        tm_modes::ReviewApplyContract::Disabled
    );
    let pending_event = wait_for_event_payload(&store, session.id, "write_proposal").await;
    assert_eq!(pending_event["kind"], json!("evolution_review"));
    assert_eq!(pending_event["applyEnabled"], json!(false));
    assert!(pending_event.get("changes").is_none());
    assert!(pending_event["preview"].as_str().unwrap().len() <= 512);
    let approval = wait_for_event_payload(&store, session.id, "approval").await;
    assert_eq!(approval["backend"], json!("evolution-review"));
    assert_eq!(approval["scope"]["applyEnabled"], json!(false));
    assert_eq!(approval["scope"]["uri"], response["resourceUri"]);

    let resource = get_session_resource_json(
        &app,
        session.id,
        "resolve",
        response["resourceUri"].as_str().unwrap(),
    )
    .await;
    let content = resource["content"].as_str().unwrap();
    assert!(content.contains("Prefer explicit verification evidence"));
    assert!(content.contains("\"applyContract\": \"disabled\""));

    resolve_test_approval(&app, session.id, approval_id, "approve").await;
    let approved = store.evolution_review_proposal(proposal_id).await.unwrap();
    assert_eq!(approved.status, tm_modes::ReviewProposalStatus::Approved);
    assert_eq!(
        approved.apply_contract,
        tm_modes::ReviewApplyContract::Disabled
    );
    let after_assets = ModesConfig::default().load_assets();
    assert_eq!(after_assets.soul, before_assets.soul);
    assert_eq!(after_assets.modes, before_assets.modes);

    let events = store.events_after(session.id, None).await.unwrap();
    let statuses = events
        .iter()
        .filter(|event| event.event_type == "write_proposal")
        .map(|event| event.payload_json["status"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(statuses, vec!["pending", "approved"]);
    assert!(events.iter().any(|event| {
        event.event_type == "approval_resolved" && event.payload_json["status"] == json!("approved")
    }));
    let audits = store.evolution_audits(session.id).await.unwrap();
    assert!(
        audits
            .iter()
            .any(|audit| audit.status == tm_host::EvolutionAuditStatus::Applied)
    );
    assert!(
        audits
            .iter()
            .all(|audit| audit.target.id.as_str() == proposal_id.to_string())
    );
}

#[tokio::test]
async fn approved_mode_addendum_composes_without_widening_authority_and_rolls_back() {
    let root = tempfile::tempdir().unwrap();
    let state = moderate_state_with_mode_addenda(root.path());
    let store = Arc::clone(&state.store);
    let persona = state.persona.clone();
    let mode_id = tm_modes::ModeId::new("serious_engineer");
    let base_profile = persona.load_assets().profile_or_unknown(&mode_id);
    let (app, _) = test_app_with_state(state);
    let session = create(&app).await;

    let response = post_review_proposal(&app, session.id, mode_review_request(5_000)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["applyEnabled"], json!(true));
    let proposal_id = response["proposalId"].as_str().unwrap().parse().unwrap();
    let approval_id = response["approvalId"].as_str().unwrap().parse().unwrap();
    let pending = store.evolution_review_proposal(proposal_id).await.unwrap();
    assert_eq!(
        pending.apply_contract,
        tm_modes::ReviewApplyContract::VersionedModeAddendum
    );

    resolve_test_approval(&app, session.id, approval_id, "approve").await;
    let managed = persona.managed_mode_addendum(&mode_id).unwrap();
    let active = managed.active.unwrap();
    assert_eq!(active.content_digest, pending.content_digest);
    let prompt = persona.build_system_prompt(&mode_id, "base", "", "close the gate");
    assert!(
        prompt
            .system_prompt
            .contains("Prefer explicit verification evidence")
    );
    assert_eq!(prompt.profile, base_profile);
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().any(|event| {
        event.event_type == "write_proposal"
            && event.payload_json["proposalId"] == json!(proposal_id)
            && event.payload_json["activation"]["active"]["contentDigest"]
                == json!(pending.content_digest)
    }));

    let rollback = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/sessions/{}/evolution/modes/serious_engineer/rollback",
                    session.id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "expectedActiveDigest": pending.content_digest,
                        "targetDigest": null,
                        "timeoutMs": 5_000
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rollback.status(), StatusCode::OK);
    let rollback = response_json(rollback).await;
    let rollback_approval_id = rollback["approvalId"].as_str().unwrap().parse().unwrap();
    resolve_test_approval(&app, session.id, rollback_approval_id, "approve").await;
    assert!(
        persona
            .managed_mode_addendum(&mode_id)
            .unwrap()
            .active
            .is_none()
    );
    assert!(
        !persona
            .build_system_prompt(&mode_id, "base", "", "close the gate")
            .system_prompt
            .contains("Approved mode addendum")
    );
    assert_eq!(
        persona.load_assets().profile_or_unknown(&mode_id),
        base_profile
    );
}

#[tokio::test]
async fn approved_persona_addendum_composes_across_modes_and_rolls_back_to_base() {
    let root = tempfile::tempdir().unwrap();
    let state = moderate_state_with_persona_addenda(root.path());
    let store = Arc::clone(&state.store);
    let persona = state.persona.clone();
    let before_assets = persona.load_assets();
    let general = tm_modes::ModeId::new("general");
    let serious = tm_modes::ModeId::new("serious_engineer");
    let general_profile = before_assets.profile_or_unknown(&general);
    let serious_profile = before_assets.profile_or_unknown(&serious);
    let (app, _) = test_app_with_state(state);
    let session = create(&app).await;

    let response = post_review_proposal(&app, session.id, persona_review_request(5_000)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["applyEnabled"], json!(true));
    let proposal_id = response["proposalId"].as_str().unwrap().parse().unwrap();
    let approval_id = response["approvalId"].as_str().unwrap().parse().unwrap();
    let pending = store.evolution_review_proposal(proposal_id).await.unwrap();
    assert_eq!(
        pending.apply_contract,
        tm_modes::ReviewApplyContract::VersionedPersonaAddendum
    );

    resolve_test_approval(&app, session.id, approval_id, "approve").await;
    let managed = persona.managed_persona_addendum("miku").unwrap();
    let active = managed.active.unwrap();
    assert_eq!(active.content_digest, pending.content_digest);
    for (mode, profile) in [
        (general.clone(), general_profile),
        (serious, serious_profile),
    ] {
        let prompt = persona.build_system_prompt(&mode, "base", "", "report status");
        assert!(prompt.system_prompt.contains("Approved persona addendum"));
        assert!(prompt.system_prompt.contains("Use Brian"));
        assert_eq!(prompt.profile, profile);
    }
    let after_assets = persona.load_assets();
    assert_eq!(after_assets.soul, before_assets.soul);
    assert_eq!(after_assets.modes, before_assets.modes);

    let rollback = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/sessions/{}/evolution/personas/miku/rollback",
                    session.id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "expectedActiveDigest": pending.content_digest,
                        "targetDigest": null,
                        "timeoutMs": 5_000
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rollback.status(), StatusCode::OK);
    let rollback = response_json(rollback).await;
    let rollback_approval_id = rollback["approvalId"].as_str().unwrap().parse().unwrap();
    resolve_test_approval(&app, session.id, rollback_approval_id, "approve").await;
    assert!(
        persona
            .managed_persona_addendum("miku")
            .unwrap()
            .active
            .is_none()
    );
    assert!(
        !persona
            .build_system_prompt(&general, "base", "", "report status")
            .system_prompt
            .contains("Approved persona addendum")
    );
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().any(|event| {
        event.event_type == "write_proposal"
            && event.payload_json["kind"] == json!("persona_addendum_rollback")
            && event.payload_json["status"] == json!("approved")
    }));
}

#[tokio::test]
async fn legacy_persona_review_sections_remain_non_activatable() {
    let root = tempfile::tempdir().unwrap();
    let state = moderate_state_with_persona_addenda(root.path());
    let store = Arc::clone(&state.store);
    let (app, _) = test_app_with_state(state);
    let session = create(&app).await;
    let response = post_review_proposal(
        &app,
        session.id,
        json!({
            "target": { "kind": "persona", "personaId": "miku" },
            "changes": [{
                "section": "voice_guidance",
                "before": null,
                "after": { "label": "Legacy", "summary": "Review only." }
            }],
            "timeoutMs": 5_000
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["applyEnabled"], json!(false));
    let proposal_id = response["proposalId"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        store
            .evolution_review_proposal(proposal_id)
            .await
            .unwrap()
            .apply_contract,
        tm_modes::ReviewApplyContract::Disabled
    );
}

#[tokio::test]
async fn conservative_review_attempt_is_denied_before_proposal_or_approval() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let response = post_review_proposal(&app, session.id, mode_review_request(5_000)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        response_json(response).await["error"]
            .as_str()
            .unwrap()
            .contains("evolution_insufficient_tier")
    );
    assert!(
        store
            .evolution_review_proposals_for_session(session.id)
            .await
            .unwrap()
            .is_empty()
    );
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .all(|event| { !matches!(event.event_type.as_str(), "write_proposal" | "approval") })
    );
    let audits = store.evolution_audits(session.id).await.unwrap();
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].status, tm_host::EvolutionAuditStatus::Denied);
}

#[tokio::test]
async fn moderate_review_deny_and_timeout_are_durable_no_apply_outcomes() {
    for (persona_target, decision, expected) in [
        (false, Some("deny"), tm_modes::ReviewProposalStatus::Denied),
        (false, None, tm_modes::ReviewProposalStatus::TimedOut),
        (true, Some("deny"), tm_modes::ReviewProposalStatus::Denied),
        (true, None, tm_modes::ReviewProposalStatus::TimedOut),
    ] {
        let root = tempfile::tempdir().unwrap();
        let state = if persona_target {
            moderate_state_with_persona_addenda(root.path())
        } else {
            moderate_state_with_mode_addenda(root.path())
        };
        let store = Arc::clone(&state.store);
        let persona = state.persona.clone();
        let (app, _) = test_app_with_state(state);
        let session = create(&app).await;
        let timeout_ms = if decision.is_some() { 5_000 } else { 25 };
        let request = if persona_target {
            persona_review_request(timeout_ms)
        } else {
            mode_review_request(timeout_ms)
        };
        let response = post_review_proposal(&app, session.id, request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = response_json(response).await;
        let proposal_id = response["proposalId"].as_str().unwrap().parse().unwrap();
        let approval_id = response["approvalId"].as_str().unwrap().parse().unwrap();

        if let Some(decision) = decision {
            resolve_test_approval(&app, session.id, approval_id, decision).await;
        } else {
            let events = store
                .expire_pending_approvals(Utc::now() + chrono::Duration::seconds(1))
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            let lease = store
                .claim_approval_effect(
                    approval_id,
                    Uuid::new_v4(),
                    Utc::now() + chrono::Duration::seconds(1),
                    chrono::Duration::seconds(30),
                )
                .await
                .unwrap()
                .unwrap();
            let approval = store
                .approval_request(session.id, approval_id)
                .await
                .unwrap();
            crate::api::approvals::apply_approval_effect_lease(
                store.as_ref(),
                &approval,
                &lease,
                tm_host::SelfEvolutionTier::Moderate,
                &persona,
                Arc::new(crate::StoreCodingEventSink::new(
                    session.id,
                    Arc::clone(&store),
                    tokio::sync::broadcast::channel(8).0,
                )),
            )
            .await
            .unwrap();
        }
        let proposal = store.evolution_review_proposal(proposal_id).await.unwrap();
        assert_eq!(proposal.status, expected);
        assert_eq!(
            proposal.apply_contract,
            if persona_target {
                tm_modes::ReviewApplyContract::VersionedPersonaAddendum
            } else {
                tm_modes::ReviewApplyContract::VersionedModeAddendum
            }
        );
        if persona_target {
            assert!(
                persona
                    .managed_persona_addendum("miku")
                    .unwrap()
                    .active
                    .is_none()
            );
        } else {
            assert!(
                persona
                    .managed_mode_addendum(&tm_modes::ModeId::new("serious_engineer"))
                    .unwrap()
                    .active
                    .is_none()
            );
        }
        let events = store.events_after(session.id, None).await.unwrap();
        assert!(events.iter().any(|event| {
            event.event_type == "write_proposal"
                && event.payload_json["status"] == json!(expected.as_str())
                && event.payload_json["applyEnabled"] == json!(true)
        }));
        let expected_audit = match expected {
            tm_modes::ReviewProposalStatus::Denied => tm_host::EvolutionAuditStatus::Denied,
            tm_modes::ReviewProposalStatus::TimedOut => tm_host::EvolutionAuditStatus::TimedOut,
            _ => unreachable!(),
        };
        assert!(
            store
                .evolution_audits(session.id)
                .await
                .unwrap()
                .iter()
                .any(|audit| audit.status == expected_audit)
        );
    }
}

#[tokio::test]
async fn approved_review_fails_closed_when_the_live_base_changes() {
    let temp = tempfile::tempdir().unwrap();
    write_mode_assets_fixture(temp.path());
    let persona = ModesConfig::from_path(temp.path());
    let store = Arc::new(InMemoryStore::default());
    let state = AppState::new(
        Arc::clone(&store),
        Arc::new(StoreMemoryProvider::new(Arc::clone(&store))),
        Arc::new(EchoChatRunner),
        persona,
        AuthConfig::NoAuth,
    )
    .with_self_evolution_tier(tm_host::SelfEvolutionTier::Moderate);
    let (app, _) = test_app_with_state(state);
    let session = create(&app).await;
    let mut request = mode_review_request(5_000);
    request["target"]["modeId"] = json!("general");
    let response = post_review_proposal(&app, session.id, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    let proposal_id = response["proposalId"].as_str().unwrap().parse().unwrap();
    let approval_id = response["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();

    let mode_path = temp.path().join("modes.json");
    let mut catalog: Value =
        serde_json::from_str(&std::fs::read_to_string(&mode_path).unwrap()).unwrap();
    catalog["modes"][0]["description"] = json!("Base changed after proposal creation.");
    std::fs::write(&mode_path, serde_json::to_vec_pretty(&catalog).unwrap()).unwrap();

    let resolution = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/approvals/{approval_id}", session.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "decision": "approve" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resolution.status(), StatusCode::FORBIDDEN);
    assert!(
        response_json(resolution).await["error"]
            .as_str()
            .unwrap()
            .contains("evolution_stale_approval")
    );
    let proposal = store.evolution_review_proposal(proposal_id).await.unwrap();
    assert_eq!(proposal.status, tm_modes::ReviewProposalStatus::Pending);
    assert_eq!(
        proposal.apply_contract,
        tm_modes::ReviewApplyContract::Disabled
    );
    assert!(
        store
            .evolution_audits(session.id)
            .await
            .unwrap()
            .iter()
            .any(|audit| {
                audit.status == tm_host::EvolutionAuditStatus::Failed
                    && audit.error_code == Some(tm_host::EvolutionPolicyReason::StaleApproval)
            })
    );
}

#[tokio::test]
async fn evolution_audit_resource_represents_every_terminal_query_status() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let template = crate::evolution::denied_evolution_audit_record(
        crate::evolution::DeniedEvolutionAuditSpec {
            tier: tm_host::SelfEvolutionTier::Conservative,
            target_class: tm_host::EvolutionTargetClass::ModeProposal,
            target_id: Uuid::new_v4().to_string(),
            actor_id: "audit-query-test".to_string(),
            session_id: session.id,
            dream_id: None,
            content: &json!({ "bounded": true }),
            occurred_at: Utc::now(),
        },
    )
    .unwrap();
    let statuses = [
        tm_host::EvolutionAuditStatus::Attempted,
        tm_host::EvolutionAuditStatus::Denied,
        tm_host::EvolutionAuditStatus::AwaitingApproval,
        tm_host::EvolutionAuditStatus::Approved,
        tm_host::EvolutionAuditStatus::TimedOut,
        tm_host::EvolutionAuditStatus::Superseded,
        tm_host::EvolutionAuditStatus::Failed,
        tm_host::EvolutionAuditStatus::Applied,
    ];
    for status in statuses {
        let mut record = template.clone();
        record.id = Uuid::new_v4();
        record.status = status;
        store
            .append_evolution_audit(crate::EvolutionAuditEntry {
                idempotency_key: format!("audit-query:{status:?}"),
                record,
            })
            .await
            .unwrap();
    }
    let resource =
        get_session_resource_json(&app, session.id, "resolve", "memory://evolution-audits").await;
    let content = resource["content"].as_str().unwrap();
    for status in [
        "attempted",
        "denied",
        "awaiting_approval",
        "approved",
        "timed_out",
        "superseded",
        "failed",
        "applied",
    ] {
        assert!(content.contains(&format!("\"status\": \"{status}\"")));
    }
}

#[tokio::test]
async fn gated_postgres_review_migrates_restarts_and_resolves_once() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = Arc::new(PostgresStore::connect(&dsn).await.unwrap());
    store.configure_owner_subject("brian").await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let persona = ModesConfig::default().with_managed_mode_addenda_path(root.path());
    let state = AppState::new(
        Arc::clone(&store),
        Arc::new(StoreMemoryProvider::new(Arc::clone(&store))),
        Arc::new(EchoChatRunner),
        persona.clone(),
        AuthConfig::NoAuth,
    )
    .with_self_evolution_tier(tm_host::SelfEvolutionTier::Moderate);
    let app = app(state);
    let session = create(&app).await;
    let response = post_review_proposal(&app, session.id, mode_review_request(5_000)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    let proposal_id = response["proposalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let approval_id = response["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();

    let resolve = |app: Router| async move {
        app.oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/approvals/{approval_id}", session.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "decision": "approve" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
    };
    let (left, right) = tokio::join!(resolve(app.clone()), resolve(app.clone()));
    assert!(matches!(
        (left, right),
        (StatusCode::OK, StatusCode::CONFLICT) | (StatusCode::CONFLICT, StatusCode::OK)
    ));

    let restarted = PostgresStore::connect(&dsn).await.unwrap();
    let proposal = restarted
        .evolution_review_proposal(proposal_id)
        .await
        .unwrap();
    assert_eq!(proposal.status, tm_modes::ReviewProposalStatus::Approved);
    assert_eq!(
        proposal.apply_contract,
        tm_modes::ReviewApplyContract::VersionedModeAddendum
    );
    let reloaded = ModesConfig::default().with_managed_mode_addenda_path(root.path());
    assert_eq!(
        reloaded
            .managed_mode_addendum(&tm_modes::ModeId::new("serious_engineer"))
            .unwrap()
            .active
            .unwrap()
            .content_digest,
        proposal.content_digest
    );
    let row_count: i64 = restarted
        .client()
        .query_one(
            "select count(*) from evolution_review_proposals where id = $1",
            &[&proposal_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(row_count, 1);
    let events = restarted.events_after(session.id, None).await.unwrap();
    let statuses = events
        .iter()
        .filter(|event| {
            event.event_type == "write_proposal"
                && event.payload_json["proposalId"] == json!(proposal_id)
        })
        .map(|event| event.payload_json["status"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(statuses, vec!["pending", "approved"]);
    let audits = restarted.evolution_audits(session.id).await.unwrap();
    assert!(
        audits
            .iter()
            .any(|audit| audit.status == tm_host::EvolutionAuditStatus::Applied)
    );
}

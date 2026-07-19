use super::*;
use tm_memory::{
    EpisodicMemoryRecord, HybridMemoryCandidate, MemoryRecordEvidence, MemoryRecordResource,
    MemoryRecordStatus, StoredMemoryRecord,
};

#[derive(Clone)]
struct PersonaEvidenceProvider {
    context: MemoryContext,
}

#[async_trait]
impl MemoryProvider for PersonaEvidenceProvider {
    async fn context_for_turn(
        &self,
        subject: &str,
        scope: &str,
        _query: &str,
    ) -> Result<MemoryContext> {
        let mut context = self.context.clone();
        context.subject = subject.to_string();
        context.scope = scope.to_string();
        Ok(context)
    }
}

fn evidence_context() -> MemoryContext {
    let now = Utc::now();
    let candidates = [
        "Brian prefers concise replies.",
        "The owner asked for shorter replies with necessary evidence.",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, text)| {
        let record =
            StoredMemoryRecord::new(MemoryRecordResource::Episodic(EpisodicMemoryRecord {
                schema_version: tm_memory::MEMORY_RECORD_SCHEMA_VERSION,
                id: Uuid::new_v4(),
                owner_subject: "owner".to_string(),
                memory_scope: "global".to_string(),
                text: text.to_string(),
                evidence: vec![MemoryRecordEvidence::resource(
                    format!("memory://fixtures/persona/{index}"),
                    "approved owner evidence",
                )],
                confidence: 0.95,
                importance: 0.8,
                observed_at: now,
                effective_from: now,
                effective_to: None,
                status: MemoryRecordStatus::Active,
                links: Default::default(),
                created_at: now,
            }))
            .unwrap();
        HybridMemoryCandidate {
            record,
            lexical_rank: Some(index as u32 + 1),
            lexical_score: Some(0.9 - index as f32 * 0.1),
            dense_rank: Some(index as u32 + 1),
            dense_score: Some(0.9 - index as f32 * 0.1),
            embedding_version: Some("persona-fixture-v1".to_string()),
            rrf_score: 0.03 - index as f32 * 0.001,
        }
    })
    .collect();
    MemoryContext::from_hybrid_candidates_with_summaries(
        "owner",
        "global",
        Vec::new(),
        candidates,
        1_600,
        Some("persona-fixture-v1".to_string()),
    )
}

async fn run_turn<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    client_message_id: &str,
) -> Uuid
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let worker_id = Uuid::new_v4();
    let turn = state
        .store
        .enqueue_turn(session_id, client_message_id, "請簡短一點")
        .await
        .unwrap();
    let claimed = state
        .store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, turn.id);
    execute_claimed_turn(state, worker_id, claimed, Duration::from_secs(1))
        .await
        .unwrap();
    turn.id
}

#[tokio::test]
async fn completed_auto_turn_enqueues_one_manual_persona_approval_and_never_applies() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let provider = Arc::new(PersonaEvidenceProvider {
        context: evidence_context(),
    });
    let persona = tm_modes::ModesConfig::default().with_managed_persona_addenda_path(root.path());
    let assets = persona.load_assets();
    let session = store
        .create_session(NewSession {
            mode: assets.modes.default_mode(),
            persona_status: assets.status,
        })
        .await
        .unwrap();
    let state = AppState::new(
        Arc::clone(&store),
        provider,
        Arc::new(EchoChatRunner),
        persona.clone(),
        AuthConfig::NoAuth,
    )
    .with_self_evolution_tier(tm_host::SelfEvolutionTier::Moderate)
    .with_auto_turn_dispatcher(false);

    let first_turn_id = run_turn(&state, session.id, "persona-auto-1").await;
    assert_eq!(store.turn(first_turn_id).await.unwrap().status, "completed");
    let proposals = store
        .evolution_review_proposals_for_session(session.id)
        .await
        .unwrap();
    assert_eq!(proposals.len(), 1);
    let proposal = &proposals[0];
    assert_eq!(proposal.status, tm_modes::ReviewProposalStatus::Pending);
    assert_eq!(
        proposal.apply_contract,
        tm_modes::ReviewApplyContract::VersionedPersonaAddendum
    );
    let candidate = proposal.auto_candidate.as_ref().unwrap();
    assert_eq!(candidate.source_turn_id, first_turn_id);
    assert_eq!(candidate.evidence.len(), 2);
    assert_eq!(
        candidate.trigger,
        crate::PersonaAutoCandidateTrigger::RepeatedPreference
    );
    assert!(
        persona
            .managed_persona_addendum("miku")
            .unwrap()
            .active
            .is_none()
    );

    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "approval")
            .count(),
        1
    );
    let proposal_event = events
        .iter()
        .find(|event| event.event_type == "write_proposal")
        .unwrap();
    assert_eq!(proposal_event.turn_id, Some(first_turn_id));
    assert_eq!(proposal_event.payload_json["source"], json!("auto_mode"));
    assert_eq!(proposal_event.payload_json["evidenceCount"], json!(2));

    run_turn(&state, session.id, "persona-auto-2").await;
    assert_eq!(
        store
            .evolution_review_proposals_for_session(session.id)
            .await
            .unwrap()
            .len(),
        1,
        "a pending semantic duplicate must not enqueue another approval"
    );
    assert!(
        persona
            .managed_persona_addendum("miku")
            .unwrap()
            .active
            .is_none()
    );
}

#[tokio::test]
async fn locked_mode_suppresses_auto_persona_detection() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let provider = Arc::new(PersonaEvidenceProvider {
        context: evidence_context(),
    });
    let persona = tm_modes::ModesConfig::default().with_managed_persona_addenda_path(root.path());
    let assets = persona.load_assets();
    let session = store
        .create_session(NewSession {
            mode: assets.modes.default_mode(),
            persona_status: assets.status,
        })
        .await
        .unwrap();
    let mut locked = session.mode_state.clone();
    locked.lock_source = Some("owner".to_string());
    store.set_mode_state(session.id, locked).await.unwrap();
    let state = AppState::new(
        Arc::clone(&store),
        provider,
        Arc::new(EchoChatRunner),
        persona,
        AuthConfig::NoAuth,
    )
    .with_self_evolution_tier(tm_host::SelfEvolutionTier::Moderate)
    .with_auto_turn_dispatcher(false);

    run_turn(&state, session.id, "persona-locked").await;
    assert!(
        store
            .evolution_review_proposals_for_session(session.id)
            .await
            .unwrap()
            .is_empty()
    );
}

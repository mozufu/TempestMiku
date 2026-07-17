use super::*;

mod claim_failure_store;

pub(super) use claim_failure_store::ClaimFailureStore;

pub(super) fn test_sender_factory() -> SenderFactory {
    let senders = Arc::new(Mutex::new(
        BTreeMap::<Uuid, broadcast::Sender<SessionEvent>>::new(),
    ));
    Arc::new(move |session_id| {
        let mut senders = senders.lock().expect("sender map lock");
        senders
            .entry(session_id)
            .or_insert_with(|| broadcast::channel(64).0)
            .clone()
    })
}

pub(super) struct FailingPublishSink;

#[async_trait]
impl crate::CodingEventSink for FailingPublishSink {
    async fn emit(&self, _event_type: &str, _payload_json: Value) -> Result<SessionEvent> {
        panic!("approval proposal finalization must publish an already-persisted event")
    }

    async fn publish_persisted(&self, _event: SessionEvent) -> Result<()> {
        Err(ServerError::Store(
            "simulated post-commit broadcast failure".to_string(),
        ))
    }
}

pub(super) struct PersistedOnlySink;

#[async_trait]
impl crate::CodingEventSink for PersistedOnlySink {
    async fn emit(&self, _event_type: &str, _payload_json: Value) -> Result<SessionEvent> {
        panic!("durable approval test helper must not append a second event")
    }
}

pub(super) async fn resolved_memory_effect(
    store: &InMemoryStore,
    session_id: Uuid,
    proposal: crate::MemoryWriteProposal,
    evolution: Value,
) -> (ApprovalRequestRecord, ApprovalEffectLease) {
    let now = Utc::now();
    let approval_id = Uuid::new_v4();
    store
        .create_approval_request(NewApprovalRequest {
            id: approval_id,
            session_id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "evolution-policy-test".to_string(),
            action: "memory.write".to_string(),
            scope_json: json!({"scope": "global"}),
            options_json: json!([]),
            effect_type: "memory_write".to_string(),
            effect_payload_json: json!({
                "evolution": evolution,
                "proposal": proposal,
            }),
            resumable: true,
            created_at: now,
            expires_at: now + Duration::minutes(5),
        })
        .await
        .unwrap();
    store
        .resolve_approval_request_with_event(
            session_id,
            approval_id,
            NewApprovalResolution {
                status: "approved".to_string(),
                selected_option_id: Some("allow".to_string()),
                resolution_json: json!({
                    "approvalId": approval_id,
                    "status": "approved",
                    "outcome": "approved",
                }),
                resolved_at: now + Duration::seconds(1),
            },
        )
        .await
        .unwrap();
    let lease = store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            now + Duration::seconds(1),
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("resolved memory effect");
    let approval = store
        .approval_request(session_id, approval_id)
        .await
        .unwrap();
    (approval, lease)
}

pub(super) async fn resolve_and_apply_durable_approval(
    store: &Arc<InMemoryStore>,
    broker: &Arc<ApprovalBroker>,
    sender_for: &SenderFactory,
    session_id: Uuid,
    approval_id: Uuid,
    request: ResolveApprovalRequest,
) {
    resolve_and_apply_durable_approval_with_persona(
        store,
        broker,
        sender_for,
        session_id,
        approval_id,
        request,
        &tm_modes::ModesConfig::default(),
    )
    .await;
}

pub(super) async fn resolve_and_apply_durable_approval_with_persona(
    store: &Arc<InMemoryStore>,
    broker: &Arc<ApprovalBroker>,
    sender_for: &SenderFactory,
    session_id: Uuid,
    approval_id: Uuid,
    request: ResolveApprovalRequest,
    persona: &tm_modes::ModesConfig,
) {
    let sink: Arc<dyn crate::CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session_id,
        Arc::clone(store),
        sender_for(session_id),
    ));
    broker
        .resolve_persisted(session_id, approval_id, request, Arc::clone(&sink))
        .await
        .unwrap();
    let lease = store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            Utc::now(),
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("resolved approval effect");
    let approval = store
        .approval_request(session_id, approval_id)
        .await
        .unwrap();
    crate::api::approvals::apply_approval_effect_lease(
        store.as_ref(),
        &approval,
        &lease,
        tm_host::SelfEvolutionTier::Conservative,
        persona,
        sink,
    )
    .await
    .unwrap();
}

pub(super) async fn wait_for_event_count(
    store: &InMemoryStore,
    session_id: Uuid,
    event_type: &str,
    count: usize,
) -> Vec<SessionEvent> {
    for _ in 0..100 {
        if event_type == "approval_resolved" {
            store.expire_pending_approvals(Utc::now()).await.unwrap();
            loop {
                let Some(lease) = store
                    .claim_next_approval_effect(Uuid::new_v4(), Utc::now(), Duration::seconds(30))
                    .await
                    .unwrap()
                else {
                    break;
                };
                let approval = store
                    .approval_request(lease.effect.session_id, lease.effect.approval_id)
                    .await
                    .unwrap();
                crate::api::approvals::apply_approval_effect_lease(
                    store,
                    &approval,
                    &lease,
                    tm_host::SelfEvolutionTier::Conservative,
                    &tm_modes::ModesConfig::default(),
                    Arc::new(PersistedOnlySink),
                )
                .await
                .unwrap();
            }
        }
        let events = store.events_after(session_id, None).await.unwrap();
        if events
            .iter()
            .filter(|event| event.event_type == event_type)
            .count()
            >= count
        {
            return events;
        }
        tokio::time::sleep(StdDuration::from_millis(5)).await;
    }
    panic!("timed out waiting for {count} {event_type} events");
}

pub(super) async fn wait_for_skill_status(
    store: &InMemoryStore,
    session_id: Uuid,
    status: SkillProposalStatus,
) -> SkillProposalRecord {
    for _ in 0..100 {
        let proposals = store.skill_proposals_for_session(session_id).await.unwrap();
        if let Some(proposal) = proposals
            .into_iter()
            .find(|proposal| proposal.status == status)
        {
            return proposal;
        }
        tokio::time::sleep(StdDuration::from_millis(5)).await;
    }
    panic!("timed out waiting for skill proposal status {status}");
}

pub(super) async fn wait_for_memory_write_status(
    store: &InMemoryStore,
    session_id: Uuid,
    status: MemoryWriteStatus,
) -> Value {
    for _ in 0..100 {
        let events = store.events_after(session_id, None).await.unwrap();
        if let Some(event) = events.into_iter().find(|event| {
            event.event_type == "write_proposal"
                && event.payload_json["kind"] == json!("memory")
                && event.payload_json["status"] == json!(status)
        }) {
            return event.payload_json;
        }
        tokio::time::sleep(StdDuration::from_millis(5)).await;
    }
    panic!("timed out waiting for memory proposal status {status:?}");
}

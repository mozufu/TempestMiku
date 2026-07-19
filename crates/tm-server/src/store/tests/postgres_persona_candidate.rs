use super::*;
use crate::{
    AutoEvolutionReviewDisposition, NewApprovalRequest, NewAutoEvolutionReviewBundle,
    NewEvolutionReviewProposal, PersonaAutoCandidate, PersonaAutoCandidateEvidence,
    PersonaAutoCandidateTrigger,
};
use tm_modes::{
    ReviewAddendumChange, ReviewAddendumSection, ReviewApplyContract, ReviewMetadata,
    ReviewProposalStatus, ReviewProposalTarget,
};

fn auto_proposal(session_id: Uuid, turn_id: Uuid, id: Uuid) -> NewEvolutionReviewProposal {
    let change = ReviewAddendumChange {
        section: ReviewAddendumSection::InteractionPreference,
        before: None,
        after: ReviewMetadata {
            label: "Concise routine replies".to_string(),
            summary: "Keep routine replies concise while preserving necessary evidence and safety details."
                .to_string(),
        },
    };
    NewEvolutionReviewProposal {
        id,
        session_id,
        target: ReviewProposalTarget::Persona {
            persona_id: "miku".to_string(),
        },
        base_version: 1,
        base_digest: format!("sha256:{}", "a".repeat(64)),
        base_active_digest: None,
        changes: vec![change],
        content_digest: format!("sha256:{}", "b".repeat(64)),
        apply_contract: ReviewApplyContract::VersionedPersonaAddendum,
        auto_candidate: Some(PersonaAutoCandidate {
            schema_version: crate::store::PERSONA_AUTO_CANDIDATE_SCHEMA_VERSION,
            trigger: PersonaAutoCandidateTrigger::RepeatedPreference,
            dedupe_key: format!("persona-auto:v1:sha256:{}", "c".repeat(64)),
            source_turn_id: turn_id,
            evidence: [
                (tm_memory::MemoryRecordKind::Episodic, "d"),
                (tm_memory::MemoryRecordKind::Semantic, "e"),
            ]
            .into_iter()
            .map(|(kind, seed)| {
                let record_id = Uuid::new_v4();
                PersonaAutoCandidateEvidence {
                    record_id,
                    kind,
                    source_uri: format!("memory://records/{}/{record_id}", kind.as_str()),
                    evidence: vec![format!("approved={seed}://fixture")],
                }
            })
            .collect(),
        }),
    }
}

fn auto_bundle(
    session_id: Uuid,
    turn_id: Uuid,
    proposal_id: Uuid,
    approval_id: Uuid,
) -> NewAutoEvolutionReviewBundle {
    let now = Utc::now();
    NewAutoEvolutionReviewBundle {
        proposal: auto_proposal(session_id, turn_id, proposal_id),
        approval: NewApprovalRequest {
            id: approval_id,
            session_id,
            turn_id: Some(turn_id),
            requester_id: Uuid::new_v4(),
            origin: "evolution-review".to_string(),
            action: "review persona addendum miku".to_string(),
            scope_json: json!({"proposalId": proposal_id}),
            options_json: json!([]),
            effect_type: "evolution_review".to_string(),
            effect_payload_json: json!({"proposalId": proposal_id}),
            resumable: true,
            created_at: now,
            expires_at: now + Duration::minutes(1),
        },
        proposal_payload_json: json!({
            "kind": "evolution_review",
            "proposalId": proposal_id,
            "status": "pending",
        }),
        approval_payload_json: json!({
            "approvalId": approval_id,
            "backend": "evolution-review",
        }),
        cooldown_since: now - Duration::days(7),
    }
}

#[tokio::test]
async fn gated_postgres_auto_persona_dedupe_and_cooldown_are_cross_instance_atomic() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    let other = PostgresStore::connect(&dsn).await.unwrap();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "persona auto-candidate postgres fixture".to_string(),
            },
        })
        .await
        .unwrap();
    let turn = store
        .enqueue_turn(
            session.id,
            &format!("persona-postgres-{}", Uuid::new_v4()),
            "請簡短一點",
        )
        .await
        .unwrap();
    let worker_id = Uuid::new_v4();
    let claimed = store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, turn.id);
    store
        .complete_turn(turn.id, worker_id, "好的。", Utc::now())
        .await
        .unwrap();

    let cutoff = Utc::now() - Duration::days(7);
    let (left, right) = tokio::join!(
        store.create_auto_evolution_review_proposal(
            auto_proposal(session.id, turn.id, Uuid::new_v4()),
            cutoff,
        ),
        other.create_auto_evolution_review_proposal(
            auto_proposal(session.id, turn.id, Uuid::new_v4()),
            cutoff,
        ),
    );
    let results = [left.unwrap(), right.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|result| result.disposition == AutoEvolutionReviewDisposition::Created)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| result.disposition == AutoEvolutionReviewDisposition::Duplicate)
            .count(),
        1
    );
    let created = results
        .iter()
        .find(|result| result.disposition == AutoEvolutionReviewDisposition::Created)
        .unwrap();
    store
        .update_evolution_review_proposal_status(created.proposal.id, ReviewProposalStatus::Denied)
        .await
        .unwrap();
    let cooldown = other
        .create_auto_evolution_review_proposal(
            auto_proposal(session.id, turn.id, Uuid::new_v4()),
            Utc::now() - Duration::days(7),
        )
        .await
        .unwrap();
    assert_eq!(
        cooldown.disposition,
        AutoEvolutionReviewDisposition::Cooldown
    );
    let after_cooldown = store
        .create_auto_evolution_review_proposal(
            auto_proposal(session.id, turn.id, Uuid::new_v4()),
            Utc::now() + Duration::seconds(1),
        )
        .await
        .unwrap();
    assert_eq!(
        after_cooldown.disposition,
        AutoEvolutionReviewDisposition::Created
    );
}

#[tokio::test]
async fn gated_postgres_auto_bundle_rolls_back_crash_and_restarts_without_duplicates() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let (admin, admin_connection) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = admin_connection.await;
    });
    let schema = format!("tm_p7_auto_bundle_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let store = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let other = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "persona auto-bundle postgres fixture".to_string(),
            },
        })
        .await
        .unwrap();
    let turn = store
        .enqueue_turn(session.id, "persona-bundle-postgres", "請簡短一點")
        .await
        .unwrap();
    let worker_id = Uuid::new_v4();
    store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_turn(turn.id, worker_id, "好的。", Utc::now())
        .await
        .unwrap();

    store
        .client()
        .batch_execute(
            "create function fail_auto_bundle_approval() returns trigger language plpgsql as $$
                 begin raise exception 'injected auto bundle approval crash'; end
             $$;
             create trigger fail_auto_bundle_approval
             before insert on approval_requests
             for each row execute function fail_auto_bundle_approval()",
        )
        .await
        .unwrap();
    let failed = auto_bundle(session.id, turn.id, Uuid::new_v4(), Uuid::new_v4());
    assert!(
        store
            .create_auto_evolution_review_bundle(failed)
            .await
            .is_err()
    );
    for table in [
        "evolution_review_proposals",
        "approval_requests",
        "approval_effects",
        "session_events",
    ] {
        let count: i64 = store
            .client()
            .query_one(&format!("select count(*) from {table}"), &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 0, "{table} must roll back with the failed bundle");
    }
    store
        .client()
        .batch_execute("drop trigger fail_auto_bundle_approval on approval_requests")
        .await
        .unwrap();

    let left_bundle = auto_bundle(session.id, turn.id, Uuid::new_v4(), Uuid::new_v4());
    let right_bundle = auto_bundle(session.id, turn.id, Uuid::new_v4(), Uuid::new_v4());
    let (left, right) = tokio::join!(
        store.create_auto_evolution_review_bundle(left_bundle),
        other.create_auto_evolution_review_bundle(right_bundle),
    );
    let results = [left.unwrap(), right.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|result| result.disposition == AutoEvolutionReviewDisposition::Created)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| result.disposition == AutoEvolutionReviewDisposition::Duplicate)
            .count(),
        1
    );
    let created = results
        .iter()
        .find(|result| result.disposition == AutoEvolutionReviewDisposition::Created)
        .unwrap();
    let approval = created.approval.as_ref().unwrap();
    assert_eq!(created.events.len(), 2);
    assert_eq!(approval.request_event_seq, Some(created.events[1].seq));
    assert!(
        created
            .events
            .iter()
            .all(|event| event.turn_id == Some(turn.id))
    );

    let restarted = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let duplicate = restarted
        .create_auto_evolution_review_bundle(auto_bundle(
            session.id,
            turn.id,
            Uuid::new_v4(),
            Uuid::new_v4(),
        ))
        .await
        .unwrap();
    assert_eq!(
        duplicate.disposition,
        AutoEvolutionReviewDisposition::Duplicate
    );
    assert!(duplicate.approval.is_none());
    assert!(duplicate.events.is_empty());
    for (table, expected) in [
        ("evolution_review_proposals", 1_i64),
        ("approval_requests", 1),
        ("approval_effects", 1),
    ] {
        let count: i64 = restarted
            .client()
            .query_one(&format!("select count(*) from {table}"), &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, expected, "{table} must remain exact-once");
    }
    let event_counts = restarted
        .client()
        .query(
            "select event_type, count(*)::bigint as count
               from session_events
              group by event_type order by event_type",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(event_counts.len(), 2);
    assert!(
        event_counts
            .iter()
            .all(|row| row.get::<_, i64>("count") == 1)
    );

    drop(restarted);
    drop(other);
    drop(store);
    admin
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}

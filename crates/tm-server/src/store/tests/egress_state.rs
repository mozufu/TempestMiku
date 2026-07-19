use super::*;

fn mutation_intent(session_id: Uuid, turn_id: Uuid) -> tm_egress::EgressMutationIntent {
    tm_egress::EgressMutationIntent {
        effect_id: "1".repeat(64),
        session_id: session_id.to_string(),
        effect_scope_id: turn_id.to_string(),
        session_digest: "2".repeat(64),
        actor_digest: "3".repeat(64),
        destination_id: "issues".into(),
        destination_version: 4,
        target_digest: "5".repeat(64),
        request_digest: "6".repeat(64),
        request_bytes: 42,
    }
}

fn usage_limits(
    requests: u64,
    request_bytes: u64,
    response_bytes: u64,
    time_ms: u64,
) -> tm_egress::EgressUsageLimits {
    tm_egress::EgressUsageLimits {
        max_requests: requests,
        max_request_bytes: request_bytes,
        max_response_bytes: response_bytes,
        max_time_ms: time_ms,
    }
}

fn budget_request(
    reservation_id: &str,
    session_id: Uuid,
    response_reserved: u64,
    time_reserved_ms: u64,
    limits: tm_egress::EgressUsageLimits,
) -> tm_egress::EgressBudgetRequest {
    tm_egress::EgressBudgetRequest {
        reservation_id: reservation_id.to_string(),
        session_id: session_id.to_string(),
        destination_id: "issues".into(),
        request_bytes: 10,
        response_reserved,
        time_reserved_ms,
        session_limits: limits,
        destination_limits: limits,
    }
}

async fn session_and_turn<S: Store>(store: &S, label: &str) -> (SessionRecord, SessionTurnRecord) {
    let session = session_only(store).await;
    let turn = store
        .enqueue_turn(session.id, label, "perform one approved egress mutation")
        .await
        .unwrap();
    (session, turn)
}

async fn session_only<S: Store>(store: &S) -> SessionRecord {
    store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "egress durable-state fixture".to_string(),
            },
        })
        .await
        .unwrap()
}

async fn assert_effect_state_machine<S: Store>(
    store: &S,
    session: &SessionRecord,
    turn: &SessionTurnRecord,
) {
    let intent = mutation_intent(session.id, turn.id);
    let first = store
        .begin_egress_mutation_effect(intent.clone())
        .await
        .unwrap();
    assert!(first.created);
    assert_eq!(
        first.record.status,
        tm_egress::EgressMutationStatus::Started
    );
    let replay = store
        .begin_egress_mutation_effect(intent.clone())
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.record, first.record);
    let terminal = store
        .finish_egress_mutation_effect(
            &intent.effect_id,
            tm_egress::EgressMutationStatus::Uncertain,
            None,
            None,
            Some("interrupted_before_terminal_persistence"),
            Some(&"7".repeat(64)),
        )
        .await
        .unwrap();
    assert_eq!(terminal.status, tm_egress::EgressMutationStatus::Uncertain);
    assert_eq!(
        store
            .finish_egress_mutation_effect(
                &intent.effect_id,
                tm_egress::EgressMutationStatus::Uncertain,
                None,
                None,
                Some("interrupted_before_terminal_persistence"),
                Some(&"7".repeat(64)),
            )
            .await
            .unwrap(),
        terminal
    );
    let mut collision = intent;
    collision.request_digest = "8".repeat(64);
    assert!(matches!(
        store.begin_egress_mutation_effect(collision).await,
        Err(ServerError::Conflict(_))
    ));
}

#[tokio::test]
async fn in_memory_egress_effects_and_budgets_are_atomic_and_fail_closed() {
    let store = InMemoryStore::default();
    let (session, turn) = session_and_turn(
        &store,
        &format!("egress-state-in-memory-{}", Uuid::new_v4()),
    )
    .await;
    assert_effect_state_machine(&store, &session, &turn).await;

    let budget_session = session_only(&store).await;
    let limits = usage_limits(1, 100, 100, 100);
    let first = budget_request(
        "10000000000000000000000000000001",
        budget_session.id,
        50,
        50,
        limits,
    );
    let second = budget_request(
        "10000000000000000000000000000002",
        budget_session.id,
        50,
        50,
        limits,
    );
    let (left, right) = tokio::join!(
        store.reserve_egress_budget(first),
        store.reserve_egress_budget(second)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(matches!(
        left.as_ref().err().or_else(|| right.as_ref().err()),
        Some(ServerError::Policy(_))
    ));
    assert!(matches!(
        store
            .clear_egress_session(&budget_session.id.to_string())
            .await,
        Err(ServerError::Conflict(_))
    ));
    store.end_session(budget_session.id).await.unwrap();
    store
        .clear_egress_session(&budget_session.id.to_string())
        .await
        .unwrap();
}

#[tokio::test]
async fn gated_postgres_egress_state_survives_restart_and_serializes_instances() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_egress_state_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let first_store = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let second_store = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();

    let (effect_session, effect_turn) = session_and_turn(
        &first_store,
        &format!("egress-effect-postgres-{}", Uuid::new_v4()),
    )
    .await;
    let effect = mutation_intent(effect_session.id, effect_turn.id);
    assert!(
        first_store
            .begin_egress_mutation_effect(effect.clone())
            .await
            .unwrap()
            .created
    );
    assert!(
        !second_store
            .begin_egress_mutation_effect(effect.clone())
            .await
            .unwrap()
            .created
    );

    let request_session = session_only(&first_store).await;
    let request_cap = usage_limits(1, 100, 100, 100);
    let left_request = budget_request(
        "20000000000000000000000000000001",
        request_session.id,
        40,
        40,
        request_cap,
    );
    let right_request = budget_request(
        "20000000000000000000000000000002",
        request_session.id,
        40,
        40,
        request_cap,
    );
    let (left, right) = tokio::join!(
        first_store.reserve_egress_budget(left_request),
        second_store.reserve_egress_budget(right_request)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);

    let reservation_session = session_only(&first_store).await;
    let byte_cap = usage_limits(3, 100, 100, 100);
    let durable_reservation = first_store
        .reserve_egress_budget(budget_request(
            "30000000000000000000000000000001",
            reservation_session.id,
            60,
            60,
            byte_cap,
        ))
        .await
        .unwrap();
    drop(first_store);
    drop(second_store);

    let restarted = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let started = restarted
        .begin_egress_mutation_effect(effect.clone())
        .await
        .unwrap();
    assert!(!started.created);
    assert_eq!(
        started.record.status,
        tm_egress::EgressMutationStatus::Started
    );
    restarted
        .finish_egress_mutation_effect(
            &effect.effect_id,
            tm_egress::EgressMutationStatus::Uncertain,
            None,
            None,
            Some("replayed_started_effect"),
            Some(&"9".repeat(64)),
        )
        .await
        .unwrap();
    assert!(matches!(
        restarted
            .reserve_egress_budget(budget_request(
                "30000000000000000000000000000002",
                reservation_session.id,
                60,
                60,
                byte_cap,
            ))
            .await,
        Err(ServerError::Policy(_))
    ));
    restarted
        .settle_egress_budget(durable_reservation, 10, 20)
        .await
        .unwrap();
    restarted
        .reserve_egress_budget(budget_request(
            "30000000000000000000000000000003",
            reservation_session.id,
            60,
            60,
            byte_cap,
        ))
        .await
        .expect("settlement releases only the durable reservation while retaining actual usage");
    let usage = restarted
        .client()
        .query_one(
            "select requests, response_bytes, response_reserved, time_ms, time_reserved_ms
               from egress_session_usage where session_id = $1",
            &[&reservation_session.id],
        )
        .await
        .unwrap();
    assert_eq!(usage.get::<_, i64>("requests"), 2);
    assert_eq!(usage.get::<_, i64>("response_bytes"), 10);
    assert_eq!(usage.get::<_, i64>("response_reserved"), 60);
    assert_eq!(usage.get::<_, i64>("time_ms"), 20);
    assert_eq!(usage.get::<_, i64>("time_reserved_ms"), 60);

    restarted.end_session(reservation_session.id).await.unwrap();
    restarted
        .clear_egress_session(&reservation_session.id.to_string())
        .await
        .unwrap();
    let usage_rows: i64 = restarted
        .client()
        .query_one(
            "select count(*) from egress_session_usage where session_id = $1",
            &[&reservation_session.id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(usage_rows, 0);
    drop(restarted);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}

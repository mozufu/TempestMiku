use super::*;

fn mutation_intent(session_id: Uuid, turn_id: Uuid) -> tm_mcp::McpMutationIntent {
    tm_mcp::McpMutationIntent {
        effect_id: "a".repeat(64),
        session_id: session_id.to_string(),
        effect_scope_id: turn_id.to_string(),
        actor_id: Some("agent://root".to_string()),
        catalog_generation: 7,
        catalog_digest: "b".repeat(64),
        server: "issues".to_string(),
        tool: "create_issue".to_string(),
        target_digest: "c".repeat(64),
        request_digest: "d".repeat(64),
        request_bytes: 42,
    }
}

async fn session_and_turn<S: Store>(
    store: &S,
    message_id: &str,
) -> (SessionRecord, SessionTurnRecord) {
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "MCP mutation effect fixture".to_string(),
            },
        })
        .await
        .unwrap();
    let turn = store
        .enqueue_turn(session.id, message_id, "create the bounded test issue")
        .await
        .unwrap();
    (session, turn)
}

async fn assert_effect_state_machine<S: Store>(
    store: &S,
    session: &SessionRecord,
    turn: &SessionTurnRecord,
) {
    let intent = mutation_intent(session.id, turn.id);
    let first = store
        .begin_mcp_mutation_effect(intent.clone())
        .await
        .unwrap();
    assert!(first.created);
    assert_eq!(
        first.record.status,
        tm_mcp::McpMutationEffectStatus::Started
    );

    let replay = store
        .begin_mcp_mutation_effect(intent.clone())
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.record, first.record);

    let mut equivalent_reload = intent.clone();
    equivalent_reload.catalog_generation += 1;
    equivalent_reload.catalog_digest = "9".repeat(64);
    let reload_replay = store
        .begin_mcp_mutation_effect(equivalent_reload)
        .await
        .unwrap();
    assert!(!reload_replay.created);
    assert_eq!(reload_replay.record, first.record);

    let terminal = store
        .finish_mcp_mutation_effect(
            &intent.effect_id,
            tm_mcp::McpMutationEffectStatus::Uncertain,
            None,
            None,
            Some("mcp_transport_uncertain"),
            Some(&"e".repeat(64)),
        )
        .await
        .unwrap();
    assert_eq!(terminal.status, tm_mcp::McpMutationEffectStatus::Uncertain);

    let terminal_replay = store
        .begin_mcp_mutation_effect(intent.clone())
        .await
        .unwrap();
    assert!(!terminal_replay.created);
    assert_eq!(terminal_replay.record, terminal);

    let idempotent = store
        .finish_mcp_mutation_effect(
            &intent.effect_id,
            tm_mcp::McpMutationEffectStatus::Uncertain,
            None,
            None,
            Some("mcp_transport_uncertain"),
            Some(&"e".repeat(64)),
        )
        .await
        .unwrap();
    assert_eq!(idempotent, terminal);
    assert!(matches!(
        store
            .finish_mcp_mutation_effect(
                &intent.effect_id,
                tm_mcp::McpMutationEffectStatus::Failed,
                None,
                None,
                Some("different_terminal"),
                Some(&"f".repeat(64)),
            )
            .await,
        Err(ServerError::Conflict(_))
    ));

    let mut collision = intent;
    collision.request_digest = "0".repeat(64);
    assert!(matches!(
        store.begin_mcp_mutation_effect(collision).await,
        Err(ServerError::Conflict(_))
    ));
}

#[tokio::test]
async fn in_memory_mcp_mutation_effects_are_idempotent_and_fail_closed() {
    let store = InMemoryStore::default();
    let (session, turn) =
        session_and_turn(&store, &format!("mcp-effect-in-memory-{}", Uuid::new_v4())).await;
    assert_effect_state_machine(&store, &session, &turn).await;

    let missing_turn = mutation_intent(session.id, Uuid::new_v4());
    assert!(matches!(
        store.begin_mcp_mutation_effect(missing_turn).await,
        Err(ServerError::NotFound(_))
    ));
}

#[tokio::test]
async fn gated_postgres_mcp_mutation_effects_survive_restart_without_reexecution() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_mcp_effects_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let store = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let (session, turn) =
        session_and_turn(&store, &format!("mcp-effect-postgres-{}", Uuid::new_v4())).await;
    let intent = mutation_intent(session.id, turn.id);
    let first = store
        .begin_mcp_mutation_effect(intent.clone())
        .await
        .unwrap();
    assert!(first.created);
    drop(store);

    let restarted = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let replay = restarted
        .begin_mcp_mutation_effect(intent.clone())
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(
        replay.record.status,
        tm_mcp::McpMutationEffectStatus::Started
    );
    let mut equivalent_reload = intent.clone();
    equivalent_reload.catalog_generation += 1;
    equivalent_reload.catalog_digest = "9".repeat(64);
    let reload_replay = restarted
        .begin_mcp_mutation_effect(equivalent_reload)
        .await
        .unwrap();
    assert!(!reload_replay.created);
    assert_eq!(reload_replay.record, first.record);
    let mut collision = intent.clone();
    collision.request_digest = "0".repeat(64);
    assert!(matches!(
        restarted.begin_mcp_mutation_effect(collision).await,
        Err(ServerError::Conflict(_))
    ));
    restarted
        .finish_mcp_mutation_effect(
            &intent.effect_id,
            tm_mcp::McpMutationEffectStatus::Uncertain,
            None,
            None,
            Some("replayed_started_effect"),
            Some(&"e".repeat(64)),
        )
        .await
        .unwrap();
    drop(restarted);

    let second_restart = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let terminal = second_restart
        .begin_mcp_mutation_effect(intent)
        .await
        .unwrap();
    assert!(!terminal.created);
    assert_eq!(
        terminal.record.status,
        tm_mcp::McpMutationEffectStatus::Uncertain
    );
    drop(second_restart);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}

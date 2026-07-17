use super::*;

#[tokio::test]
async fn gated_postgres_tm_runtime_events_survive_restart_and_cursor_replay() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_lang_events_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let store = Arc::new(
        PostgresStore::connect_in_schema(&dsn, &schema)
            .await
            .unwrap(),
    );
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "tm-lang Postgres fixture".into(),
            },
        })
        .await
        .unwrap();
    let (sender, _receiver) = tokio::sync::broadcast::channel(16);
    let sink = crate::PersistingEventSink::new(session.id, Arc::clone(&store), sender);
    for (kind, payload) in [
        (
            "scope_start",
            json!({"cellId":"cell-1","nodeId":"scope-1","status":"running"}),
        ),
        (
            "effect_start",
            json!({"cellId":"cell-1","nodeId":"effect-1","parentNodeId":"scope-1","argsPreview":"[redacted]"}),
        ),
        (
            "effect_suspended",
            json!({"cellId":"cell-1","nodeId":"effect-1"}),
        ),
        (
            "effect_resumed",
            json!({"cellId":"cell-1","nodeId":"effect-1","decision":"approved"}),
        ),
        (
            "effect_result",
            json!({"cellId":"cell-1","nodeId":"effect-1","status":"completed"}),
        ),
        (
            "scope_result",
            json!({"cellId":"cell-1","nodeId":"scope-1","status":"completed"}),
        ),
        (
            "display",
            json!({"cellId":"cell-1","nodeId":"display-1","kind":"table"}),
        ),
        (
            "binding_committed",
            json!({"cellId":"cell-1","names":["result"]}),
        ),
    ] {
        tm_core::EventSink::try_on_runtime_event(&sink, kind, &payload).unwrap();
    }
    sink.flush().await.unwrap();
    let initial = store.events_after(session.id, None).await.unwrap();
    assert_eq!(initial.len(), 8);
    assert!(
        initial
            .windows(2)
            .all(|pair| pair[0].seq + 1 == pair[1].seq)
    );
    assert_eq!(initial[1].payload_json["argsPreview"], "[redacted]");
    let cursor = initial[3].seq;

    drop(sink);
    drop(store);
    let restarted = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let replay = restarted
        .events_after(session.id, Some(cursor))
        .await
        .unwrap();
    assert_eq!(
        replay
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec![
            "effect_result",
            "scope_result",
            "display",
            "binding_committed"
        ]
    );

    drop(restarted);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}

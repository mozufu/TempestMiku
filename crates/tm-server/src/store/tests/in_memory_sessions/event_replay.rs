use super::*;

#[tokio::test]
async fn in_memory_session_events_expose_actor_output_refs() {
    let store = InMemoryStore::default();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();

    let non_actor = store
        .append_event(
            session.id,
            "text",
            json!({"delta": "artifact://not-an-actor-output"}),
        )
        .await
        .unwrap();
    assert_eq!(non_actor.actor_id, None);
    assert_eq!(non_actor.artifact_uri, None);
    assert_eq!(non_actor.history_uri, None);

    let completed = store
        .append_event(
            session.id,
            "actor_completed",
            json!({
                "kind": "completed",
                "actor_id": "Worker",
                "artifact_uri": "artifact://0",
                "history_uri": "history://Worker",
            }),
        )
        .await
        .unwrap();
    assert_eq!(completed.actor_id.as_deref(), Some("Worker"));
    assert_eq!(completed.artifact_uri.as_deref(), Some("artifact://0"));
    assert_eq!(completed.history_uri.as_deref(), Some("history://Worker"));

    let linked = store
        .append_event(
            session.id,
            "actor_resources_linked",
            json!({
                "kind": "resources_linked",
                "actor_id": "Worker",
                "source_event_seq": completed.seq,
                "artifact_uri": "artifact://0",
                "history_uri": "history://Worker",
            }),
        )
        .await
        .unwrap();
    assert_eq!(linked.actor_id.as_deref(), Some("Worker"));
    assert_eq!(linked.artifact_uri.as_deref(), Some("artifact://0"));
    assert_eq!(linked.history_uri.as_deref(), Some("history://Worker"));

    let replay = store
        .events_after(session.id, Some(completed.seq - 1))
        .await
        .unwrap();
    assert_eq!(
        replay
            .iter()
            .map(|event| (
                event.event_type.as_str(),
                event.actor_id.as_deref(),
                event.artifact_uri.as_deref(),
                event.history_uri.as_deref(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                "actor_completed",
                Some("Worker"),
                Some("artifact://0"),
                Some("history://Worker")
            ),
            (
                "actor_resources_linked",
                Some("Worker"),
                Some("artifact://0"),
                Some("history://Worker")
            ),
        ]
    );
}

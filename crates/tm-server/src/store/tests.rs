use chrono::Utc;
use serde_json::json;
use tm_modes::{ModeId, AssetStatus};
use uuid::Uuid;

use super::*;

fn postgres_test_dsn() -> Option<String> {
    (std::env::var("TM_POSTGRES_TESTS").ok().as_deref() == Some("1"))
        .then(|| {
            std::env::var("TM_TEST_DATABASE_URL")
                .or_else(|_| std::env::var("TM_DATABASE_URL"))
                .ok()
        })
        .flatten()
}

#[tokio::test]
async fn gated_postgres_covers_replay_memory_approvals_and_project_refs() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("personal_assistant"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();

    let mut mode_state = session.mode_state.clone();
    mode_state.mode = ModeId::from("serious_engineer");
    mode_state.router_reason = Some("postgres replay test".to_string());
    mode_state.updated_at = Utc::now();
    let updated = store
        .set_mode_state(session.id, mode_state.clone())
        .await
        .unwrap();
    assert_eq!(updated.mode_state.mode, ModeId::from("serious_engineer"));

    store
        .append_event(
            session.id,
            "mode",
            serde_json::to_value(StoreEvent::ModeChanged {
                from: Some(ModeId::from("personal_assistant")),
                mode: ModeId::from("serious_engineer"),
                label: "Serious Engineer".to_string(),
                voice_cap: "off".to_string(),
                capabilities: vec![
                    "fs.*".to_string(),
                    "code.*".to_string(),
                    "proc.*".to_string(),
                    "backend.coding".to_string(),
                ],
                active_skills: Vec::new(),
                router_reason: mode_state.router_reason,
                lock_source: None,
                override_source: None,
                updated_at: Utc::now(),
                persona_status: updated.persona_status,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_event(
            session.id,
            "approval",
            json!({"approvalId": Uuid::new_v4(), "backend": "postgres-test"}),
        )
        .await
        .unwrap();
    store
        .append_event(
            session.id,
            "approval_resolved",
            json!({"backend": "postgres-test", "outcome": "selected"}),
        )
        .await
        .unwrap();
    let replay = store.events_after(session.id, Some(1)).await.unwrap();
    assert_eq!(
        replay
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["approval", "approval_resolved"]
    );

    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "project:postgres-test".to_string(),
            text: "memory row for P1 project view".to_string(),
            source: format!("session:{}", session.id),
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    let memory = store
        .recall_chunks("project:postgres-test", "P1 project", 5)
        .await
        .unwrap();
    assert_eq!(memory.len(), 1);

    let project_id = format!("postgres-test-{}", Uuid::new_v4());
    let item = store
        .upsert_project_item(NewProjectItem {
            project_id: project_id.clone(),
            kind: ProjectItemKind::Artifact,
            text: "artifact://0".to_string(),
            target_uri: format!("project://{project_id}/artifacts/0"),
            source_session_id: session.id,
            source_event_seq: Some(2),
            source_uri: Some("artifact://0".to_string()),
            dedupe_key: "artifact://0".to_string(),
            provenance_json: json!({
                "sourceSession": session.id,
                "sourceEventId": 2,
                "sourceUri": "artifact://0",
            }),
        })
        .await
        .unwrap();
    let duplicate = store
        .upsert_project_item(NewProjectItem {
            project_id: project_id.clone(),
            kind: ProjectItemKind::Artifact,
            text: "artifact://0".to_string(),
            target_uri: format!("project://{project_id}/artifacts/0"),
            source_session_id: session.id,
            source_event_seq: Some(2),
            source_uri: Some("artifact://0".to_string()),
            dedupe_key: "artifact://0".to_string(),
            provenance_json: json!({"sourceSession": session.id}),
        })
        .await
        .unwrap();
    assert_eq!(item.id, duplicate.id);
    assert_eq!(
        store
            .project_items(&project_id, Some(ProjectItemKind::Artifact))
            .await
            .unwrap()
            .len(),
        1
    );
}

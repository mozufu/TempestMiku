use serde_json::json;
use tm_modes::{AssetStatus, ModeId};

use super::*;

#[test]
fn mode_state_new_sets_mode_and_empty_sources() {
    let updated_at = Utc::now();
    let state = ModeState::new(ModeId::from("serious_engineer"), updated_at);

    assert_eq!(state.mode.as_str(), "serious_engineer");
    assert_eq!(state.router_reason, None);
    assert_eq!(state.lock_source, None);
    assert_eq!(state.override_source, None);
    assert_eq!(state.updated_at, updated_at);
}

#[test]
fn mode_state_serializes_public_fields_as_camel_case() {
    let updated_at = Utc::now();
    let state = ModeState {
        mode: ModeId::from("general"),
        router_reason: Some("user asked directly".to_string()),
        lock_source: Some("user".to_string()),
        override_source: Some("manual".to_string()),
        updated_at,
    };

    let value = serde_json::to_value(&state).expect("serialize mode state");

    assert_eq!(value["mode"], json!("general"));
    assert_eq!(value["routerReason"], json!("user asked directly"));
    assert_eq!(value["lockSource"], json!("user"));
    assert_eq!(value["overrideSource"], json!("manual"));
    assert!(value.get("updatedAt").is_some());
    assert!(value.get("router_reason").is_none());
}

#[test]
fn project_item_kind_round_trips_all_store_values() {
    let cases = [
        (ProjectItemKind::Status, "status"),
        (ProjectItemKind::Summary, "summary"),
        (ProjectItemKind::OpenLoop, "open_loop"),
        (ProjectItemKind::Decision, "decision"),
        (ProjectItemKind::NextAction, "next_action"),
        (ProjectItemKind::Resource, "resource"),
        (ProjectItemKind::Artifact, "artifact"),
        (ProjectItemKind::Workspace, "workspace"),
        (ProjectItemKind::Linked, "linked"),
    ];

    for (kind, wire) in cases {
        assert_eq!(kind.as_str(), wire);
        assert_eq!(wire.parse::<ProjectItemKind>().expect("parse kind"), kind);
        assert_eq!(
            serde_json::to_value(kind).expect("serialize kind"),
            json!(wire)
        );
    }
}

#[test]
fn project_item_kind_rejects_unknown_values() {
    let err = "blocked".parse::<ProjectItemKind>().unwrap_err();

    assert!(matches!(err, ServerError::Store(_)));
    assert!(
        err.to_string()
            .contains("unknown project item kind blocked")
    );
}

#[test]
fn store_event_serializes_sse_payload_shapes() {
    let updated_at = Utc::now();

    assert_eq!(
        serde_json::to_value(StoreEvent::Text {
            delta: "hello".to_string(),
        })
        .expect("serialize text event"),
        json!({"event": "text", "delta": "hello"})
    );
    assert_eq!(
        serde_json::to_value(StoreEvent::Final {
            text: "done".to_string(),
        })
        .expect("serialize final event"),
        json!({"event": "final", "text": "done"})
    );

    let value = serde_json::to_value(StoreEvent::ModeChanged(Box::new(ModeChangedStoreEvent {
        from: Some(ModeId::from("general")),
        mode: ModeId::from("serious_engineer"),
        label: "Serious Engineer".to_string(),
        voice_cap: "off".to_string(),
        capabilities: vec!["fs.read".to_string()],
        active_skills: vec!["serious-engineer-ops".to_string()],
        router_reason: Some("coding request".to_string()),
        lock_source: Some("user".to_string()),
        override_source: None,
        updated_at,
        persona_status: AssetStatus::Degraded {
            warning: "missing configured assets".to_string(),
        },
    })))
    .expect("serialize mode changed event");

    assert_eq!(value["event"], json!("mode_changed"));
    assert_eq!(value["from"], json!("general"));
    assert_eq!(value["mode"], json!("serious_engineer"));
    assert_eq!(value["voice_cap"], json!("off"));
    assert_eq!(value["router_reason"], json!("coding request"));
    assert_eq!(value["lock_source"], json!("user"));
    assert_eq!(value["override_source"], json!(null));
    assert_eq!(value["activeSkills"], json!(["serious-engineer-ops"]));
    assert!(value.get("active_skills").is_none());
    assert_eq!(
        value["persona_status"],
        json!({"state": "degraded", "warning": "missing configured assets"})
    );
    assert!(value.get("updated_at").is_some());
}

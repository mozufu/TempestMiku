use super::*;
use crate::workflow::conversation_round;
use serde_json::json;

#[test]
fn parses_json_sse_block() {
    let event = parse_sse_block(
        "id: 7\nevent: session_event\ndata: {\"type\":\"final\",\"turnId\":\"turn-7\",\"payload\":{\"text\":\"done\"},\"createdAt\":\"2026-07-10T00:00:00Z\"}\n",
    )
    .unwrap();
    let event = event.unwrap();
    assert_eq!(event.id, Some(7));
    assert_eq!(event.turn_id.as_deref(), Some("turn-7"));
    assert_eq!(event.event_type, "final");
    assert_eq!(event.data["text"], json!("done"));
}

#[test]
fn ignores_keepalive_comments() {
    assert!(parse_sse_block(": keep-alive\n").unwrap().is_none());
}

#[test]
fn builds_conversation_round_from_sse_events() {
    let events = vec![
        E2eEvent {
            id: Some(3),
            turn_id: Some("turn-1".to_string()),
            event_type: "text".to_string(),
            data: json!({ "delta": "hello " }),
        },
        E2eEvent {
            id: Some(4),
            turn_id: Some("turn-1".to_string()),
            event_type: "text".to_string(),
            data: json!({ "delta": "artifact://0" }),
        },
        E2eEvent {
            id: Some(5),
            turn_id: Some("turn-1".to_string()),
            event_type: "artifact".to_string(),
            data: json!({ "artifact": { "uri": "artifact://0" } }),
        },
        E2eEvent {
            id: Some(6),
            turn_id: Some("turn-1".to_string()),
            event_type: "final".to_string(),
            data: json!({ "text": "hello artifact://0" }),
        },
    ];

    let round = conversation_round(
        1,
        WorkflowStep::PersonalAssistantGreeting,
        "status please",
        "general",
        &events,
        "hello artifact://0",
    )
    .unwrap();

    assert_eq!(round.index, 1);
    assert_eq!(round.step, "personal_assistant_greeting");
    assert_eq!(round.user_message, "status please");
    assert_eq!(round.assistant_streamed_text, "hello artifact://0");
    assert_eq!(round.assistant_final_text, "hello artifact://0");
    assert_eq!(round.mode, "general");
    assert_eq!(round.event_id_start, Some(3));
    assert_eq!(round.event_id_end, Some(6));
    assert_eq!(round.event_types, vec!["text", "artifact", "final"]);
    assert_eq!(round.resource_uris, vec!["artifact://0"]);
}

#[tokio::test]
async fn scripted_speaker_uses_message_overrides() {
    let speaker = ScriptedSpeaker::new(
        Some("what tools are available?".to_string()),
        Some("fix the rust test".to_string()),
    );

    assert_eq!(
        speaker
            .message(
                WorkflowStep::PersonalAssistantGreeting,
                &WorkflowContext::default()
            )
            .await
            .unwrap(),
        "what tools are available?"
    );
    assert_eq!(
        speaker
            .message(WorkflowStep::CodingModeProbe, &WorkflowContext::default())
            .await
            .unwrap(),
        "fix the rust test"
    );
}

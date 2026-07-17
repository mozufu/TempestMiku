use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};
use tm_core::{EventSink, LlmClient};
use tm_host::HostEventSink;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::super::{
    events::{ForwardingEventSink, forward_events},
    sink_proxy::SwappableCodingSink,
};
use super::support::{BurstLlm, RecordingCodingSink, backend, coding_turn, options, run_turn};
use crate::CodingEventSink;

#[tokio::test]
async fn sensitive_cell_boundary_forwards_only_structured_previews() {
    let (sender, receiver) = mpsc::channel(8);
    let forwarding = Arc::new(ForwardingEventSink { sender });
    let recorded = Arc::new(RecordingCodingSink::default());
    let target: Arc<dyn CodingEventSink> = recorded.clone();
    let writer = tokio::spawn(forward_events(receiver, target));

    forwarding
        .try_on_cell_start("@fs.patch {patch: \"secret-source-value\"}")
        .unwrap();
    forwarding
        .try_on_cell_result("diff:\n+secret-result-value")
        .unwrap();
    HostEventSink::emit(
        forwarding.as_ref(),
        "cell_start",
        json!({"cellId":"cell-1","sourcePreview":"[redacted]"}),
    )
    .await
    .unwrap();
    HostEventSink::emit(
        forwarding.as_ref(),
        "cell_result",
        json!({"cellId":"cell-1","status":"completed","resultPreview":"[redacted]"}),
    )
    .await
    .unwrap();
    drop(forwarding);
    writer.await.unwrap().unwrap();

    let events = recorded.events.lock();
    assert_eq!(
        events
            .iter()
            .map(|(event_type, _)| event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["cell_start", "cell_result"]
    );
    assert_eq!(events[0].1["sourcePreview"], "[redacted]");
    assert_eq!(events[1].1["resultPreview"], "[redacted]");
    let encoded = serde_json::to_string(&*events).unwrap();
    assert!(!encoded.contains("secret-source-value"), "{encoded}");
    assert!(!encoded.contains("secret-result-value"), "{encoded}");
    assert!(!encoded.contains("\"code\""), "{encoded}");
    assert!(!encoded.contains("\"shaped\""), "{encoded}");
}

#[tokio::test]
async fn swappable_proxy_routes_only_to_the_current_turn_sink() {
    let proxy = SwappableCodingSink::default();
    let first = Arc::new(RecordingCodingSink::default());
    let second = Arc::new(RecordingCodingSink::default());
    let first_target: Arc<dyn CodingEventSink> = first.clone();
    proxy.bind(first_target);
    proxy
        .emit("host_first", json!({ "turn": 1 }))
        .await
        .unwrap();
    let second_target: Arc<dyn CodingEventSink> = second.clone();
    proxy.bind(second_target);
    proxy
        .emit("host_second", json!({ "turn": 2 }))
        .await
        .unwrap();
    proxy.clear();

    assert_eq!(first.event_types(), vec!["host_first"]);
    assert_eq!(second.event_types(), vec!["host_second"]);
    assert!(proxy.emit("late", Value::Null).await.is_err());
}

#[serial_test::serial]
#[tokio::test]
async fn bounded_event_forwarding_fails_the_turn_on_backpressure() {
    let temp = tempfile::tempdir().unwrap();
    let llm: Arc<dyn LlmClient> = Arc::new(BurstLlm::new(64));
    let backend = backend(llm, temp.path(), options(Duration::from_secs(60), 1));
    let sink = Arc::new(RecordingCodingSink::slow(Duration::from_millis(20)));

    let error = run_turn(&backend, coding_turn(Uuid::from_u128(4)), sink)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("native tm bounded event channel is full"),
        "{error}"
    );
}

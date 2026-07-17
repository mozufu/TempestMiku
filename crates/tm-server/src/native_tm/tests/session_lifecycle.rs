use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use serde_json::json;
use tm_core::{LlmClient, Message, Role};
use uuid::Uuid;

use super::support::{RecordingCodingSink, StatefulLlm, backend, coding_turn, options, run_turn};
use crate::{CodingBackend, session_shards::shard_index as native_shard_index};

#[serial_test::serial]
#[tokio::test]
async fn native_sessions_reuse_state_isolate_sessions_and_reset_on_profile_change() {
    let temp = tempfile::tempdir().unwrap();
    let llm = Arc::new(StatefulLlm::new());
    let llm_client: Arc<dyn LlmClient> = llm.clone();
    let backend = backend(
        llm_client,
        temp.path(),
        options(Duration::from_secs(60), 64),
    );
    let first = Uuid::from_u128(1);
    let second = Uuid::from_u128(2);
    assert_eq!(native_shard_index(first, 2), 1);
    assert_eq!(native_shard_index(second, 2), 0);

    let first_sink = Arc::new(RecordingCodingSink::default());
    run_turn(&backend, coding_turn(first), Arc::clone(&first_sink))
        .await
        .unwrap();
    let (agent_start, binding, agent_result) = {
        let first_events = first_sink.events.lock();
        let agent_start = first_events
            .iter()
            .position(|(event, payload)| {
                event == "cell_start" && payload.get("sourcePreview").is_some()
            })
            .unwrap();
        let binding = first_events
            .iter()
            .position(|(event, _)| event == "binding_committed")
            .unwrap();
        let agent_result = first_events
            .iter()
            .position(|(event, payload)| {
                event == "cell_result"
                    && payload.get("status") == Some(&json!("completed"))
                    && payload.get("resultPreview").is_some()
            })
            .unwrap();
        (agent_start, binding, agent_result)
    };
    assert!(agent_start < binding && binding < agent_result);
    let mut incremented = coding_turn(first);
    incremented.user_prompt = "increment native state".to_string();
    run_turn(
        &backend,
        incremented,
        Arc::new(RecordingCodingSink::default()),
    )
    .await
    .unwrap();
    run_turn(
        &backend,
        coding_turn(second),
        Arc::new(RecordingCodingSink::default()),
    )
    .await
    .unwrap();
    let mut changed_profile = coding_turn(first);
    changed_profile.capabilities = vec!["http.get".to_string()];
    run_turn(
        &backend,
        changed_profile,
        Arc::new(RecordingCodingSink::default()),
    )
    .await
    .unwrap();

    let tool_results = llm.tool_results();
    assert_eq!(tool_results.len(), 4);
    assert!(
        tool_results[0].contains("result:\n1"),
        "{}",
        tool_results[0]
    );
    assert!(
        tool_results[1].contains("result:\n2"),
        "{}",
        tool_results[1]
    );
    assert!(
        tool_results[2].contains("result:\n1"),
        "{}",
        tool_results[2]
    );
    assert!(
        tool_results[3].contains("result:\n1"),
        "{}",
        tool_results[3]
    );
}

#[serial_test::serial]
#[tokio::test]
async fn native_failed_runtime_event_discards_cached_session() {
    let temp = tempfile::tempdir().unwrap();
    let llm = Arc::new(StatefulLlm::new());
    let llm_client: Arc<dyn LlmClient> = llm.clone();
    let backend = backend(
        llm_client,
        temp.path(),
        options(Duration::from_secs(60), 64),
    );
    let session_id = Uuid::from_u128(15);
    let sink = Arc::new(RecordingCodingSink::fail_binding_once());

    let error = run_turn(&backend, coding_turn(session_id), Arc::clone(&sink))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("binding_committed persistence failed"),
        "{error}"
    );

    let mut retry = coding_turn(session_id);
    retry.prior_messages = vec![Message::user("persisted earlier turn")];
    run_turn(&backend, retry, Arc::clone(&sink)).await.unwrap();

    assert_eq!(sink.runtime_reset_attempts.load(Ordering::SeqCst), 1);
    assert!(sink.event_types().contains(&"runtime_reset".to_string()));
    let tool_results = llm.tool_results();
    assert_eq!(tool_results.len(), 1);
    assert!(
        tool_results[0].contains("result:\n1"),
        "{}",
        tool_results[0]
    );
}

#[serial_test::serial]
#[tokio::test]
async fn native_durable_abort_evicts_quarantined_runtime_state() {
    let temp = tempfile::tempdir().unwrap();
    let llm = Arc::new(StatefulLlm::new());
    let llm_client: Arc<dyn LlmClient> = llm.clone();
    let backend = backend(
        llm_client,
        temp.path(),
        options(Duration::from_secs(60), 64),
    );
    let session_id = Uuid::from_u128(25);
    let committed_id = Uuid::from_u128(2501);
    let aborted_id = Uuid::from_u128(2502);
    let sink = Arc::new(RecordingCodingSink::default());

    let mut committed = coding_turn(session_id);
    committed.durable_turn_id = Some(committed_id);
    run_turn(&backend, committed, Arc::clone(&sink))
        .await
        .unwrap();
    backend
        .promote_session(session_id, committed_id)
        .await
        .unwrap();

    let mut aborted = coding_turn(session_id);
    aborted.durable_turn_id = Some(aborted_id);
    aborted.user_prompt = "increment native state".to_string();
    run_turn(&backend, aborted, Arc::clone(&sink))
        .await
        .unwrap();
    backend.abort_session(session_id, aborted_id).await.unwrap();

    run_turn(&backend, coding_turn(session_id), sink)
        .await
        .unwrap();

    let tool_results = llm.tool_results();
    assert_eq!(tool_results.len(), 3);
    assert!(tool_results[0].contains("result:\n1"));
    assert!(tool_results[1].contains("result:\n2"));
    assert!(tool_results[2].contains("result:\n1"));
}

#[serial_test::serial]
#[tokio::test]
async fn native_ttl_reopen_emits_reset_and_includes_prior_history() {
    let temp = tempfile::tempdir().unwrap();
    let llm = Arc::new(StatefulLlm::new());
    let llm_client: Arc<dyn LlmClient> = llm.clone();
    let backend = backend(llm_client, temp.path(), options(Duration::ZERO, 64));
    let session_id = Uuid::from_u128(3);
    let first_sink = Arc::new(RecordingCodingSink::default());
    run_turn(&backend, coding_turn(session_id), Arc::clone(&first_sink))
        .await
        .unwrap();
    assert!(
        !first_sink
            .event_types()
            .contains(&"runtime_reset".to_string())
    );

    let second_sink = Arc::new(RecordingCodingSink::default());
    let mut resumed = coding_turn(session_id);
    resumed.prior_messages = vec![
        Message::user("earlier native question"),
        Message::assistant("earlier native answer"),
    ];
    run_turn(&backend, resumed, Arc::clone(&second_sink))
        .await
        .unwrap();

    assert!(
        second_sink
            .event_types()
            .contains(&"runtime_reset".to_string())
    );
    let requests = llm.requests.lock();
    assert_eq!(requests[2][0].role, Role::System);
    assert!(
        requests[2][0]
            .content
            .starts_with("## Immutable tm runtime contract")
    );
    assert!(requests[2][0].content.contains("native test system"));
    assert!(requests[2][0].content.contains("tm-conformance-v2"));
    assert_eq!(requests[2][1], Message::user("earlier native question"));
    assert_eq!(requests[2][2], Message::assistant("earlier native answer"));
    assert_eq!(requests[2][3], Message::user("advance native state"));
    drop(requests);
    let tool_results = llm.tool_results();
    assert!(tool_results[0].contains("result:\n1"));
    assert!(tool_results[1].contains("result:\n1"));
}

#[serial_test::serial]
#[tokio::test]
async fn native_runtime_reset_persistence_failure_retries_before_model_turn() {
    let temp = tempfile::tempdir().unwrap();
    let llm = Arc::new(StatefulLlm::new());
    let llm_client: Arc<dyn LlmClient> = llm.clone();
    let backend = backend(
        llm_client,
        temp.path(),
        options(Duration::from_secs(60), 64),
    );
    let session_id = Uuid::from_u128(14);
    run_turn(
        &backend,
        coding_turn(session_id),
        Arc::new(RecordingCodingSink::default()),
    )
    .await
    .unwrap();
    let requests_before_reset = llm.requests.lock().len();

    let mut resumed = coding_turn(session_id);
    resumed.scope = "project:retry-reset".to_string();
    resumed.prior_messages = vec![
        Message::user("earlier native question"),
        Message::assistant("earlier native answer"),
    ];
    let sink = Arc::new(RecordingCodingSink::fail_runtime_reset_once());

    let error = run_turn(&backend, resumed.clone(), Arc::clone(&sink))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("runtime_reset persistence failed"),
        "{error}"
    );
    assert_eq!(llm.requests.lock().len(), requests_before_reset);

    run_turn(&backend, resumed.clone(), Arc::clone(&sink))
        .await
        .unwrap();
    let mut incremented = resumed;
    incremented.user_prompt = "increment native state".to_string();
    run_turn(&backend, incremented, Arc::clone(&sink))
        .await
        .unwrap();

    assert_eq!(sink.runtime_reset_attempts.load(Ordering::SeqCst), 2);
    let event_types = sink.event_types();
    let reset = event_types
        .iter()
        .position(|event| event == "runtime_reset")
        .unwrap();
    let first_cell = event_types
        .iter()
        .position(|event| event == "cell_start")
        .unwrap();
    assert!(
        reset < first_cell,
        "runtime reset must persist before cells"
    );
    assert_eq!(
        event_types
            .into_iter()
            .filter(|event| event == "runtime_reset")
            .count(),
        1,
        "an acknowledged runtime reset must not be persisted again"
    );
}

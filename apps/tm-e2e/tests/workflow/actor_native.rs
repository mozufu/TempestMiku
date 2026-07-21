use super::server::{
    start_actor_smoke_server, start_native_actor_coordination_server, start_native_tm_server,
};
use super::support::{NATIVE_P3_FINAL_TEXT, assert_native_child_resources};
use super::*;

#[tokio::test]
async fn actor_smoke_covers_progress_approval_resource_and_replay() {
    let (base_url, server, _temp) = start_actor_smoke_server().await;
    let client = MikuClient::new(E2eConfig {
        base_url,
        bearer_token: None,
        timeout: Duration::from_secs(10),
    })
    .unwrap();

    let report = run_actor_smoke(&client).await.unwrap();

    assert_eq!(report.actor_id, "Worker0");
    assert_eq!(report.agent_uri, "agent://Worker0");
    assert_eq!(report.artifact_uri, "artifact://0");
    assert_eq!(report.history_uri, "history://Worker0");
    assert_eq!(report.cancelled_actor_id, "CancelledWorker");
    assert_eq!(report.cancelled_agent_uri, "agent://CancelledWorker");
    assert!(report.approval_id.len() > 8);
    assert!(
        report
            .replayed_event_types
            .contains(&"actor_spawned".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"approval".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"approval_resolved".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"actor_completed".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"actor_resources_linked".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"actor_cancelled".to_string())
    );

    server.abort();
}

#[tokio::test]
async fn native_tm_actor_coordination_public_api_covers_p3_plus_route() {
    let (base_url, server, _temp) = start_native_actor_coordination_server().await;
    let client = MikuClient::new(E2eConfig {
        base_url,
        bearer_token: None,
        timeout: Duration::from_secs(20),
    })
    .unwrap();

    let session = client
        .create_session(Some("serious_engineer"))
        .await
        .unwrap();
    let (_, mode_event) = client
        .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
        .await
        .unwrap();
    let replay_anchor = mode_event.id;

    let send_client = client.clone();
    let send_session_id = session.id.clone();
    let send = tokio::spawn(async move {
        send_client
            .send_message(
                &send_session_id,
                "exercise native P3+ actor coordination route",
            )
            .await
    });
    let live_events = client
        .read_until_final(&session.id, replay_anchor)
        .await
        .unwrap();
    send.await.unwrap().unwrap();

    let final_text = live_events
        .iter()
        .find(|event| event.event_type == "final")
        .and_then(|event| event.data["text"].as_str())
        .unwrap_or_default();
    assert!(final_text.contains(NATIVE_P3_FINAL_TEXT));

    let (first_link_batch, first_link) = client
        .wait_for_event(&session.id, replay_anchor, |event| {
            event.event_type == "actor_resources_linked"
        })
        .await
        .unwrap();
    let (second_link_batch, second_link) = client
        .wait_for_event(&session.id, first_link.id, |event| {
            event.event_type == "actor_resources_linked"
        })
        .await
        .unwrap();
    let replayed = [
        live_events.clone(),
        first_link_batch,
        second_link_batch,
        vec![first_link.clone(), second_link.clone()],
    ]
    .concat();
    let event_types = replayed
        .iter()
        .map(|event| event.event_type.as_str())
        .collect::<Vec<_>>();
    assert!(
        event_types
            .iter()
            .filter(|kind| **kind == "actor_spawned")
            .count()
            >= 2,
        "expected two actor_spawned events, saw {event_types:?}"
    );
    assert!(
        event_types
            .iter()
            .filter(|kind| **kind == "actor_message")
            .count()
            >= 4,
        "expected broadcast and child reply actor_message events, saw {event_types:?}"
    );
    assert!(
        event_types
            .iter()
            .filter(|kind| **kind == "actor_completed")
            .count()
            >= 2,
        "expected two actor_completed events, saw {event_types:?}"
    );
    assert!(
        event_types
            .iter()
            .filter(|kind| **kind == "actor_resources_linked")
            .count()
            >= 2,
        "expected two actor_resources_linked events, saw {event_types:?}"
    );
    assert!(
        event_types.contains(&"final"),
        "expected final event in replayed/native route events"
    );
    let artifact_uris = [
        first_link.data["artifact_uri"].as_str().unwrap(),
        second_link.data["artifact_uri"].as_str().unwrap(),
    ];
    assert_ne!(
        artifact_uris[0], artifact_uris[1],
        "child actor artifact links should be distinct"
    );

    for linked in [first_link, second_link] {
        assert_native_child_resources(&client, &session.id, &linked).await;
    }

    server.abort();
}

#[tokio::test]
async fn native_tm_http_sse_e2e_approves_and_replays_structured_trace() {
    let (base_url, server, temp) = start_native_tm_server().await;
    let client = MikuClient::new(E2eConfig {
        base_url,
        bearer_token: None,
        timeout: Duration::from_secs(10),
    })
    .unwrap();
    let session = client
        .create_session(Some("serious_engineer"))
        .await
        .unwrap();
    client
        .set_session_scope(&session.id, "project:repo")
        .await
        .unwrap();
    let (_, mode) = client
        .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
        .await
        .unwrap();
    let anchor = mode.id;

    let send_client = client.clone();
    let send_session = session.id.clone();
    let send = tokio::spawn(async move {
        send_client
            .send_message(&send_session, "approve the tm e2e fixture removal")
            .await
    });
    let (before_approval, approval) = client
        .wait_for_event(&session.id, anchor, |event| event.event_type == "approval")
        .await
        .unwrap();
    assert!(before_approval.iter().any(|event| {
        event.event_type == "effect_suspended" && event.data["nodeId"].as_str().is_some()
    }));
    let approval_id = approval.data["approvalId"].as_str().unwrap();
    client
        .resolve_approval(&session.id, approval_id, "approve")
        .await
        .unwrap();
    let live_tail = client
        .read_until_final(&session.id, approval.id)
        .await
        .unwrap();
    send.await.unwrap().unwrap();
    assert!(!temp.path().join("repo/remove-me.txt").exists());

    let replay = client.read_until_final(&session.id, anchor).await.unwrap();
    for expected in [
        "scope_start",
        "effect_start",
        "effect_suspended",
        "approval",
        "approval_resolved",
        "effect_resumed",
        "effect_result",
        "scope_result",
        "display",
        "binding_committed",
        "cell_result",
        "final",
    ] {
        assert!(
            replay.iter().any(|event| event.event_type == expected),
            "missing {expected}: {:?}",
            replay
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>()
        );
    }
    let remove_start = replay
        .iter()
        .find(|event| event.event_type == "effect_start" && event.data["capability"] == "fs.remove")
        .unwrap();
    assert_eq!(remove_start.data["argsPreview"], "[redacted]");
    assert!(live_tail.iter().any(|event| event.event_type == "final"));
    server.abort();
}

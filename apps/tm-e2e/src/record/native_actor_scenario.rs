use super::*;

pub(super) async fn run_record_native_actor_inner(recorder: &EvidenceRecorder) -> Result<()> {
    recorder.append_transcript(format!(
        "- Run `native-actor` started at `{}`.",
        timestamp()
    ));
    let live_model = live_llm_model();
    let server = NativeActorRecordingServer::start(&recorder.root(), live_model.clone()).await?;
    recorder.set_server(ServerEvidence {
        base_url: server.base_url.clone(),
        artifact_root: server.artifact_root.display().to_string(),
        store: "in-memory".to_string(),
        coding_backend: "native-tm-scripted-execute-live-final".to_string(),
    });
    let client = MikuClient::new(E2eConfig {
        base_url: server.base_url.clone(),
        bearer_token: None,
        timeout: Duration::from_secs(45),
    })?
    .with_recorder(recorder.clone());

    run_live_llm_preflight(recorder, &live_model).await?;
    run_native_actor_coordination_scenario(recorder, &client, Arc::clone(&server.live_tail_calls))
        .await
}

pub(super) fn ensure_live_llm_env(label: &str) -> Result<()> {
    let api_key_set = env::var("OPENAI_API_KEY")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let base_url_set = env::var("OPENAI_BASE_URL")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    ensure!(
        api_key_set || base_url_set,
        "{label} needs OPENAI_API_KEY or OPENAI_BASE_URL in the environment/.env"
    );
    Ok(())
}

fn live_llm_model() -> String {
    env::var("TM_LLM_MODEL")
        .or_else(|_| env::var("OPENAI_MODEL"))
        .unwrap_or_else(|_| "gpt-4o-mini".to_string())
}

async fn run_live_llm_preflight(recorder: &EvidenceRecorder, model: &str) -> Result<()> {
    let started_at = timestamp();
    let result = async {
        let client =
            OpenAiClient::from_env().context("creating native-actor live preflight LLM")?;
        let turn = client
            .chat(&ChatRequest {
                model: model.to_string(),
                messages: vec![
                    Message::system("You are a deterministic integration-test endpoint."),
                    Message::user(format!(
                        "Return exactly this ASCII token and nothing else: {LIVE_PREFLIGHT_TOKEN}"
                    )),
                ],
                tools: Vec::new(),
                tool_choice: ToolChoice::None,
                temperature: Some(0.0),
                max_tokens: Some(32),
            })
            .await
            .context("running live LLM credential preflight")?;
        ensure!(
            turn.tool_calls.is_empty(),
            "live credential preflight unexpectedly returned tool calls"
        );
        ensure!(
            turn.text
                .to_ascii_uppercase()
                .contains(LIVE_PREFLIGHT_TOKEN),
            "live credential preflight did not return expected token {LIVE_PREFLIGHT_TOKEN}: {:?}",
            turn.text
        );
        recorder.append_transcript(format!(
            "- Live credential preflight: `{LIVE_PREFLIGHT_TOKEN}` via model `{model}`."
        ));
        Ok::<Value, anyhow::Error>(json!({
            "model": model,
            "expectedToken": LIVE_PREFLIGHT_TOKEN,
            "responsePreview": turn.text.chars().take(80).collect::<String>(),
        }))
    }
    .await;
    record_scenario_result(recorder, "live-llm-preflight", started_at, &result);
    result.map(|_| ())
}

async fn run_native_actor_coordination_scenario(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    live_tail_calls: Arc<AtomicUsize>,
) -> Result<()> {
    let started_at = timestamp();
    let result = async {
        let session = client.create_session(Some("serious_engineer")).await?;
        let (_, mode_event) = client
            .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
            .await?;
        let replay_anchor = mode_event.id;

        let send_client = client.clone();
        let send_session_id = session.id.clone();
        let send = tokio::spawn(async move {
            send_client
                .send_message(
                    &send_session_id,
                    "exercise native P3+ actor coordination route with .env live credentials",
                )
                .await
        });
        let live_events = client.read_until_final(&session.id, replay_anchor).await?;
        send.await.context("joining native actor send task")??;
        let final_text = live_events
            .iter()
            .find(|event| event.event_type == "final")
            .and_then(|event| event.data["text"].as_str())
            .unwrap_or_default()
            .to_string();

        let (first_link_batch, first_link) = client
            .wait_for_event(&session.id, replay_anchor, |event| {
                event.event_type == "actor_resources_linked"
            })
            .await?;
        let (second_link_batch, second_link) = client
            .wait_for_event(&session.id, first_link.id, |event| {
                event.event_type == "actor_resources_linked"
            })
            .await?;
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
        ensure_min_event_count(&event_types, "actor_spawned", 2)?;
        ensure_min_event_count(&event_types, "actor_message", 4)?;
        ensure_min_event_count(&event_types, "actor_completed", 2)?;
        ensure_min_event_count(&event_types, "actor_resources_linked", 2)?;
        ensure!(
            event_types.contains(&"final"),
            "native actor route did not replay a final event"
        );

        let mut resources = Vec::new();
        let mut artifact_uris = Vec::new();
        for linked in [&first_link, &second_link] {
            let actor_id = linked.data["actor_id"]
                .as_str()
                .context("actor_resources_linked actor_id")?
                .to_string();
            let artifact_uri = linked.data["artifact_uri"]
                .as_str()
                .context("actor_resources_linked artifact_uri")?
                .to_string();
            let history_uri = linked.data["history_uri"]
                .as_str()
                .context("actor_resources_linked history_uri")?
                .to_string();
            ensure!(
                history_uri == format!("history://{actor_id}"),
                "unexpected history uri {history_uri} for actor {actor_id}"
            );
            ensure_native_child_resource_contents(
                client,
                &session.id,
                &actor_id,
                &artifact_uri,
                &history_uri,
            )
            .await?;
            let agent_uri = format!("agent://{actor_id}");
            capture_resource(recorder, client, &session.id, &artifact_uri).await?;
            capture_resource(recorder, client, &session.id, &history_uri).await?;
            capture_resource(recorder, client, &session.id, &agent_uri).await?;
            artifact_uris.push(artifact_uri.clone());
            resources.push(json!({
                "actorId": actor_id,
                "artifactUri": artifact_uri,
                "historyUri": history_uri,
                "agentUri": agent_uri,
            }));
        }
        ensure!(
            artifact_uris.len() == 2 && artifact_uris[0] != artifact_uris[1],
            "native actor route expected distinct child artifact URIs, saw {artifact_uris:?}"
        );

        let live_tail_call_count = live_tail_calls.load(Ordering::SeqCst);
        ensure!(
            live_tail_call_count >= 3,
            "native actor route expected at least 3 live final LLM calls, saw {live_tail_call_count}"
        );
        recorder.append_transcript(format!(
            "- Native actor route final: `{}`",
            final_text.chars().take(240).collect::<String>()
        ));
        recorder.append_transcript(format!(
            "- Native actor replay event types: `{}`",
            event_types.join("`, `")
        ));
        recorder.append_transcript(format!(
            "- Native actor live final LLM calls: `{live_tail_call_count}`."
        ));
        Ok::<Value, anyhow::Error>(json!({
            "sessionId": session.id,
            "finalText": final_text,
            "eventTypes": event_types,
            "resources": resources,
            "liveTailCalls": live_tail_call_count,
        }))
    }
    .await;
    record_scenario_result(recorder, "native-actor", started_at, &result);
    result.map(|_| ())
}

fn ensure_min_event_count(event_types: &[&str], event_type: &str, expected: usize) -> Result<()> {
    let actual = event_types
        .iter()
        .filter(|kind| **kind == event_type)
        .count();
    ensure!(
        actual >= expected,
        "expected at least {expected} `{event_type}` events, saw {actual}: {event_types:?}"
    );
    Ok(())
}

async fn ensure_native_child_resource_contents(
    client: &MikuClient,
    session_id: &str,
    actor_id: &str,
    artifact_uri: &str,
    history_uri: &str,
) -> Result<()> {
    let artifact = client.resolve_resource(session_id, artifact_uri).await?;
    let artifact_content = artifact["content"]
        .as_str()
        .context("artifact content string")?;
    ensure!(
        artifact_content.contains(NATIVE_P3_BROADCAST_TEXT),
        "artifact {artifact_uri} did not contain broadcast token: {artifact_content}"
    );

    let history = client.resolve_resource(session_id, history_uri).await?;
    let history_content = history["content"]
        .as_str()
        .context("history content string")?;
    ensure!(
        history_content.contains("[tool_call] execute")
            && history_content.contains("[cell_start] [redacted]")
            && history_content.contains("[cell_result]")
            && !history_content.contains("@agents.wait"),
        "history {history_uri} did not preserve content-blind native actor transcript markers"
    );

    let agent = client
        .resolve_resource(session_id, &format!("agent://{actor_id}"))
        .await?;
    let record: Value =
        serde_json::from_str(agent["content"].as_str().context("agent content string")?)?;
    ensure!(
        record["status"] == json!("terminated"),
        "actor not terminal"
    );
    ensure!(record["cancelled"] == json!(false), "actor was cancelled");
    ensure!(
        record["artifact_uri"] == json!(artifact_uri),
        "agent record artifact_uri mismatch"
    );
    ensure!(
        record["history_uri"] == json!(history_uri),
        "agent record history_uri mismatch"
    );
    Ok(())
}

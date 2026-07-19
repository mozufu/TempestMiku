use super::*;

pub async fn run_record_suite(options: RecordOptions) -> Result<EvidenceManifest> {
    run_recorded("suite", options, true, true, false).await
}

pub async fn run_record_api(options: RecordOptions) -> Result<EvidenceManifest> {
    run_recorded("api", options, true, true, false).await
}

pub async fn run_record_live_api(options: RecordOptions) -> Result<EvidenceManifest> {
    crate::load_dotenv();
    ensure!(
        env::var("TM_LLM_E2E_LIVE").ok().as_deref() == Some("1"),
        "record live-api is gated by TM_LLM_E2E_LIVE=1"
    );
    run_recorded("live-api", options, true, false, true).await
}

pub async fn run_record_native_actor(options: RecordOptions) -> Result<EvidenceManifest> {
    crate::load_dotenv();
    ensure!(
        env::var("TM_LLM_E2E_LIVE").ok().as_deref() == Some("1"),
        "record native-actor is gated by TM_LLM_E2E_LIVE=1"
    );
    ensure_live_llm_env("record native-actor")?;

    let root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| default_run_dir("native-actor"));
    let command = env::args().collect::<Vec<_>>().join(" ");
    let recorder = EvidenceRecorder::create(&root, command)?;
    let result = run_record_native_actor_inner(&recorder).await;
    let manifest = recorder.finish(result.is_ok())?;
    if let Err(err) = result {
        bail!(
            "tm-e2e record native-actor failed: {err}; evidence: {}",
            manifest.run_dir
        );
    }
    Ok(manifest)
}

async fn run_recorded(
    label: &str,
    options: RecordOptions,
    include_public_api: bool,
    include_actor_api: bool,
    live_api: bool,
) -> Result<EvidenceManifest> {
    crate::load_dotenv();
    let root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| default_run_dir(label));
    let command = env::args().collect::<Vec<_>>().join(" ");
    let recorder = EvidenceRecorder::create(&root, command)?;
    let result = run_recorded_inner(
        label,
        &recorder,
        include_public_api,
        include_actor_api,
        live_api,
    )
    .await;
    let manifest = recorder.finish(result.is_ok())?;
    if let Err(err) = result {
        bail!(
            "tm-e2e record {label} failed: {err}; evidence: {}",
            manifest.run_dir
        );
    }
    Ok(manifest)
}

async fn run_recorded_inner(
    label: &str,
    recorder: &EvidenceRecorder,
    include_public_api: bool,
    include_actor_api: bool,
    live_api: bool,
) -> Result<()> {
    recorder.append_transcript(format!("- Run `{label}` started at `{}`.", timestamp()));

    let (client, _server) = if live_api {
        (
            MikuClient::from_env()?.with_recorder(recorder.clone()),
            None,
        )
    } else {
        let server = RecordingServer::start(&recorder.root()).await?;
        recorder.set_server(ServerEvidence {
            base_url: server.base_url.clone(),
            artifact_root: server.artifact_root.display().to_string(),
            store: "in-memory".to_string(),
            coding_backend: "tm-e2e-recording-fixture".to_string(),
        });
        let client = MikuClient::new(E2eConfig {
            base_url: server.base_url.clone(),
            bearer_token: None,
            timeout: Duration::from_secs(30),
        })?
        .with_recorder(recorder.clone());
        (client, Some(server))
    };

    if include_public_api {
        run_public_api_scenario(recorder, &client, live_api).await?;
    }
    if include_actor_api {
        run_actor_api_scenario(recorder, &client).await?;
    }
    Ok(())
}

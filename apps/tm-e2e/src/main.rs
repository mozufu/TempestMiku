use anyhow::{Result, bail};
use tm_e2e::{LiveSpeaker, MikuClient, ScriptedSpeaker, WorkflowOptions, run_workflow};

#[tokio::main]
async fn main() -> Result<()> {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "scripted".to_string());
    let require_artifact = std::env::var("TM_E2E_REQUIRE_ARTIFACT").ok().as_deref() == Some("1");
    let client = MikuClient::from_env()?;
    let options = WorkflowOptions { require_artifact };

    let report = match mode.as_str() {
        "scripted" => run_workflow(&client, &ScriptedSpeaker, options).await?,
        "live" => {
            let speaker = LiveSpeaker::from_env()?;
            run_workflow(&client, &speaker, options).await?
        }
        "help" | "--help" | "-h" => {
            print_help();
            return Ok(());
        }
        other => bail!("unsupported tm-e2e mode {other}; expected scripted or live"),
    };

    println!("tm-e2e workflow passed");
    println!("session: {}", report.session_id);
    println!("memory: {}", report.memory_record_uri);
    if let Some(uri) = report.artifact_uri {
        println!("artifact: {uri}");
    }
    println!("promoted: {}", report.promoted_count);
    Ok(())
}

fn print_help() {
    println!(
        "tm-e2e — drive TempestMiku through the public session API\n\n\
         Usage:\n  \
           cargo run -p tm-e2e -- scripted\n  \
           TM_LLM_E2E_LIVE=1 OPENAI_API_KEY=... cargo run -p tm-e2e -- live\n\n\
         Environment:\n  \
           TM_MIKU_BASE_URL          server URL, default http://127.0.0.1:8787\n  \
           TM_MIKU_BEARER_TOKEN      optional bearer token for tm-server auth\n  \
           TM_MIKU_E2E_TIMEOUT_MS    SSE wait timeout, default 15000\n  \
           TM_E2E_REQUIRE_ARTIFACT   set 1 to require an artifact event/resource\n  \
           TM_E2E_SPEAKER_MODEL      live-mode speaker model, default OPENAI_MODEL"
    );
}

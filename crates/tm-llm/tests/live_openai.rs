//! Gated live end-to-end coverage for the OpenAI-compatible chat client.
//!
//! Run with an OpenAI-compatible endpoint in the environment:
//!
//! ```text
//! TM_LLM_LIVE=1 TM_LLM_MODEL=<model> cargo test -p tm-llm --test live_openai -- --nocapture
//! ```
//!
//! Optional: `OPENAI_BASE_URL` defaults to `https://api.openai.com/v1`; set
//! `OPENAI_STREAM_USAGE=0` for endpoints that reject `stream_options.include_usage`.

use std::path::Path;
use tm_core::{ChatRequest, LlmClient, Message, ToolChoice};
use tm_llm::OpenAiClient;

const EXPECTED_TOKEN: &str = "TEMPEST_MIKU_E2E_OK";

#[tokio::test]
async fn live_chat_completions_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    load_dotenv();
    if !live_enabled() {
        eprintln!(
            "skipping live OpenAI-compatible e2e; set TM_LLM_LIVE=1 and TM_LLM_MODEL or OPENAI_MODEL"
        );
        return Ok(());
    }

    let model = live_model().expect("set TM_LLM_MODEL or OPENAI_MODEL for live e2e");
    let client = OpenAiClient::from_env()?;

    let turn = client
        .chat(&ChatRequest {
            model,
            messages: vec![
                Message::system("You are a deterministic integration-test endpoint."),
                Message::user(format!(
                    "Return exactly this ASCII token and nothing else: {EXPECTED_TOKEN}"
                )),
            ],
            tools: Vec::new(),
            tool_choice: ToolChoice::None,
            temperature: Some(0.0),
            max_tokens: Some(32),
        })
        .await?;

    assert!(
        turn.tool_calls.is_empty(),
        "basic no-tool request unexpectedly returned tool calls: {:?}",
        turn.tool_calls
    );
    assert!(
        turn.text.to_ascii_uppercase().contains(EXPECTED_TOKEN),
        "live response did not contain expected token {EXPECTED_TOKEN}: {:?}",
        turn.text
    );

    Ok(())
}

fn load_dotenv() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(path) = manifest_dir
        .ancestors()
        .map(|dir| dir.join(".env"))
        .find(|path| path.is_file())
    {
        let _ = dotenvy::from_path(path);
    }
}

fn live_enabled() -> bool {
    std::env::var("TM_LLM_LIVE")
        .map(|v| {
            matches!(
                v.trim(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(false)
}

fn live_model() -> Option<String> {
    std::env::var("TM_LLM_MODEL")
        .or_else(|_| std::env::var("OPENAI_MODEL"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

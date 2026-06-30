use std::{net::SocketAddr, sync::Arc};

use tm_core::{Agent, AgentConfig, DEFAULT_SYSTEM_PROMPT};
use tm_llm::OpenAiClient;
use tm_persona::{Mode, PersonaConfig};
use tm_sandbox::StubSandbox;

use tm_server::{
    AgentChatRunner, AppState, AuthConfig, CodingBackend, EchoChatRunner, InMemoryStore,
    OmpAcpBackend, OmpAcpConfig, PostgresStore, ServerChatRunner, StoreMemoryProvider, app,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let addr: SocketAddr = std::env::var("TM_SERVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8787".to_string())
        .parse()?;
    let persona = std::env::var_os("TM_PERSONA_PATH")
        .map(PersonaConfig::from_path)
        .unwrap_or_default();
    let chat = build_chat()?;

    if let Ok(dsn) = std::env::var("TM_DATABASE_URL") {
        let store = Arc::new(PostgresStore::connect(&dsn).await?);
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let state = configure_coding_backend(AppState::new(
            store,
            memory,
            chat,
            persona,
            AuthConfig::NoAuth,
        ))?;
        serve(addr, state).await?;
    } else {
        tracing::warn!(
            "TM_DATABASE_URL not set; using non-durable in-memory store for local development"
        );
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let state = configure_coding_backend(AppState::new(
            store,
            memory,
            chat,
            persona,
            AuthConfig::NoAuth,
        ))?;
        serve(addr, state).await?;
    }

    Ok(())
}

fn build_chat() -> Result<Arc<ServerChatRunner>, Box<dyn std::error::Error>> {
    let api_key_set = std::env::var("OPENAI_API_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    let base_url_set = std::env::var("OPENAI_BASE_URL")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);

    if !api_key_set && !base_url_set {
        tracing::warn!("OPENAI_API_KEY / OPENAI_BASE_URL not set — falling back to EchoChatRunner");
        return Ok(Arc::new(ServerChatRunner::Echo(EchoChatRunner)));
    }

    let llm = Arc::new(OpenAiClient::from_env()?);
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let sandbox = Arc::new(StubSandbox);
    let system_prompt = format!(
        "{DEFAULT_SYSTEM_PROMPT}\n\n{}",
        Mode::PersonalAssistant.system_addendum()
    );
    let cfg = AgentConfig {
        model: model.clone(),
        system_prompt,
        ..AgentConfig::default()
    };
    let agent = Agent::new(llm, sandbox, cfg);
    tracing::info!(model, "real LLM agent runner configured");
    Ok(Arc::new(ServerChatRunner::Agent(AgentChatRunner::new(agent))))
}

fn configure_coding_backend<S, M, C>(
    mut state: AppState<S, M, C>,
) -> Result<AppState<S, M, C>, Box<dyn std::error::Error>> {
    if let Some(root) = std::env::var_os("TM_OMP_ACP_ARTIFACT_ROOT") {
        state = state.with_artifact_root(root);
    }
    if std::env::var("TM_OMP_ACP_ENABLED").ok().as_deref() == Some("1") {
        let backend: Arc<dyn CodingBackend> = Arc::new(OmpAcpBackend::new(
            OmpAcpConfig::from_env()?,
            Arc::clone(&state.approval_broker),
        )?);
        state = state.with_coding_backend(backend);
    }
    Ok(state)
}

async fn serve<S, M, C>(
    addr: SocketAddr,
    state: AppState<S, M, C>,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: tm_server::Store,
    M: tm_server::MemoryProvider,
    C: tm_server::ChatRunner,
{
    tracing::info!(%addr, "tm-server listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(state)).await?;
    Ok(())
}

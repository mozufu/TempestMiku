use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use tm_artifacts::default_root;
use tm_core::{AgentConfig, CellBudget, DEFAULT_SYSTEM_PROMPT, LlmClient};
use tm_host::{ApprovalPolicy, DefaultDenyApprovalPolicy, LinkedFolders, P0HostConfig};
use tm_llm::OpenAiClient;
use tm_persona::{Mode, PersonaConfig};
use tm_sandbox::DenoSandboxOptions;

use tm_server::{
    AgentChatRunner, AppState, AuthConfig, CodingBackend, EchoChatRunner, InMemoryStore,
    NativeApprovalMode, NativeDenoBackend, OmpAcpBackend, OmpAcpConfig, PostgresStore,
    ServerChatRunner, StoreMemoryProvider, app,
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
    let host_config = load_host_config()?;
    let linked_folders = host_config.linked_folders()?;
    let artifact_root = server_artifact_root(&host_config);
    let runtime = build_runtime(&host_config, &linked_folders, artifact_root.clone())?;

    if let Ok(dsn) = std::env::var("TM_DATABASE_URL")
        && !dsn.trim().is_empty()
    {
        let store = Arc::new(PostgresStore::connect(&dsn).await?);
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let state = configure_coding_backend(
            AppState::new(store, memory, runtime.chat, persona, AuthConfig::NoAuth),
            &linked_folders,
            artifact_root.clone(),
            runtime.native_deno,
        )?;
        serve(addr, state).await?;
    } else {
        tracing::warn!(
            "TM_DATABASE_URL not set; using non-durable in-memory store for local development"
        );
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let state = configure_coding_backend(
            AppState::new(store, memory, runtime.chat, persona, AuthConfig::NoAuth),
            &linked_folders,
            artifact_root.clone(),
            runtime.native_deno,
        )?;
        serve(addr, state).await?;
    }

    Ok(())
}

struct BuiltRuntime {
    chat: Arc<ServerChatRunner>,
    native_deno: Option<NativeDenoBackendConfig>,
}

struct NativeDenoBackendConfig {
    llm: Arc<dyn LlmClient>,
    cfg: AgentConfig,
    sandbox_options: DenoSandboxOptions,
    approval_mode: NativeApprovalMode,
}

fn build_runtime(
    host_config: &P0HostConfig,
    linked_folders: &LinkedFolders,
    artifact_root: PathBuf,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    let api_key_set = std::env::var("OPENAI_API_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    let base_url_set = std::env::var("OPENAI_BASE_URL")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);

    if !api_key_set && !base_url_set {
        tracing::warn!("OPENAI_API_KEY / OPENAI_BASE_URL not set — falling back to EchoChatRunner");
        return Ok(BuiltRuntime {
            chat: Arc::new(ServerChatRunner::Echo(EchoChatRunner)),
            native_deno: None,
        });
    }

    let llm: Arc<dyn LlmClient> = Arc::new(OpenAiClient::from_env()?);
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let system_prompt = server_agent_prompt(linked_folders);
    let cfg = AgentConfig {
        model: model.clone(),
        system_prompt,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };
    let linked_folders = (!linked_folders.is_empty()).then_some(linked_folders.clone());
    let sandbox_options = DenoSandboxOptions {
        artifact_root,
        linked_folders,
        approval_policy: chat_approval_policy(host_config)?,
        approval_timeout: Duration::from_millis(host_config.approvals.timeout_ms),
        ..DenoSandboxOptions::default()
    };
    let approval_mode = NativeApprovalMode::parse(&host_config.approvals.mode)?;
    tracing::info!(model, "real LLM agent runner configured");
    Ok(BuiltRuntime {
        chat: Arc::new(ServerChatRunner::Agent(AgentChatRunner::deno(
            Arc::clone(&llm),
            cfg.clone(),
            sandbox_options.clone(),
        ))),
        native_deno: Some(NativeDenoBackendConfig {
            llm,
            cfg,
            sandbox_options,
            approval_mode,
        }),
    })
}

fn configure_coding_backend<S, M, C>(
    mut state: AppState<S, M, C>,
    linked_folders: &LinkedFolders,
    artifact_root: PathBuf,
    native_deno: Option<NativeDenoBackendConfig>,
) -> Result<AppState<S, M, C>, Box<dyn std::error::Error>> {
    state = state
        .with_artifact_root(artifact_root)
        .with_linked_folders(linked_folders.clone());
    if std::env::var("TM_OMP_ACP_ENABLED").ok().as_deref() == Some("1") {
        let backend: Arc<dyn CodingBackend> = Arc::new(OmpAcpBackend::new(
            OmpAcpConfig::from_env()?,
            Arc::clone(&state.approval_broker),
        )?);
        state = state.with_coding_backend(backend);
    } else if let Some(native_deno) = native_deno {
        let backend: Arc<dyn CodingBackend> = Arc::new(NativeDenoBackend::new(
            native_deno.llm,
            native_deno.cfg,
            native_deno.sandbox_options,
            native_deno.approval_mode,
            Arc::clone(&state.approval_broker),
        ));
        state = state.with_coding_backend(backend);
    }
    Ok(state)
}

fn load_host_config() -> Result<P0HostConfig, Box<dyn std::error::Error>> {
    let path = std::env::var_os("TM_HOST_CONFIG")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TM_CONFIG").map(PathBuf::from))
        .or_else(|| {
            let default = PathBuf::from(".tempestmiku/config.json");
            default.exists().then_some(default)
        });
    match path {
        Some(path) => Ok(P0HostConfig::from_json_file(path)?),
        None => Ok(P0HostConfig {
            linked_folders: Vec::new(),
            approvals: Default::default(),
            artifact_root: None,
        }),
    }
}

fn server_artifact_root(host_config: &P0HostConfig) -> PathBuf {
    std::env::var_os("TM_OMP_ACP_ARTIFACT_ROOT")
        .map(PathBuf::from)
        .or_else(|| host_config.artifact_root.clone())
        .unwrap_or_else(default_root)
}

fn chat_approval_policy(
    config: &P0HostConfig,
) -> Result<Arc<dyn ApprovalPolicy>, Box<dyn std::error::Error>> {
    match config.approvals.mode.as_str() {
        "deny" | "" | "manual" => Ok(Arc::new(DefaultDenyApprovalPolicy)),
        other => Err(format!("unsupported approval mode {other}").into()),
    }
}

fn server_agent_prompt(linked_folders: &LinkedFolders) -> String {
    let mut prompt = format!(
        "{DEFAULT_SYSTEM_PROMPT}\n\n{}",
        Mode::PersonalAssistant.system_addendum()
    );
    prompt.push('\n');
    if linked_folders.is_empty() {
        prompt.push_str("No linked folders configured; fs.*, code.*, and proc.* will fail closed.");
    } else {
        for policy in linked_folders.policies() {
            let mode = match policy.mode {
                tm_host::FsMode::Ro => "ro",
                tm_host::FsMode::Rw => "rw",
            };
            prompt.push_str(&format!(
                "Linked folders: {} ({mode}) at linked://{}/\n",
                policy.alias, policy.alias
            ));
        }
    }
    prompt
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

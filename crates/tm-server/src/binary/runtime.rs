use std::{path::PathBuf, sync::Arc, time::Duration};

use tm_agents::MailboxRegistry;
use tm_artifacts::ArtifactStore;
use tm_core::{AgentConfig, CellBudget, DEFAULT_SYSTEM_PROMPT, LlmClient, Sandbox};
use tm_egress::{EgressAdmin, EgressRuntime, register_egress_functions};
use tm_host::{
    ApprovalPolicy, CapabilityGrants, DefaultDenyApprovalPolicy, HostEventSink, InvocationCtx,
    LinkedFolders, P0HostConfig,
};
use tm_lang::{TmSandbox, TmSandboxOptions};
use tm_llm::OpenAiClient;
use tm_mcp::{
    EgressMcpTransport, McpBindings, McpBounds, McpCatalogContext, McpCatalogManager,
    McpCatalogView, McpHttpServerConfig, McpHttpTransportBounds, McpRuntimeConfig,
};
use tm_modes::ModesConfig;
use tm_server::{
    AgentChatRunner, AppState, ApprovalBroker, ChatActorExecutor, CodingBackend, CodingEventSink,
    EchoChatRunner, HttpApprovalPolicy, NativeApprovalMode, NativeTmBackend, OmpAcpBackend,
    OmpAcpConfig, RosterCodingEventSink, ServerChatRunner,
};
use tm_worker_protocol::{RemoteWorkerConfig, RemoteWorkerConnector};

use super::{BoxError, config::load_host_config};

pub(super) struct BuiltRuntime {
    pub(super) chat: Arc<ServerChatRunner>,
    pub(super) native_tm: Option<NativeTmBackendConfig>,
    pub(super) mcp: Option<BuiltMcpRuntime>,
    pub(super) egress_admin: EgressAdmin,
    pub(super) linked_aliases: Vec<String>,
    pub(super) linked_resource_handler: Option<Arc<dyn tm_host::ResourceHandler>>,
}

pub(super) struct BuiltMcpRuntime {
    pub(super) catalog: McpCatalogView,
    pub(super) bindings: McpBindings,
    pub(super) http_servers: Vec<McpHttpServerConfig>,
}

pub(super) struct NativeTmBackendConfig {
    llm: Arc<dyn LlmClient>,
    cfg: AgentConfig,
    sandbox_options: TmSandboxOptions,
    approval_mode: NativeApprovalMode,
}

pub(super) struct RuntimePolicies<'a> {
    pub(super) host: &'a P0HostConfig,
    pub(super) mcp: &'a McpRuntimeConfig,
    pub(super) remote_worker: Option<&'a RemoteWorkerConfig>,
}

pub(super) async fn build_runtime(
    policies: RuntimePolicies<'_>,
    linked_folders: &LinkedFolders,
    persona: &ModesConfig,
    artifact_root: PathBuf,
    drive_store: Option<tm_drive::SharedDriveStore>,
    roster: Arc<MailboxRegistry>,
    approval_broker: Arc<ApprovalBroker>,
) -> Result<BuiltRuntime, BoxError> {
    let host_config = policies.host;
    // Build one shared policy/budget/secret-handle boundary for root chat, native coding, and
    // explicitly delegated child actor sandboxes. Cloning the runtime retains the same state.
    let egress_runtime = EgressRuntime::new(host_config.egress.clone())?;
    let egress_admin = egress_runtime.admin_handle();
    let mcp = build_mcp_runtime(policies.mcp, egress_runtime.clone()).await?;
    let remote_connector = policies
        .remote_worker
        .map(|config| RemoteWorkerConnector::from_config(config.clone()))
        .transpose()?;
    let linked_aliases = remote_connector
        .as_ref()
        .map(|connector| connector.linked_aliases().to_vec())
        .unwrap_or_default();
    let linked_resource_handler = remote_connector
        .as_ref()
        .map(|connector| {
            ArtifactStore::open(&artifact_root, "remote-gateway")
                .map(|artifacts| connector.linked_resource_handler(artifacts))
        })
        .transpose()?;
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
            native_tm: None,
            mcp,
            egress_admin,
            linked_aliases,
            linked_resource_handler,
        });
    }

    let llm: Arc<dyn LlmClient> = Arc::new(OpenAiClient::from_env()?);
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let cfg = AgentConfig {
        model: model.clone(),
        system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };
    let linked_folders =
        (drive_store.is_some() || !linked_folders.is_empty()).then_some(linked_folders.clone());
    let approval_mode = NativeApprovalMode::parse(&host_config.approvals.mode)?;
    let mut sandbox_options = TmSandboxOptions {
        artifact_root,
        linked_folders,
        drive_store,
        approval_policy: chat_approval_policy(host_config)?,
        approval_timeout: Duration::from_millis(host_config.approvals.timeout_ms),
        proc_run_timeout: Duration::from_millis(host_config.proc_run_timeout_ms),
        proc_isolation: host_config.proc_isolation.clone(),
        ..TmSandboxOptions::default()
    };
    if let Some(connector) = remote_connector {
        sandbox_options.linked_aliases = connector.linked_aliases().to_vec();
        sandbox_options.host_connectors.push(Arc::new(connector));
    }
    tm_agents::register(
        &mut sandbox_options.host_registry,
        &mut sandbox_options.resource_registry,
        Arc::clone(&roster),
    );
    register_egress_functions(&mut sandbox_options.host_registry, egress_runtime);
    sandbox_options
        .resource_registry
        .register(Arc::new(tm_modes::SkillResourceHandler::new(
            persona.clone(),
        )));
    if let Some(mcp) = &mcp {
        mcp.bindings.register_into(
            &mut sandbox_options.host_registry,
            &mut sandbox_options.resource_registry,
        )?;
    }

    // Inject executor AFTER sandbox_options has agents.* registered so child actor
    // sandboxes inherit the same host registry (including agents.* for recursive actors).
    let executor_options = sandbox_options.clone();
    let executor_artifact_root = executor_options.artifact_root.clone();
    let executor_roster = Arc::clone(&roster);
    let executor_approval_roster = Arc::clone(&roster);
    let executor_approval_broker = Arc::clone(&approval_broker);
    let executor: Arc<dyn tm_agents::ActorExecutor> =
        Arc::new(ChatActorExecutor::with_actor_context(
            Arc::clone(&llm),
            cfg.clone(),
            move |session_id: uuid::Uuid,
                  actor_id: Option<&str>,
                  grants: &tm_host::CapabilityGrants,
                  project_id: Option<&str>,
                  cancellation: Option<Arc<dyn tm_core::CancellationToken>>| {
                let mut opts = executor_options.clone();
                opts.session_id = session_id.to_string();
                opts.actor_id = actor_id.map(str::to_string);
                opts.project_id = project_id.map(str::to_string);
                opts.cancellation = cancellation;
                opts.grants =
                    CapabilityGrants::default().allow_many(grants.names().map(str::to_string));
                if matches!(approval_mode, NativeApprovalMode::Manual) {
                    let sink: Arc<dyn CodingEventSink> = Arc::new(RosterCodingEventSink::new(
                        session_id,
                        Arc::clone(&executor_approval_roster),
                    ));
                    opts.approval_policy = Arc::new(
                        HttpApprovalPolicy::new(
                            Arc::clone(&executor_approval_broker),
                            session_id,
                            sink,
                        )
                        .with_actor_id(actor_id.map(str::to_string)),
                    );
                }
                Arc::new(TmSandbox::new(opts)) as Arc<dyn Sandbox>
            },
            Some(executor_artifact_root),
            executor_roster,
        ));
    roster.set_executor(executor);

    let chat = AgentChatRunner::tm(Arc::clone(&llm), cfg.clone(), sandbox_options.clone());
    tracing::info!(
        model,
        sandbox_backend = "tm",
        "real LLM agent runner configured"
    );
    Ok(BuiltRuntime {
        chat: Arc::new(ServerChatRunner::Agent(chat)),
        native_tm: Some(NativeTmBackendConfig {
            llm,
            cfg,
            sandbox_options,
            approval_mode,
        }),
        mcp,
        egress_admin,
        linked_aliases,
        linked_resource_handler,
    })
}

/// Install a trusted local policy-reload seam. SIGHUP never exposes a network admin endpoint:
/// it rereads the operator-owned host config, validates the complete replacement, and swaps it
/// atomically. A failed load leaves the previous generation active.
#[cfg(unix)]
pub(super) fn start_egress_policy_reload(admin: EgressAdmin) -> Result<(), BoxError> {
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    tokio::spawn(async move {
        while signal.recv().await.is_some() {
            let config = match load_host_config() {
                Ok(config) => config.egress,
                Err(error) => {
                    tracing::error!(error = %error, "rejected SIGHUP egress policy reload");
                    continue;
                }
            };
            match admin.replace(config).await {
                Ok(()) => {
                    let generation = admin.policy_generation().await;
                    tracing::info!(generation, "reloaded egress policy after SIGHUP");
                }
                Err(error) => {
                    tracing::error!(error = %error, "rejected SIGHUP egress policy replacement");
                }
            }
        }
    });
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn start_egress_policy_reload(_admin: EgressAdmin) -> Result<(), BoxError> {
    tracing::warn!("live egress policy reload is unavailable on this platform");
    Ok(())
}

#[derive(Debug)]
struct CatalogAuditSink;

#[async_trait::async_trait]
impl HostEventSink for CatalogAuditSink {
    async fn emit(&self, event_type: &str, payload_json: serde_json::Value) -> tm_host::Result<()> {
        // Discovery runs before an application store exists. Keep its host-owned audit bounded
        // and explicit; all user-session MCP/P9 events use the durable per-turn sink.
        tracing::info!(event_type, payload = %payload_json, "MCP catalog host audit");
        Ok(())
    }
}

async fn build_mcp_runtime(
    config: &McpRuntimeConfig,
    egress: EgressRuntime,
) -> Result<Option<BuiltMcpRuntime>, BoxError> {
    if !config.enabled {
        return Ok(None);
    }
    let bounds = McpBounds::default();
    config.validate(&bounds)?;
    let http_servers = config.http_servers();
    let transport = Arc::new(EgressMcpTransport::new(
        egress,
        http_servers.clone(),
        McpHttpTransportBounds::default(),
    )?);
    let mut grants = CapabilityGrants::default();
    for server in &http_servers {
        grants = grants.allow(format!("egress.destination:{}", server.destination_id));
        if let Some(secret_id) = &server.secret_id {
            grants = grants.allow(format!("secrets.use:{secret_id}"));
        }
    }
    let catalog_context = McpCatalogContext::new(
        InvocationCtx::new(grants)
            .with_session_id("mcp-catalog-host")
            .with_event_sink(Arc::new(CatalogAuditSink)),
    )?;
    let manager = McpCatalogManager::new(transport, bounds, catalog_context)?;
    let report = manager.reload(&config.specs()).await?;
    let catalog = manager.catalog();
    let bindings = manager.bindings()?;
    tracing::info!(
        generation = report.generation,
        digest = %report.digest,
        servers = report.servers,
        tools = report.tools,
        resources = report.resources,
        prompts = report.prompts,
        "activated immutable MCP startup catalog"
    );
    Ok(Some(BuiltMcpRuntime {
        catalog,
        bindings,
        http_servers,
    }))
}

pub(super) fn configure_coding_backend<S, M, C>(
    mut state: AppState<S, M, C>,
    linked_folders: &LinkedFolders,
    artifact_root: PathBuf,
    native_tm: Option<NativeTmBackendConfig>,
    remote_linked_aliases: &[String],
    remote_linked_resource_handler: Option<Arc<dyn tm_host::ResourceHandler>>,
) -> Result<AppState<S, M, C>, BoxError> {
    let linked_folders = linked_folders
        .clone()
        .with_virtual_aliases(remote_linked_aliases.iter().cloned())?;
    state = state
        .with_artifact_root(artifact_root)
        .with_linked_folders(linked_folders);
    if let Some(handler) = remote_linked_resource_handler {
        state = state.with_linked_resource_handler(handler);
    }
    if std::env::var("TM_OMP_ACP_ENABLED").ok().as_deref() == Some("1") {
        let backend: Arc<dyn CodingBackend> = Arc::new(OmpAcpBackend::new(
            OmpAcpConfig::from_env()?,
            Arc::clone(&state.approval_broker),
        )?);
        state = state.with_coding_backend(backend);
    } else if let Some(native_tm) = native_tm {
        let approval_broker = Arc::clone(&state.approval_broker);
        let backend: Arc<dyn CodingBackend> = Arc::new(NativeTmBackend::new(
            native_tm.llm,
            native_tm.cfg,
            native_tm.sandbox_options,
            native_tm.approval_mode,
            approval_broker,
        ));
        state = state.with_coding_backend(backend);
    }
    Ok(state)
}

fn chat_approval_policy(config: &P0HostConfig) -> Result<Arc<dyn ApprovalPolicy>, BoxError> {
    match config.approvals.mode.as_str() {
        "deny" | "" | "manual" => Ok(Arc::new(DefaultDenyApprovalPolicy)),
        other => Err(format!("unsupported approval mode {other}").into()),
    }
}

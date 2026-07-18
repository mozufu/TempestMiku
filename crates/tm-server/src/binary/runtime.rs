use std::{path::PathBuf, sync::Arc, time::Duration};

use tm_agents::MailboxRegistry;
use tm_core::{AgentConfig, CellBudget, DEFAULT_SYSTEM_PROMPT, LlmClient, Sandbox};
use tm_host::{
    ApprovalPolicy, CapabilityGrants, DefaultDenyApprovalPolicy, LinkedFolders, P0HostConfig,
};
use tm_lang::{TmSandbox, TmSandboxOptions};
use tm_llm::OpenAiClient;
use tm_modes::ModesConfig;
use tm_server::{
    AgentChatRunner, AppState, ApprovalBroker, ChatActorExecutor, CodingBackend, CodingEventSink,
    EchoChatRunner, HttpApprovalPolicy, NativeApprovalMode, NativeTmBackend, OmpAcpBackend,
    OmpAcpConfig, RosterCodingEventSink, ServerChatRunner,
};

use super::BoxError;

pub(super) struct BuiltRuntime {
    pub(super) chat: Arc<ServerChatRunner>,
    pub(super) native_tm: Option<NativeTmBackendConfig>,
}

pub(super) struct NativeTmBackendConfig {
    llm: Arc<dyn LlmClient>,
    cfg: AgentConfig,
    sandbox_options: TmSandboxOptions,
    approval_mode: NativeApprovalMode,
}

pub(super) fn build_runtime(
    host_config: &P0HostConfig,
    linked_folders: &LinkedFolders,
    persona: &ModesConfig,
    artifact_root: PathBuf,
    drive_store: Option<tm_drive::SharedDriveStore>,
    roster: Arc<MailboxRegistry>,
    approval_broker: Arc<ApprovalBroker>,
) -> Result<BuiltRuntime, BoxError> {
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
        ..TmSandboxOptions::default()
    };
    tm_agents::register(
        &mut sandbox_options.host_registry,
        &mut sandbox_options.resource_registry,
        Arc::clone(&roster),
    );
    sandbox_options
        .resource_registry
        .register(Arc::new(tm_modes::SkillResourceHandler::new(
            persona.clone(),
        )));

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
                  session_scope: Option<&str>,
                  cancellation: Option<Arc<dyn tm_core::CancellationToken>>| {
                let mut opts = executor_options.clone();
                opts.session_id = session_id.to_string();
                opts.actor_id = actor_id.map(str::to_string);
                opts.session_scope = session_scope.map(str::to_string);
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
    })
}

pub(super) fn configure_coding_backend<S, M, C>(
    mut state: AppState<S, M, C>,
    linked_folders: &LinkedFolders,
    artifact_root: PathBuf,
    native_tm: Option<NativeTmBackendConfig>,
) -> Result<AppState<S, M, C>, BoxError> {
    state = state
        .with_artifact_root(artifact_root)
        .with_linked_folders(linked_folders.clone());
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

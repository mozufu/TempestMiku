use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use ipnet::IpNet;
use tm_agents::MailboxRegistry;
use tm_artifacts::{ArtifactStore, default_root};
use tm_core::{AgentConfig, CellBudget, DEFAULT_SYSTEM_PROMPT, LlmClient, Sandbox};
use tm_host::{
    ApprovalPolicy, DefaultDenyApprovalPolicy, FsMode, FsPolicy, LinkedFolders, P0HostConfig,
};
use tm_llm::OpenAiClient;
use tm_modes::ModesConfig;
use tm_sandbox::DenoSandbox;
use tm_sandbox::DenoSandboxOptions;

use tm_server::{
    AgentChatRunner, AppState, ApprovalBroker, AuthConfig, ChatActorExecutor, CodingBackend,
    CodingEventSink, DeviceAuthConfig, EchoChatRunner, ForwardedAuthConfig, HttpApprovalPolicy,
    InMemoryAuthDeviceStore, InMemoryStore, NativeApprovalMode, NativeDenoBackend, OmpAcpBackend,
    OmpAcpConfig, PostgresDriveMetadataStore, PostgresPushStore, PostgresStore, PushCipher,
    PushProvider, PushService, RosterCodingEventSink, RuntimeConfig, RuntimeStatus,
    ServerChatRunner, ServerRole, Store, StoreMemoryProvider, UnifiedPushProvider, run_server,
};

type ConfiguredPush = (Arc<dyn PushProvider>, PushCipher);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let addr: SocketAddr = std::env::var("TM_SERVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8787".to_string())
        .parse()?;
    let database_dsn = std::env::var("TM_DATABASE_URL")
        .ok()
        .filter(|dsn| !dsn.trim().is_empty());
    let role = std::env::var("TM_SERVER_ROLE")
        .unwrap_or_else(|_| "api".to_string())
        .parse::<ServerRole>()?;
    let owner_subject = std::env::var("TM_OWNER_SUBJECT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| "brian".to_string());
    let owner_subject = owner_subject.trim().to_string();
    if owner_subject.len() > 128 || owner_subject.chars().any(char::is_control) {
        return Err("TM_OWNER_SUBJECT must be 1-128 non-control characters".into());
    }
    if role.serves_api() && !addr.ip().is_loopback() && database_dsn.is_none() {
        return Err(
            "non-loopback deployments require TM_DATABASE_URL so device credentials survive restart"
                .into(),
        );
    }
    if role.runs_workers() && database_dsn.is_none() {
        return Err(
            "TM_SERVER_ROLE=worker/all requires TM_DATABASE_URL; embedded workers cannot use process-local state"
                .into(),
        );
    }
    let auth = if role.serves_api() {
        server_auth_config(addr, &owner_subject)?
    } else {
        AuthConfig::NoAuth
    };
    let mut persona = std::env::var_os("TM_MODES_PATH")
        .map(ModesConfig::from_path)
        .unwrap_or_default();
    let host_config = load_host_config()?;
    let linked_folders = host_config.linked_folders()?;
    let artifact_root = server_artifact_root(&host_config);
    let managed_skills_path = std::env::var_os("TM_MANAGED_SKILLS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| artifact_root.join("managed-skills"));
    persona = persona.with_managed_skills_path(managed_skills_path);
    let managed_mode_addenda_path = std::env::var_os("TM_MANAGED_MODE_ADDENDA_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| artifact_root.join("managed-mode-addenda"));
    persona = persona.with_managed_mode_addenda_path(managed_mode_addenda_path);
    let roster = Arc::new(MailboxRegistry::new());
    let approval_broker = Arc::new(ApprovalBroker::default());
    let push_config = push_config_from_env()?;

    if let Some(dsn) = database_dsn {
        let store = Arc::new(PostgresStore::connect(&dsn).await?);
        let drive_metadata = PostgresDriveMetadataStore::connect(&dsn).await?;
        let drive_store: tm_drive::SharedDriveStore =
            Arc::new(tm_drive::DriveService::with_metadata(
                ArtifactStore::open(&artifact_root, "drive")?,
                drive_metadata,
            ));
        let link_hydration_failures = hydrate_drive_links(&drive_store, &linked_folders).await?;
        let runtime = build_runtime(
            &host_config,
            &linked_folders,
            &persona,
            artifact_root.clone(),
            Some(drive_store.clone()),
            Arc::clone(&roster),
            Arc::clone(&approval_broker),
        )?;
        let backfilled = store.configure_owner_subject(&owner_subject).await?;
        tracing::info!(%owner_subject, backfilled_sessions = backfilled, "configured server owner authority");
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let runtime_status = Arc::new(RuntimeStatus::new(role, true, true));
        runtime_status.add_link_hydration_failures(link_hydration_failures as u64);
        let mut state = AppState::new(store.clone(), memory, runtime.chat, persona, auth.clone())
            .with_auth_store(store)
            .with_approval_broker(Arc::clone(&approval_broker))
            .with_actor_roster(Arc::clone(&roster))
            .with_drive_store(drive_store.clone())
            .with_self_evolution_tier(host_config.self_evolution.tier)
            .with_runtime_status(runtime_status)
            .with_auto_turn_dispatcher(false);
        if let Some((provider, cipher)) = &push_config {
            let push_store = Arc::new(PostgresPushStore::connect(&dsn).await?);
            state = state.with_push_service(Arc::new(PushService::new(
                push_store,
                Arc::clone(provider),
                cipher.clone(),
            )));
        }
        let state = configure_coding_backend(
            state,
            &linked_folders,
            artifact_root.clone(),
            runtime.native_deno,
        )?;
        state.wire_lifecycle_sink();
        run_server(addr, state, role, RuntimeConfig::default()).await?;
    } else {
        tracing::warn!(
            "TM_DATABASE_URL not set; using non-durable in-memory server and drive metadata stores for local development"
        );
        let drive_store: tm_drive::SharedDriveStore = Arc::new(tm_drive::InMemoryDriveStore::new(
            ArtifactStore::open(&artifact_root, "drive")?,
        ));
        let runtime = build_runtime(
            &host_config,
            &linked_folders,
            &persona,
            artifact_root.clone(),
            Some(drive_store.clone()),
            Arc::clone(&roster),
            Arc::clone(&approval_broker),
        )?;
        let store = Arc::new(InMemoryStore::default());
        store.configure_owner_subject(&owner_subject).await?;
        let auth_store = Arc::new(InMemoryAuthDeviceStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let mut state = AppState::new(store, memory, runtime.chat, persona, auth)
            .with_auth_store(auth_store)
            .with_approval_broker(Arc::clone(&approval_broker))
            .with_actor_roster(roster)
            .with_drive_store(drive_store)
            .with_self_evolution_tier(host_config.self_evolution.tier)
            .with_runtime_status(Arc::new(RuntimeStatus::new(role, false, false)))
            .with_auto_turn_dispatcher(false);
        if let Some((provider, cipher)) = &push_config {
            state = state.with_push_service(Arc::new(PushService::new(
                Arc::new(tm_server::InMemoryPushStore::default()),
                Arc::clone(provider),
                cipher.clone(),
            )));
        }
        let state = configure_coding_backend(
            state,
            &linked_folders,
            artifact_root.clone(),
            runtime.native_deno,
        )?;
        state.wire_lifecycle_sink();
        run_server(addr, state, role, RuntimeConfig::default()).await?;
    }

    Ok(())
}

fn push_config_from_env() -> Result<Option<ConfiguredPush>, Box<dyn std::error::Error>> {
    let provider = std::env::var("TM_PUSH_PROVIDER").unwrap_or_else(|_| "disabled".to_string());
    match provider.trim().to_ascii_lowercase().as_str() {
        "" | "disabled" | "none" => Ok(None),
        "fake" if cfg!(debug_assertions) => {
            let key = required_env("TM_PUSH_ENCRYPTION_KEY")?;
            Ok(Some((
                Arc::new(tm_server::FakePushProvider::default()),
                PushCipher::from_base64(&key)?,
            )))
        }
        "fake" => Err("TM_PUSH_PROVIDER=fake is unavailable in release builds".into()),
        "unifiedpush" => {
            let key = required_env("TM_PUSH_ENCRYPTION_KEY")?;
            let endpoint_origin = required_env("TM_UNIFIED_PUSH_ENDPOINT_ORIGIN")?;
            Ok(Some((
                Arc::new(UnifiedPushProvider::new(&endpoint_origin)?),
                PushCipher::from_base64(&key)?,
            )))
        }
        other => Err(format!(
            "unsupported TM_PUSH_PROVIDER={other}; expected disabled or unifiedpush"
        )
        .into()),
    }
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
    persona: &ModesConfig,
    artifact_root: PathBuf,
    drive_store: Option<tm_drive::SharedDriveStore>,
    roster: Arc<MailboxRegistry>,
    approval_broker: Arc<ApprovalBroker>,
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
    let mut sandbox_options = DenoSandboxOptions {
        artifact_root,
        linked_folders,
        drive_store,
        approval_policy: chat_approval_policy(host_config)?,
        approval_timeout: Duration::from_millis(host_config.approvals.timeout_ms),
        ..DenoSandboxOptions::default()
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
                opts.grants = tm_sandbox::core_sandbox_grants()
                    .allow_many(grants.names().map(str::to_string));
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
                Arc::new(DenoSandbox::new(opts)) as Arc<dyn Sandbox>
            },
            Some(executor_artifact_root),
            executor_roster,
        ));
    roster.set_executor(executor);

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
            self_evolution: Default::default(),
        }),
    }
}

fn server_artifact_root(host_config: &P0HostConfig) -> PathBuf {
    std::env::var_os("TM_OMP_ACP_ARTIFACT_ROOT")
        .map(PathBuf::from)
        .or_else(|| host_config.artifact_root.clone())
        .unwrap_or_else(default_root)
}

async fn hydrate_drive_links(
    drive_store: &tm_drive::SharedDriveStore,
    linked_folders: &LinkedFolders,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut failures = 0;
    for link in drive_store.links().await? {
        if link.status != tm_drive::DriveLinkStatus::Active {
            let _ = linked_folders.remove_policy(&link.alias);
            continue;
        }
        let stored_root = PathBuf::from(&link.canonical_root);
        let validation = (|| -> Result<FsPolicy, String> {
            let metadata = std::fs::symlink_metadata(&stored_root)
                .map_err(|err| format!("linked root is unavailable: {err}"))?;
            if metadata.file_type().is_symlink() {
                return Err("linked root was replaced by a symlink".to_string());
            }
            let canonical = stored_root
                .canonicalize()
                .map_err(|err| format!("linked root cannot be canonicalized: {err}"))?;
            if canonical != stored_root {
                return Err(format!(
                    "linked root canonical identity changed from {} to {}",
                    stored_root.display(),
                    canonical.display()
                ));
            }
            if !canonical.is_dir() {
                return Err("linked root is no longer a directory".to_string());
            }
            let mode = match link.mode.as_str() {
                "ro" => FsMode::Ro,
                "rw" => FsMode::Rw,
                other => return Err(format!("persisted linked root has invalid mode {other}")),
            };
            Ok(FsPolicy {
                alias: link.alias.clone(),
                root: canonical,
                mode,
                commands: BTreeSet::new(),
                safe_args: Vec::new(),
            })
        })()
        .and_then(|policy| {
            linked_folders
                .insert_policy(policy)
                .map_err(|err| err.to_string())
        });
        if let Err(reason) = validation {
            failures += 1;
            let _ = linked_folders.remove_policy(&link.alias);
            drive_store.invalidate_link(&link.alias, &reason).await?;
            let alias = tm_memory::redact_dream_text(&link.alias).text;
            let reason = tm_memory::redact_dream_text(&reason).text;
            tracing::warn!(%alias, %reason, "disabled invalid persisted drive link");
        }
    }
    Ok(failures)
}

fn chat_approval_policy(
    config: &P0HostConfig,
) -> Result<Arc<dyn ApprovalPolicy>, Box<dyn std::error::Error>> {
    match config.approvals.mode.as_str() {
        "deny" | "" | "manual" => Ok(Arc::new(DefaultDenyApprovalPolicy)),
        other => Err(format!("unsupported approval mode {other}").into()),
    }
}

fn server_auth_config(
    addr: SocketAddr,
    owner_subject: &str,
) -> Result<AuthConfig, Box<dyn std::error::Error>> {
    let mode = std::env::var("TM_AUTH_MODE").unwrap_or_else(|_| "device".to_string());
    let public_url = std::env::var("TM_PUBLIC_BASE_URL").ok();
    let allow_insecure = env_flag("TM_ALLOW_INSECURE_HTTP");
    if allow_insecure && !cfg!(debug_assertions) {
        return Err("TM_ALLOW_INSECURE_HTTP is available only in debug builds".into());
    }
    let allowed_origin = public_url.as_deref().map(public_origin).transpose()?;
    validate_bind_security(addr, allow_insecure)?;

    match mode.trim().to_ascii_lowercase().as_str() {
        "device" | "" => Ok(AuthConfig::Device(DeviceAuthConfig {
            cookie_name: tm_server::auth::DEFAULT_DEVICE_COOKIE.to_string(),
            secure_cookie: public_url
                .as_deref()
                .is_some_and(|url| url.trim().starts_with("https://")),
            owner_subject: owner_subject.to_string(),
            bootstrap_token_hash: std::env::var("TM_AUTH_BOOTSTRAP_TOKEN")
                .ok()
                .filter(|token| !token.trim().is_empty())
                .map(|token| tm_server::auth::hash_secret(token.trim())),
            allow_loopback_pairing: addr.ip().is_loopback(),
            allowed_origin,
        })),
        "bearer" => {
            let token = required_env("TM_AUTH_TOKEN")?;
            Ok(AuthConfig::BearerToken(token))
        }
        "forwarded" => {
            let user_header = required_env("TM_AUTH_FORWARDED_USER_HEADER")?;
            let trusted_proxy_networks = std::env::var("TM_AUTH_TRUSTED_PROXY_CIDRS")
                .or_else(|_| std::env::var("TM_AUTH_TRUSTED_PROXY_IPS"))
                .map_err(|_| {
                    "TM_AUTH_TRUSTED_PROXY_CIDRS is required for forwarded auth (TM_AUTH_TRUSTED_PROXY_IPS is a legacy alias)"
                })?;
            let trusted_proxy_cidrs = trusted_proxy_networks
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| {
                    if let Ok(network) = value.parse::<IpNet>() {
                        Ok(network)
                    } else {
                        value
                            .parse::<IpAddr>()
                            .map(IpNet::from)
                            .map_err(|error| format!("invalid trusted proxy CIDR {value}: {error}"))
                    }
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if trusted_proxy_cidrs.is_empty() {
                return Err("TM_AUTH_TRUSTED_PROXY_CIDRS must contain at least one network".into());
            }
            let expected_user = std::env::var("TM_AUTH_FORWARDED_EXPECTED_USER")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| owner_subject.to_string());
            if expected_user != owner_subject {
                return Err("TM_AUTH_FORWARDED_EXPECTED_USER must match TM_OWNER_SUBJECT".into());
            }
            Ok(AuthConfig::Forwarded(ForwardedAuthConfig {
                user_header,
                expected_user: Some(expected_user),
                trusted_proxy_cidrs,
            }))
        }
        "no_auth" | "none" => {
            if !addr.ip().is_loopback() {
                return Err("TM_AUTH_MODE=no_auth is restricted to loopback binds".into());
            }
            Ok(AuthConfig::NoAuth)
        }
        other => Err(format!("unsupported TM_AUTH_MODE {other}").into()),
    }
}

fn validate_bind_security(
    addr: SocketAddr,
    allow_insecure: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !addr.ip().is_loopback() && !allow_insecure {
        return Err(
            "tm-server serves plain HTTP and must bind to loopback behind an HTTPS proxy or Tailscale Serve; TM_PUBLIC_BASE_URL does not secure a non-loopback bind (TM_ALLOW_INSECURE_HTTP=1 is debug-only)"
                .into(),
        );
    }
    Ok(())
}

fn required_env(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} is required").into())
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn public_origin(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let (scheme, rest) = url
        .trim()
        .split_once("://")
        .ok_or("TM_PUBLIC_BASE_URL must include http:// or https://")?;
    if !matches!(scheme, "http" | "https") {
        return Err("TM_PUBLIC_BASE_URL must use http or https".into());
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err("TM_PUBLIC_BASE_URL must include a host and no userinfo".into());
    }
    Ok(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_host::LinkedFolderConfig;

    #[test]
    fn raw_http_bind_is_loopback_only_without_debug_override() {
        assert!(validate_bind_security("127.0.0.1:8787".parse().unwrap(), false).is_ok());
        let error = validate_bind_security("0.0.0.0:8787".parse().unwrap(), false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("must bind to loopback"));
        assert!(error.contains("TM_PUBLIC_BASE_URL does not secure"));
        assert!(validate_bind_security("0.0.0.0:8787".parse().unwrap(), true).is_ok());
    }

    #[tokio::test]
    async fn durable_link_tombstones_override_static_config_on_restart() {
        let artifacts = tempfile::tempdir().unwrap();
        let revoked_root = tempfile::tempdir().unwrap();
        let invalid_root = tempfile::tempdir().unwrap();
        let drive: tm_drive::SharedDriveStore = Arc::new(tm_drive::InMemoryDriveStore::new(
            ArtifactStore::open(artifacts.path(), "drive").unwrap(),
        ));
        let revoked =
            tm_drive::drive_link_plan(revoked_root.path(), FsMode::Ro, Some("revoked-project"))
                .unwrap();
        let invalid =
            tm_drive::drive_link_plan(invalid_root.path(), FsMode::Ro, Some("invalid-project"))
                .unwrap();
        drive.record_link(&revoked).await.unwrap();
        drive.revoke_link(&revoked.alias).await.unwrap();
        drive.record_link(&invalid).await.unwrap();
        drive
            .invalidate_link(&invalid.alias, "test invalidation")
            .await
            .unwrap();
        let linked = LinkedFolders::from_configs(vec![
            LinkedFolderConfig {
                name: revoked.alias.clone(),
                path: revoked_root.path().to_path_buf(),
                mode: FsMode::Ro,
                commands: Vec::new(),
                safe_args: Vec::new(),
            },
            LinkedFolderConfig {
                name: invalid.alias.clone(),
                path: invalid_root.path().to_path_buf(),
                mode: FsMode::Ro,
                commands: Vec::new(),
                safe_args: Vec::new(),
            },
        ])
        .unwrap();

        hydrate_drive_links(&drive, &linked).await.unwrap();
        assert!(linked.policy(&revoked.alias).is_err());
        assert!(linked.policy(&invalid.alias).is_err());
    }
}

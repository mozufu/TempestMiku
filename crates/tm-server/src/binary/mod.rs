mod auth;
mod config;
mod drive_links;
mod embedding;
mod push;
mod runtime;

use std::sync::Arc;

use tm_agents::MailboxRegistry;
use tm_artifacts::ArtifactStore;
use tm_server::{
    AppState, ApprovalBroker, AuthConfig, InMemoryAuthDeviceStore, InMemoryStore,
    PostgresDriveMetadataStore, PostgresMemoryEmbeddingWorker, PostgresPushStore, PostgresStore,
    PushService, RuntimeConfig, RuntimeStatus, SelfHostedAsr, Store, StoreMemoryProvider,
    run_server,
};

use self::{
    auth::server_auth_config,
    config::{
        apply_managed_persona_paths, database_dsn_from_env, load_host_config, load_mcp_config,
        modes_config_from_env, owner_subject_from_env, self_hosted_asr_config_from_env,
        server_addr_from_env, server_artifact_root, server_role_from_env,
    },
    drive_links::hydrate_drive_links,
    embedding::embedding_setup_from_env,
    push::push_config_from_env,
    runtime::{
        RuntimePolicies, build_runtime, configure_coding_backend, start_egress_policy_reload,
    },
};

pub(super) type BoxError = Box<dyn std::error::Error>;

pub(super) async fn run() -> Result<(), BoxError> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let addr = server_addr_from_env()?;
    let database_dsn = database_dsn_from_env();
    let embedding_setup = embedding_setup_from_env()?;
    if embedding_setup.config.is_enabled() && database_dsn.is_none() {
        return Err("enabled memory embeddings require TM_DATABASE_URL".into());
    }
    let role = server_role_from_env()?;
    let owner_subject = owner_subject_from_env()?;
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
    let persona = modes_config_from_env();
    let host_config = load_host_config()?;
    recover_proc_isolation_orphans_at_startup(&host_config)?;
    let mcp_config = load_mcp_config(&host_config)?;
    let linked_folders = host_config.linked_folders()?;
    let artifact_root = server_artifact_root(&host_config);
    let persona = apply_managed_persona_paths(persona, &artifact_root);
    let roster = Arc::new(MailboxRegistry::new());
    let approval_broker = Arc::new(ApprovalBroker::default());
    let push_config = push_config_from_env()?;
    let self_hosted_asr = self_hosted_asr_config_from_env()?
        .map(SelfHostedAsr::new)
        .transpose()?
        .map(Arc::new);

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
            RuntimePolicies {
                host: &host_config,
                mcp: &mcp_config,
            },
            &linked_folders,
            &persona,
            artifact_root.clone(),
            Some(drive_store.clone()),
            Arc::clone(&roster),
            Arc::clone(&approval_broker),
        )
        .await?;
        start_egress_policy_reload(runtime.egress_admin.clone())?;
        let backfilled = store.configure_owner_subject(&owner_subject).await?;
        tracing::info!(%owner_subject, backfilled_sessions = backfilled, "configured server owner authority");
        let memory_readiness = store.memory_readiness(&embedding_setup.config).await?;
        if !memory_readiness.allows_durable_writes() {
            let error: BoxError = Box::new(tm_server::ServerError::Policy(format!(
                "durable P8 memory schema is not ready: {:?}",
                memory_readiness.schema
            )));
            return Err(error);
        }
        tracing::info!(
            ?memory_readiness,
            "configured durable P8 memory storage state"
        );
        let memory = match &embedding_setup.client {
            Some(client) => Arc::new(
                StoreMemoryProvider::new(store.clone())
                    .with_embeddings(embedding_setup.config.clone(), Arc::clone(client))?,
            ),
            None => Arc::new(StoreMemoryProvider::new(store.clone())),
        };
        let runtime_status =
            Arc::new(RuntimeStatus::new(role, true, true).with_memory_readiness(memory_readiness));
        runtime_status.add_link_hydration_failures(link_hydration_failures as u64);
        let mut state = AppState::new(
            store.clone(),
            memory,
            Arc::clone(&runtime.chat),
            persona,
            auth.clone(),
        )
        .with_auth_store(store.clone())
        .with_approval_broker(Arc::clone(&approval_broker))
        .with_actor_roster(Arc::clone(&roster))
        .with_drive_store(drive_store.clone())
        .with_egress_config(&host_config.egress)
        .with_egress_admin(runtime.egress_admin.clone())
        .with_self_evolution_tier(host_config.self_evolution.tier)
        .with_runtime_status(runtime_status)
        .with_auto_turn_dispatcher(false);
        if let Some(mcp) = &runtime.mcp {
            state =
                state.with_mcp_runtime(mcp.catalog.clone(), &mcp.bindings, &mcp.http_servers)?;
        }
        if let Some(service) = &self_hosted_asr {
            state = state.with_self_hosted_asr(Arc::clone(service));
        }
        if let Some(client) = &embedding_setup.client {
            state =
                state.with_memory_embedding_worker(Arc::new(PostgresMemoryEmbeddingWorker::new(
                    store.clone(),
                    Arc::clone(client),
                    embedding_setup.config.clone(),
                    owner_subject.clone(),
                )?));
        }
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
            runtime.native_tm,
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
            RuntimePolicies {
                host: &host_config,
                mcp: &mcp_config,
            },
            &linked_folders,
            &persona,
            artifact_root.clone(),
            Some(drive_store.clone()),
            Arc::clone(&roster),
            Arc::clone(&approval_broker),
        )
        .await?;
        start_egress_policy_reload(runtime.egress_admin.clone())?;
        let store = Arc::new(InMemoryStore::default());
        store.configure_owner_subject(&owner_subject).await?;
        let auth_store = Arc::new(InMemoryAuthDeviceStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let mut state = AppState::new(store, memory, Arc::clone(&runtime.chat), persona, auth)
            .with_auth_store(auth_store)
            .with_approval_broker(Arc::clone(&approval_broker))
            .with_actor_roster(roster)
            .with_drive_store(drive_store)
            .with_egress_config(&host_config.egress)
            .with_egress_admin(runtime.egress_admin.clone())
            .with_self_evolution_tier(host_config.self_evolution.tier)
            .with_runtime_status(Arc::new(RuntimeStatus::new(role, false, false)))
            .with_auto_turn_dispatcher(false);
        if let Some(mcp) = &runtime.mcp {
            state =
                state.with_mcp_runtime(mcp.catalog.clone(), &mcp.bindings, &mcp.http_servers)?;
        }
        if let Some(service) = &self_hosted_asr {
            state = state.with_self_hosted_asr(Arc::clone(service));
        }
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
            runtime.native_tm,
        )?;
        state.wire_lifecycle_sink();
        run_server(addr, state, role, RuntimeConfig::default()).await?;
    }

    Ok(())
}

fn recover_proc_isolation_orphans_at_startup(
    host_config: &tm_host::P0HostConfig,
) -> Result<(), BoxError> {
    if !host_config
        .proc_isolation
        .requires_startup_orphan_recovery()
    {
        return Ok(());
    }

    // This runs before either the API or worker runtime is constructed, so no proc.run can race
    // recovery in this service instance. Deployment is responsible for assigning an exclusive
    // delegated subtree to each concurrently running instance.
    let report = host_config.proc_isolation.recover_orphans_at_startup()?;
    tracing::info!(
        provider = %report.provider,
        cgroup_root = %report.cgroup_root.display(),
        recovered_leaves = report.recovered.len(),
        "completed proc isolation startup orphan recovery"
    );
    Ok(())
}

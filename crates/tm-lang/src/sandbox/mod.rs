use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use tm_artifacts::ArtifactStore;
use tm_core::{CancellationToken, Result, Sandbox, Session, SessionConfig};
use tm_drive::SharedDriveStore;
use tm_host::{
    ApprovalPolicy, ArtifactResourceHandler, CapabilityGrants, DefaultDenyApprovalPolicy,
    HostEventSink, HostRegistry, InvocationCtx, LinkedFolders, NoopHostEventSink, ResourceRegistry,
    register_p0_linked_folder_functions,
};

use crate::{Interpreter, RuntimeLimits, catalog_from_registry};

#[derive(Clone)]
pub struct TmSandboxOptions {
    pub artifact_root: PathBuf,
    pub session_id: String,
    pub actor_id: Option<String>,
    pub session_scope: Option<String>,
    pub http_allowlist: BTreeMap<String, String>,
    pub host_registry: HostRegistry,
    pub resource_registry: ResourceRegistry,
    pub grants: CapabilityGrants,
    pub linked_folders: Option<LinkedFolders>,
    pub drive_store: Option<SharedDriveStore>,
    pub approval_policy: Arc<dyn ApprovalPolicy>,
    pub approval_timeout: Duration,
    pub proc_run_timeout: Duration,
    pub host_event_sink: Arc<dyn HostEventSink>,
    pub limits: RuntimeLimits,
    pub cancellation: Option<Arc<dyn CancellationToken>>,
}

impl Default for TmSandboxOptions {
    fn default() -> Self {
        Self {
            artifact_root: tm_artifacts::default_root(),
            session_id: "default".into(),
            actor_id: None,
            session_scope: None,
            http_allowlist: BTreeMap::new(),
            host_registry: HostRegistry::new(),
            resource_registry: ResourceRegistry::new(),
            // Registration and sandbox construction never grant model authority. Callers must
            // supply every externally authoritative capability for this exact turn/actor.
            grants: CapabilityGrants::default(),
            linked_folders: None,
            drive_store: None,
            approval_policy: Arc::new(DefaultDenyApprovalPolicy),
            approval_timeout: Duration::from_secs(60),
            proc_run_timeout: Duration::from_millis(180_000),
            host_event_sink: Arc::new(NoopHostEventSink),
            limits: RuntimeLimits::default(),
            cancellation: None,
        }
    }
}

#[derive(Clone)]
pub struct TmSandbox {
    options: TmSandboxOptions,
}
impl TmSandbox {
    pub fn new(options: TmSandboxOptions) -> Self {
        Self { options }
    }
}

#[async_trait]
impl Sandbox for TmSandbox {
    async fn open(&self, _cfg: SessionConfig) -> Result<Box<dyn Session>> {
        let artifact_root = self.options.artifact_root.clone();
        let artifact_session_id = self.options.session_id.clone();
        let artifacts = tokio::task::spawn_blocking(move || {
            ArtifactStore::open(artifact_root, artifact_session_id)
        })
        .await
        .map_err(|error| tm_core::Error::Sandbox(format!("artifact open task failed: {error}")))?
        .map_err(|error| tm_core::Error::Sandbox(error.to_string()))?;
        let mut registry = self.options.host_registry.clone();
        registry.register(Arc::new(HttpGetFn::new(
            self.options.http_allowlist.clone(),
        )));
        let mut resources = self.options.resource_registry.clone();
        resources.register(Arc::new(ArtifactResourceHandler::new(artifacts.clone())));
        let linked_folders = self.options.linked_folders.clone().or_else(|| {
            self.options
                .drive_store
                .as_ref()
                .map(|_| LinkedFolders::default())
        });
        if let Some(linked_folders) = linked_folders.clone() {
            register_p0_linked_folder_functions(
                &mut registry,
                &mut resources,
                linked_folders,
                artifacts.clone(),
                self.options.proc_run_timeout,
            );
        }
        if let Some(drive_store) = self.options.drive_store.clone() {
            tm_drive::register_drive_functions(
                &mut registry,
                &mut resources,
                drive_store,
                linked_folders.clone(),
            );
        }
        let mut invocation = InvocationCtx::with_approvals(
            self.options.grants.clone(),
            self.options.approval_policy.clone(),
            self.options.approval_timeout,
        )
        .with_session_id(self.options.session_id.clone())
        .with_actor_id(self.options.actor_id.clone())
        .with_event_sink(self.options.host_event_sink.clone());
        if let Some(scope) = self.options.session_scope.clone() {
            invocation = invocation.with_session_scope(scope);
        }
        let resource_schemes = resources
            .capabilities()
            .into_iter()
            .filter_map(|(scheme, capability)| {
                invocation.grants.permits(&capability).then_some(scheme)
            })
            .collect::<Vec<_>>();
        if !resource_schemes.is_empty() {
            invocation.grants = invocation.grants.clone().allow_many([
                "resources.read",
                "resources.preview",
                "resources.list",
            ]);
        }
        let mut catalog_schemes = resource_schemes;
        // `alias:path` is tm's compact linked-folder literal. It is not a ResourceRegistry
        // scheme, so admit only aliases that this server-authoritative session scope can use.
        if let Some(linked_folders) = &linked_folders {
            catalog_schemes.extend(
                linked_folders
                    .policies()
                    .into_iter()
                    .map(|policy| policy.alias)
                    .filter(|alias| invocation.require_linked_alias(alias).is_ok()),
            );
        }
        catalog_schemes.sort();
        catalog_schemes.dedup();
        for operation in [
            ResourceOperation::Read,
            ResourceOperation::Preview,
            ResourceOperation::List,
        ] {
            registry.register(Arc::new(ResourceFn::new(operation, resources.clone())));
        }
        for operation in [
            ArtifactOperation::Put,
            ArtifactOperation::Get,
            ArtifactOperation::Slice,
            ArtifactOperation::List,
        ] {
            registry.register(Arc::new(ArtifactFn::new(operation, artifacts.clone())));
        }
        // Session-local output spilling and granted-catalog inspection are intrinsic tm runtime
        // operations. They do not add host, resource-read, network, or child authority.
        invocation.grants =
            invocation
                .grants
                .clone()
                .allow_many(["artifacts.put", "tools.search", "tools.docs"]);
        if invocation.grants.permits("resources.read:artifact") {
            invocation.grants = invocation.grants.clone().allow_many([
                "artifacts.get",
                "artifacts.slice",
                "artifacts.list",
            ]);
            if !catalog_schemes.iter().any(|scheme| scheme == "artifact") {
                catalog_schemes.push("artifact".into());
            }
        }
        let catalog_registry = Arc::new(registry.clone());
        for operation in [CatalogOperation::Search, CatalogOperation::Docs] {
            registry.register(Arc::new(CatalogFn::new(
                operation,
                Arc::clone(&catalog_registry),
            )));
        }
        let registry = Arc::new(registry);
        let catalog = catalog_from_registry(&registry, &invocation, catalog_schemes);
        Ok(Box::new(TmSession {
            interpreter: Interpreter::new(
                catalog,
                registry,
                invocation,
                self.options.limits.clone(),
            ),
            cancellation: self.options.cancellation.clone(),
            limits: self.options.limits.clone(),
        }))
    }
}

mod adapters;
mod session;

use adapters::{
    ArtifactFn, ArtifactOperation, CatalogFn, CatalogOperation, HttpGetFn, ResourceFn,
    ResourceOperation,
};
use session::TmSession;

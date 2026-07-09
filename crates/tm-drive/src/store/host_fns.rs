use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use tm_host::{
    HostError, HostFn, HostRegistry, InvocationCtx, LinkedFolders, ResourceRegistry, ToolDocs,
};

use super::{
    core::{
        drive_error_to_host, drive_put_requires_approval, host_drive_put_options,
        linked_alias_from_target, normalize_canonical_path, validate_host_organizer_config,
    },
    docs::{content_to_bytes, drive_docs},
    payloads::{
        drive_entry_event_payload, drive_linked_payload, drive_moved_payload,
        drive_unlinked_payload, drive_write_proposal_payload, organizer_completed_payload,
        organizer_failed_payload, organizer_failed_payload_with_proposals,
        organizer_started_payload,
    },
    types::InMemoryDriveStore,
};
use crate::{
    DriveCollisionStrategy, DriveListOptions, DriveOrganizerConfig, DrivePutOptions,
    DriveSearchOptions, DriveUnlinkResult, OrganizerProposal, ProposalStatus, drive_link_policy,
    memory_scope_for_project, resources::DriveResourceHandler,
};

pub fn register_drive_functions(
    host_registry: &mut HostRegistry,
    resource_registry: &mut ResourceRegistry,
    store: InMemoryDriveStore,
    linked_folders: Option<LinkedFolders>,
) {
    let linked_for_unlink = linked_folders.clone();
    host_registry.register(Arc::new(DrivePutFn::new(store.clone())));
    host_registry.register(Arc::new(DriveGetFn::new(store.clone())));
    host_registry.register(Arc::new(DriveLsFn::new(store.clone())));
    host_registry.register(Arc::new(DriveMoveFn::new(store.clone())));
    host_registry.register(Arc::new(DriveSearchFn::new(store.clone())));
    host_registry.register(Arc::new(DriveTagFn::new(store.clone())));
    host_registry.register(Arc::new(DriveLinkFn::new(linked_folders)));
    host_registry.register(Arc::new(DriveUnlinkFn::new(linked_for_unlink)));
    host_registry.register(Arc::new(DriveOrganizeFn::new(store.clone())));
    resource_registry.register(Arc::new(DriveResourceHandler::new(store)));
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrivePutArgs {
    content: Value,
    #[serde(default)]
    options: DrivePutOptions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrivePathArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    selector: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveMoveArgs {
    from: String,
    to: String,
    #[serde(default)]
    collision: DriveCollisionStrategy,
    #[serde(default)]
    overwrite: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveTagArgs {
    path: String,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveLinkArgs {
    host_path: String,
    #[serde(default = "default_link_mode")]
    mode: String,
    #[serde(default)]
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveUnlinkArgs {
    alias: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DriveOrganizeArgs {
    #[serde(default)]
    apply: bool,
    #[serde(default)]
    config: DriveOrganizerConfig,
}

fn default_link_mode() -> String {
    "ro".to_string()
}

macro_rules! drive_fn {
    ($name:ident, $cap:literal, $summary:literal, $approval:literal, $sensitive:expr) => {
        pub struct $name {
            docs: ToolDocs,
            store: Option<InMemoryDriveStore>,
        }

        impl $name {
            fn new(store: InMemoryDriveStore) -> Self {
                Self {
                    docs: drive_docs($cap, $summary, $approval, $sensitive),
                    store: Some(store),
                }
            }
        }

        #[async_trait]
        impl HostFn for $name {
            fn docs(&self) -> &ToolDocs {
                &self.docs
            }

            async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
                self.call_drive(args, ctx).await
            }
        }
    };
}

drive_fn!(
    DrivePutFn,
    "drive.put",
    "Store a document in the local-first drive",
    "policy",
    true
);
drive_fn!(
    DriveGetFn,
    "drive.get",
    "Read a drive document by path or drive:// URI",
    "none",
    false
);
drive_fn!(
    DriveLsFn,
    "drive.ls",
    "List canonical drive paths or virtual directories",
    "none",
    false
);
drive_fn!(
    DriveMoveFn,
    "drive.move",
    "Move a filed drive document",
    "on-write",
    true
);
drive_fn!(
    DriveSearchFn,
    "drive.search",
    "Search filed drive documents",
    "none",
    false
);
drive_fn!(
    DriveTagFn,
    "drive.tag",
    "Add tags to a filed drive document",
    "on-write",
    true
);
drive_fn!(
    DriveOrganizeFn,
    "drive.organize",
    "Generate organizer proposals for filed documents",
    "policy",
    true
);

pub struct DriveLinkFn {
    docs: ToolDocs,
    linked_folders: Option<LinkedFolders>,
}

impl DriveLinkFn {
    fn new(linked_folders: Option<LinkedFolders>) -> Self {
        Self {
            docs: drive_docs(
                "drive.link",
                "Register an approval-gated linked folder plus project memory scope",
                "always",
                true,
            ),
            linked_folders,
        }
    }
}

#[async_trait]
impl HostFn for DriveLinkFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveLinkArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        ctx.require_approval(&format!("drive.link {}", args.host_path))
            .await?;
        let mode = match args.mode.as_str() {
            "ro" => tm_host::FsMode::Ro,
            "rw" => tm_host::FsMode::Rw,
            other => {
                return Err(HostError::InvalidArgs(format!(
                    "drive.link mode must be ro or rw, got {other}"
                )));
            }
        };
        let (plan, policy) = drive_link_policy(&args.host_path, mode, args.project.as_deref())
            .map_err(drive_error_to_host)?;
        if let Some(linked_folders) = &self.linked_folders {
            linked_folders.insert_policy(policy)?;
        }
        ctx.emit_event("drive_linked", drive_linked_payload(&plan))
            .await?;
        serde_json::to_value(plan).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

pub struct DriveUnlinkFn {
    docs: ToolDocs,
    linked_folders: Option<LinkedFolders>,
}

impl DriveUnlinkFn {
    fn new(linked_folders: Option<LinkedFolders>) -> Self {
        Self {
            docs: drive_docs(
                "drive.unlink",
                "Revoke an approval-gated linked folder and its project memory scope",
                "always",
                true,
            ),
            linked_folders,
        }
    }
}

#[async_trait]
impl HostFn for DriveUnlinkFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveUnlinkArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let alias = linked_alias_from_target(&args.alias)?;
        ctx.require_approval(&format!("drive.unlink {alias}"))
            .await?;
        let linked_folders = self
            .linked_folders
            .as_ref()
            .ok_or_else(|| HostError::InvalidPath("no linked folders configured".to_string()))?;
        let policy = linked_folders.remove_policy(&alias)?;
        let result = DriveUnlinkResult {
            alias: alias.clone(),
            canonical_root: policy.root.display().to_string(),
            linked_uri: format!("linked://{alias}/"),
            memory_scope: memory_scope_for_project(&alias),
            revoked_at: Utc::now(),
        };
        ctx.emit_event("drive_unlinked", drive_unlinked_payload(&result))
            .await?;
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DrivePutFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DrivePutArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let mut options = host_drive_put_options(args.options);
        if options.session_id.is_none() && !ctx.session_id.is_empty() {
            options.session_id = Some(ctx.session_id.clone());
        }
        let bytes = content_to_bytes(&self.store, &args.content)?;
        let store = self.store.as_ref().expect("drive store");
        let plan = store
            .plan_put_bytes(&bytes, &options)
            .map_err(drive_error_to_host)?;
        if drive_put_requires_approval(&options) {
            ctx.require_approval(&format!("drive.put {}", plan.path))
                .await?;
        }
        let result = self
            .store
            .as_ref()
            .expect("drive store")
            .commit_put_bytes(&bytes, options, plan)
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_put", drive_entry_event_payload("put", &result.entry))
            .await?;
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveGetFn {
    async fn call_drive(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DrivePathArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let target = args
            .uri
            .or(args.path)
            .ok_or_else(|| HostError::InvalidArgs("drive.get requires path or uri".to_string()))?;
        let uri = if target.starts_with("drive://") {
            target
        } else {
            format!(
                "drive://{}",
                normalize_canonical_path(&target).map_err(drive_error_to_host)?
            )
        };
        let content = self
            .store
            .as_ref()
            .expect("drive store")
            .resource_content(&uri, args.selector.as_deref())
            .map_err(drive_error_to_host)?;
        serde_json::to_value(content).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveLsFn {
    async fn call_drive(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let options: DriveListOptions =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let entries = self
            .store
            .as_ref()
            .expect("drive store")
            .list(options)
            .map_err(drive_error_to_host)?;
        serde_json::to_value(entries).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveMoveFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveMoveArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        ctx.require_approval(&format!("drive.move {} -> {}", args.from, args.to))
            .await?;
        let collision = if args.overwrite {
            DriveCollisionStrategy::Overwrite
        } else {
            args.collision
        };
        let entry = self
            .store
            .as_ref()
            .expect("drive store")
            .move_entry(&args.from, &args.to, collision)
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_moved", drive_moved_payload(&args.from, &entry))
            .await?;
        serde_json::to_value(entry).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveSearchFn {
    async fn call_drive(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let options: DriveSearchOptions =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let results = self
            .store
            .as_ref()
            .expect("drive store")
            .search(options)
            .map_err(drive_error_to_host)?;
        serde_json::to_value(results).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveTagFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveTagArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        ctx.require_approval(&format!("drive.tag {}", args.path))
            .await?;
        let entry = self
            .store
            .as_ref()
            .expect("drive store")
            .tag_entry(&args.path, args.tags)
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_tagged", drive_entry_event_payload("tag", &entry))
            .await?;
        serde_json::to_value(entry).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveOrganizeFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveOrganizeArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        validate_host_organizer_config(&args.config)?;
        let store = self.store.as_ref().expect("drive store");
        ctx.emit_event(
            "drive_organizer_started",
            organizer_started_payload(args.apply, &args.config),
        )
        .await?;
        if !args.apply {
            let proposals = match store.organize_with_config(args.config.clone()) {
                Ok(proposals) => proposals,
                Err(err) => {
                    ctx.emit_event(
                        "drive_organizer_failed",
                        organizer_failed_payload(args.apply, &args.config, &err.to_string()),
                    )
                    .await?;
                    return Err(drive_error_to_host(err));
                }
            };
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &args.config, &proposals),
            )
            .await?;
            return serde_json::to_value(proposals)
                .map_err(|err| HostError::HostCall(err.to_string()));
        }

        let mut ids = store.pending_proposal_ids();
        let mut generated = Vec::<OrganizerProposal>::new();
        if ids.is_empty() {
            generated = match store.organize_with_config(args.config.clone()) {
                Ok(proposals) => proposals,
                Err(err) => {
                    ctx.emit_event(
                        "drive_organizer_failed",
                        organizer_failed_payload(args.apply, &args.config, &err.to_string()),
                    )
                    .await?;
                    return Err(drive_error_to_host(err));
                }
            };
            ids = generated
                .iter()
                .filter(|proposal| {
                    matches!(
                        proposal.status,
                        ProposalStatus::Pending | ProposalStatus::Approved
                    )
                })
                .into_iter()
                .map(|proposal| proposal.id)
                .collect();
        }
        if ids.is_empty() {
            emit_drive_write_proposals(ctx, &generated).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &args.config, &generated),
            )
            .await?;
            return serde_json::to_value(generated)
                .map_err(|err| HostError::HostCall(err.to_string()));
        }
        if let Err(err) = ctx.require_approval("drive.organize apply").await {
            let status = match err {
                HostError::ApprovalDenied(_) => ProposalStatus::Denied,
                HostError::ApprovalTimeout(_) => ProposalStatus::Failed,
                _ => ProposalStatus::Failed,
            };
            store.mark_proposals_status(&ids, status);
            let proposals = store
                .proposals()
                .into_iter()
                .filter(|proposal| ids.contains(&proposal.id))
                .collect::<Vec<_>>();
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_failed",
                organizer_failed_payload_with_proposals(
                    args.apply,
                    &args.config,
                    &err.to_string(),
                    &proposals,
                ),
            )
            .await?;
            return Err(err);
        }
        let proposals = match store.apply_organizer_proposals(&ids) {
            Ok(proposals) => proposals,
            Err(err) => {
                ctx.emit_event(
                    "drive_organizer_failed",
                    organizer_failed_payload(args.apply, &args.config, &err.to_string()),
                )
                .await?;
                return Err(drive_error_to_host(err));
            }
        };
        if generated.is_empty() {
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &args.config, &proposals),
            )
            .await?;
            return serde_json::to_value(proposals)
                .map_err(|err| HostError::HostCall(err.to_string()));
        }
        for updated in proposals {
            if let Some(proposal) = generated
                .iter_mut()
                .find(|proposal| proposal.id == updated.id)
            {
                *proposal = updated;
            } else {
                generated.push(updated);
            }
        }
        emit_drive_write_proposals(ctx, &generated).await?;
        ctx.emit_event(
            "drive_organizer_completed",
            organizer_completed_payload(args.apply, &args.config, &generated),
        )
        .await?;
        serde_json::to_value(generated).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

async fn emit_drive_write_proposals(
    ctx: &InvocationCtx,
    proposals: &[OrganizerProposal],
) -> tm_host::Result<()> {
    for proposal in proposals {
        ctx.emit_event("write_proposal", drive_write_proposal_payload(proposal))
            .await?;
    }
    Ok(())
}

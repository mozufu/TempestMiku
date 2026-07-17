use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use tm_host::{HostError, HostFn, InvocationCtx, LinkedFolders, ToolDocs};

use super::authority::{DriveAuthority, cross_project_error, drive_authority, linked_scope_error};
use crate::{
    DriveUnlinkResult, SharedDriveStore, drive_link_policy, memory_scope_for_project,
    store::{
        core::{drive_error_to_host, linked_alias_from_target},
        docs::drive_docs,
        payloads::{drive_linked_payload, drive_unlinked_payload},
    },
};

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

fn default_link_mode() -> String {
    "ro".to_string()
}

pub(super) struct DriveLinkFn {
    docs: ToolDocs,
    store: SharedDriveStore,
    linked_folders: Option<LinkedFolders>,
}

impl DriveLinkFn {
    pub(super) fn new(store: SharedDriveStore, linked_folders: Option<LinkedFolders>) -> Self {
        Self {
            docs: drive_docs(
                "drive.link",
                "Register an approval-gated linked folder plus project memory scope",
                "always",
                true,
            ),
            store,
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
        let authority = drive_authority(ctx)?;
        let authorized_project = match &authority {
            DriveAuthority::Project(project) => {
                if args
                    .project
                    .as_deref()
                    .is_some_and(|requested| crate::slug(requested) != crate::slug(project))
                {
                    return Err(cross_project_error("drive.link", project));
                }
                Some(project.as_str())
            }
            DriveAuthority::Trusted => args.project.as_deref(),
            DriveAuthority::Global => return Err(linked_scope_error("drive.link")),
        };
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
        let (plan, policy) = drive_link_policy(&args.host_path, mode, authorized_project)
            .map_err(drive_error_to_host)?;
        let linked_folders = self
            .linked_folders
            .as_ref()
            .ok_or_else(|| HostError::InvalidPath("no linked folders configured".to_string()))?;
        linked_folders.insert_policy(policy)?;
        if let Err(err) = self.store.record_link(&plan).await {
            let _ = linked_folders.remove_policy(&plan.alias);
            return Err(drive_error_to_host(err));
        }
        ctx.emit_event("drive_linked", drive_linked_payload(&plan))
            .await?;
        serde_json::to_value(plan).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

pub(super) struct DriveUnlinkFn {
    docs: ToolDocs,
    store: SharedDriveStore,
    linked_folders: Option<LinkedFolders>,
}

impl DriveUnlinkFn {
    pub(super) fn new(store: SharedDriveStore, linked_folders: Option<LinkedFolders>) -> Self {
        Self {
            docs: drive_docs(
                "drive.unlink",
                "Revoke an approval-gated linked folder and its project memory scope",
                "always",
                true,
            ),
            store,
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
        match drive_authority(ctx)? {
            DriveAuthority::Project(project) if crate::slug(&project) == alias => {}
            DriveAuthority::Project(project) => {
                return Err(cross_project_error("drive.unlink", &project));
            }
            DriveAuthority::Trusted => {}
            DriveAuthority::Global => return Err(linked_scope_error("drive.unlink")),
        }
        ctx.require_approval(&format!("drive.unlink {alias}"))
            .await?;
        self.store
            .revoke_link(&alias)
            .await
            .map_err(drive_error_to_host)?;
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

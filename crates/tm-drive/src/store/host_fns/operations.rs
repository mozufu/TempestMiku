use serde::Deserialize;
use serde_json::Value;
use tm_host::{HostError, InvocationCtx};

use super::{
    DriveGetFn, DriveLsFn, DriveMoveFn, DrivePutFn, DriveSearchFn, DriveTagFn,
    authority::{
        DriveAuthority, authorize_drive_entry, cross_project_error, drive_authority,
        global_project_error,
    },
};
use crate::{
    DriveCollisionStrategy, DriveListOptions, DrivePutOptions, DriveSearchOptions,
    store::{
        core::{
            drive_error_to_host, drive_put_requires_approval, host_drive_put_options,
            normalize_canonical_path,
        },
        docs::content_to_bytes,
        payloads::{drive_entry_event_payload, drive_moved_payload},
    },
};

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

impl DrivePutFn {
    pub(super) async fn call_drive(
        &self,
        args: Value,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Value> {
        let args: DrivePutArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let mut options = host_drive_put_options(args.options);
        match drive_authority(ctx)? {
            DriveAuthority::Project(project) => {
                if options
                    .project
                    .as_deref()
                    .is_some_and(|requested| crate::slug(requested) != crate::slug(&project))
                {
                    return Err(cross_project_error("drive.put", &project));
                }
                options.project = Some(project);
            }
            DriveAuthority::Global if options.project.is_some() => {
                return Err(global_project_error("drive.put"));
            }
            DriveAuthority::Global | DriveAuthority::Trusted => {}
        }
        if options.session_id.is_none() && !ctx.session_id.is_empty() {
            options.session_id = Some(ctx.session_id.clone());
        }
        let bytes = content_to_bytes(&self.store, &args.content).await?;
        let path = self
            .store
            .plan_put_path(&bytes, &options)
            .await
            .map_err(drive_error_to_host)?;
        if drive_put_requires_approval(&options) {
            ctx.require_approval(&format!("drive.put {path}")).await?;
        }
        let result = self
            .store
            .put_bytes(&bytes, options)
            .await
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_put", drive_entry_event_payload("put", &result.entry))
            .await?;
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveGetFn {
    pub(super) async fn call_drive(
        &self,
        args: Value,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Value> {
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
        authorize_drive_entry(&self.store, &uri, ctx).await?;
        let content = self
            .store
            .resource_content(&uri, args.selector.as_deref())
            .await
            .map_err(drive_error_to_host)?;
        serde_json::to_value(content).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveLsFn {
    pub(super) async fn call_drive(
        &self,
        args: Value,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Value> {
        let options: DriveListOptions =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let authority = drive_authority(ctx)?;
        let mut entries = self
            .store
            .list(options)
            .await
            .map_err(drive_error_to_host)?;
        entries.retain(|entry| authority.permits_entry(entry));
        serde_json::to_value(entries).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveMoveFn {
    pub(super) async fn call_drive(
        &self,
        args: Value,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Value> {
        let args: DriveMoveArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        authorize_drive_entry(&self.store, &args.from, ctx).await?;
        ctx.require_approval(&format!("drive.move {} -> {}", args.from, args.to))
            .await?;
        let collision = if args.overwrite {
            DriveCollisionStrategy::Overwrite
        } else {
            args.collision
        };
        let entry = self
            .store
            .move_entry(&args.from, &args.to, collision)
            .await
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_moved", drive_moved_payload(&args.from, &entry))
            .await?;
        serde_json::to_value(entry).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveSearchFn {
    pub(super) async fn call_drive(
        &self,
        args: Value,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Value> {
        let mut options: DriveSearchOptions =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let authority = drive_authority(ctx)?;
        match &authority {
            DriveAuthority::Project(project) => {
                if options
                    .project
                    .as_deref()
                    .is_some_and(|requested| crate::slug(requested) != crate::slug(project))
                {
                    return Err(cross_project_error("drive.search", project));
                }
                options.project = Some(project.clone());
            }
            DriveAuthority::Global if options.project.is_some() => {
                return Err(global_project_error("drive.search"));
            }
            DriveAuthority::Global | DriveAuthority::Trusted => {}
        }
        let mut results = self
            .store
            .search(options)
            .await
            .map_err(drive_error_to_host)?;
        results.retain(|result| authority.permits_project(result.project.as_deref()));
        serde_json::to_value(results).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveTagFn {
    pub(super) async fn call_drive(
        &self,
        args: Value,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Value> {
        let args: DriveTagArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        authorize_drive_entry(&self.store, &args.path, ctx).await?;
        ctx.require_approval(&format!("drive.tag {}", args.path))
            .await?;
        let entry = self
            .store
            .tag_entry(&args.path, args.tags)
            .await
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_tagged", drive_entry_event_payload("tag", &entry))
            .await?;
        serde_json::to_value(entry).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

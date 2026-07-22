use std::collections::BTreeSet;

use tm_host::{HostError, InvocationCtx};
use uuid::Uuid;

use super::super::core::{drive_error_to_host, normalize_canonical_path};
use crate::{DriveListOptions, SharedDriveStore};

#[derive(Debug, Clone)]
pub(crate) enum DriveAuthority {
    Trusted,
    Global,
    Project(String),
}

impl DriveAuthority {
    pub(crate) fn is_trusted(&self) -> bool {
        matches!(self, Self::Trusted)
    }

    pub(super) fn permits_project(&self, project: Option<&str>) -> bool {
        match self {
            Self::Trusted => true,
            Self::Global => project.is_none(),
            Self::Project(authorized) => {
                project.is_some_and(|project| crate::slug(project) == crate::slug(authorized))
            }
        }
    }

    pub(crate) fn permits_entry(&self, entry: &crate::DriveEntry) -> bool {
        self.permits_project(entry.project.as_deref())
    }
}

pub(crate) fn drive_authority(ctx: &InvocationCtx) -> tm_host::Result<DriveAuthority> {
    let real_session = !ctx.session_id.is_empty() && ctx.session_id != "default";
    match ctx.project_id.as_deref() {
        Some(project) => {
            let project = project.trim();
            if project.is_empty() {
                return Err(HostError::CapabilityDenied(
                    "drive requires a non-empty project scope".to_string(),
                ));
            }
            Ok(DriveAuthority::Project(project.to_string()))
        }
        None if real_session => Ok(DriveAuthority::Global),
        None => Ok(DriveAuthority::Trusted),
    }
}

pub(super) fn cross_project_error(operation: &str, project: &str) -> HostError {
    HostError::CapabilityDenied(format!(
        "{operation} is restricted to project:{}",
        crate::slug(project)
    ))
}

pub(super) fn global_project_error(operation: &str) -> HostError {
    HostError::CapabilityDenied(format!(
        "{operation} from global scope is restricted to unprojected drive entries"
    ))
}

pub(super) fn linked_scope_error(operation: &str) -> HostError {
    HostError::CapabilityDenied(format!("{operation} requires an authorized project scope"))
}

pub(super) async fn authorize_drive_entry(
    store: &SharedDriveStore,
    path_or_uri: &str,
    ctx: &InvocationCtx,
) -> tm_host::Result<()> {
    let authority = drive_authority(ctx)?;
    let path = normalize_canonical_path(path_or_uri).map_err(drive_error_to_host)?;
    let entry = store
        .list(DriveListOptions {
            path: Some(path.clone()),
            recursive: true,
            limit: 1,
            include_archived: true,
        })
        .await
        .map_err(drive_error_to_host)?
        .into_iter()
        .find(|entry| entry.path == path)
        .ok_or_else(|| HostError::NotFound(path_or_uri.to_string()))?;
    if authority.permits_entry(&entry) {
        Ok(())
    } else {
        match authority {
            DriveAuthority::Project(project) => {
                Err(cross_project_error("drive entry access", &project))
            }
            DriveAuthority::Global => Err(global_project_error("drive entry access")),
            DriveAuthority::Trusted => Ok(()),
        }
    }
}

pub(super) async fn authorized_proposal_ids(
    store: &SharedDriveStore,
    ids: Vec<Uuid>,
    authority: &DriveAuthority,
) -> tm_host::Result<Vec<Uuid>> {
    let entry_ids = store
        .list(DriveListOptions {
            path: None,
            recursive: true,
            limit: usize::MAX,
            include_archived: true,
        })
        .await
        .map_err(drive_error_to_host)?
        .into_iter()
        .filter(|entry| authority.permits_entry(entry))
        .map(|entry| entry.id)
        .collect::<BTreeSet<_>>();
    let allowed = store
        .proposals()
        .await
        .map_err(drive_error_to_host)?
        .into_iter()
        .filter(|proposal| entry_ids.contains(&proposal.entry_id))
        .map(|proposal| proposal.id)
        .collect::<BTreeSet<_>>();
    Ok(ids.into_iter().filter(|id| allowed.contains(id)).collect())
}

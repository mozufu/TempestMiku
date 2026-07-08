use std::{collections::BTreeSet, path::Path};

use tm_host::{FsMode, FsPolicy};

use crate::{DriveLinkPlan, organize::slug, types::DriveError};

pub fn drive_link_plan(
    host_path: impl AsRef<Path>,
    mode: FsMode,
    project: Option<&str>,
) -> Result<DriveLinkPlan, DriveError> {
    drive_link_policy(host_path, mode, project).map(|(plan, _)| plan)
}

pub fn drive_link_policy(
    host_path: impl AsRef<Path>,
    mode: FsMode,
    project: Option<&str>,
) -> Result<(DriveLinkPlan, FsPolicy), DriveError> {
    let host_path = host_path.as_ref();
    let canonical = host_path
        .canonicalize()
        .map_err(|err| DriveError::InvalidPath(format!("{}: {err}", host_path.display())))?;
    if !canonical.is_dir() {
        return Err(DriveError::InvalidPath(format!(
            "linked drive path is not a directory: {}",
            canonical.display()
        )));
    }
    let project = project
        .filter(|project| !project.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            canonical
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "project".to_string());
    let alias = slug(&project);
    let mode_label = match mode {
        FsMode::Ro => "ro",
        FsMode::Rw => "rw",
    }
    .to_string();
    let plan = DriveLinkPlan {
        alias: alias.clone(),
        canonical_root: canonical.display().to_string(),
        mode: mode_label,
        linked_uri: format!("linked://{alias}/"),
        memory_scope: memory_scope_for_project(&project),
        project,
        requires_approval: true,
    };
    let policy = FsPolicy {
        alias,
        root: canonical,
        mode,
        commands: BTreeSet::new(),
        safe_args: Vec::new(),
    };
    Ok((plan, policy))
}

pub fn memory_scope_for_project(project: &str) -> String {
    format!("project:{}", slug(project))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_plan_couples_folder_and_memory_scope() {
        let dir = tempfile::tempdir().unwrap();
        let plan = drive_link_plan(dir.path(), FsMode::Ro, Some("Tempest Miku")).unwrap();

        assert_eq!(plan.alias, "tempest-miku");
        assert_eq!(plan.mode, "ro");
        assert_eq!(plan.linked_uri, "linked://tempest-miku/");
        assert_eq!(plan.memory_scope, "project:tempest-miku");
        assert!(plan.requires_approval);
    }
}

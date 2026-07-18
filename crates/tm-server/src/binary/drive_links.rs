use std::{collections::BTreeSet, path::PathBuf};

use tm_host::{FsMode, FsPolicy, LinkedFolders};

use super::BoxError;

pub(super) async fn hydrate_drive_links(
    drive_store: &tm_drive::SharedDriveStore,
    linked_folders: &LinkedFolders,
) -> Result<usize, BoxError> {
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tm_artifacts::ArtifactStore;
    use tm_host::LinkedFolderConfig;

    use super::*;

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

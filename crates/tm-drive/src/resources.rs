use async_trait::async_trait;
use tm_artifacts::ResourceContent;
use tm_host::{HostError, InvocationCtx, ResourceEntry, ResourceHandler};

use crate::{InMemoryDriveStore, types::DriveError};

#[derive(Debug, Clone)]
pub struct DriveResourceHandler {
    store: InMemoryDriveStore,
}

impl DriveResourceHandler {
    pub fn new(store: InMemoryDriveStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ResourceHandler for DriveResourceHandler {
    fn scheme(&self) -> &str {
        "drive"
    }

    fn capability(&self) -> &str {
        "resources.read:drive"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> tm_host::Result<ResourceContent> {
        self.store
            .resource_content(uri, selector)
            .map_err(drive_error_to_host)
    }

    async fn preview(&self, uri: &str, _ctx: &InvocationCtx) -> tm_host::Result<ResourceContent> {
        let mut content = self
            .store
            .resource_content(uri, None)
            .map_err(drive_error_to_host)?;
        content.content.clear();
        Ok(content)
    }

    async fn list(
        &self,
        uri: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> tm_host::Result<Vec<ResourceEntry>> {
        let uri = uri.unwrap_or("drive://");
        if uri == "drive://" || uri == "drive:///" {
            let mut entries = virtual_roots();
            entries.extend(
                self.store
                    .resource_entries(None)
                    .map_err(drive_error_to_host)?,
            );
            return Ok(entries);
        }
        self.store
            .resource_entries(Some(uri))
            .map_err(drive_error_to_host)
    }
}

fn virtual_roots() -> Vec<ResourceEntry> {
    [
        (
            "drive://recent",
            "recent",
            "virtual_dir",
            "Recent documents",
        ),
        (
            "drive://by-project",
            "by-project",
            "virtual_dir",
            "Documents by project",
        ),
        (
            "drive://by-type",
            "by-type",
            "virtual_dir",
            "Documents by type",
        ),
        (
            "drive://by-tag",
            "by-tag",
            "virtual_dir",
            "Documents by tag",
        ),
        (
            "drive://by-date",
            "by-date",
            "virtual_dir",
            "Documents by date",
        ),
    ]
    .into_iter()
    .map(|(uri, name, kind, title)| ResourceEntry {
        uri: uri.to_string(),
        name: name.to_string(),
        kind: kind.to_string(),
        title: Some(title.to_string()),
        size_bytes: None,
        modified_at: None,
    })
    .collect()
}

fn drive_error_to_host(err: DriveError) -> HostError {
    match err {
        DriveError::NotFound(target) => HostError::NotFound(target),
        DriveError::InvalidArgs(message) => HostError::InvalidArgs(message),
        DriveError::InvalidPath(path) => HostError::InvalidPath(path),
        DriveError::Collision(path) => HostError::InvalidArgs(format!("drive path exists: {path}")),
        DriveError::Integrity { .. } => HostError::HostCall(err.to_string()),
        DriveError::Store(message) => HostError::HostCall(message),
    }
}

#[cfg(test)]
mod tests {
    use tm_artifacts::ArtifactStore;
    use tm_host::{CapabilityGrants, InvocationCtx, ResourceRegistry};

    use crate::{DrivePutOptions, InMemoryDriveStore};

    use super::*;

    #[tokio::test]
    async fn resource_handler_reads_lists_and_denies_without_grant() {
        let dir = tempfile::tempdir().unwrap();
        let store = InMemoryDriveStore::new(ArtifactStore::open(dir.path(), "drive").unwrap());
        let put = store
            .put_bytes(
                b"# Note\nhello",
                DrivePutOptions {
                    suggested_path: Some("notes/a.md".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let mut registry = ResourceRegistry::new();
        registry.register(std::sync::Arc::new(DriveResourceHandler::new(store)));

        let denied = registry
            .read(
                &put.uri,
                None,
                &InvocationCtx::new(CapabilityGrants::default()),
            )
            .await
            .unwrap_err();
        assert!(matches!(denied, HostError::CapabilityDenied(_)));

        let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:drive"));
        let content = registry.read(&put.uri, Some("2-2"), &ctx).await.unwrap();
        assert_eq!(content.content, "hello");
        let missing = registry
            .read("drive://notes/missing.md", None, &ctx)
            .await
            .unwrap_err();
        let HostError::NotFound(target) = missing else {
            panic!("expected drive not found, got {missing:?}");
        };
        assert!(target.contains("notes/missing.md"));
        assert!(target.contains("nearby paths: notes/a.md"));
        let listed = registry.list(Some("drive://"), &ctx).await.unwrap();
        assert!(listed.iter().any(|entry| entry.uri == put.uri));
    }
}

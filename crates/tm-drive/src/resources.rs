use async_trait::async_trait;
use tm_artifacts::ResourceContent;
use tm_host::{HostError, InvocationCtx, ResourceEntry, ResourceHandler};

use crate::{
    DriveEntry, DriveListOptions, IntoSharedDriveStore, SharedDriveStore, store::drive_authority,
    types::DriveError,
};

#[derive(Debug, Clone)]
pub struct DriveResourceHandler {
    store: SharedDriveStore,
}

impl DriveResourceHandler {
    pub fn new(store: impl IntoSharedDriveStore) -> Self {
        Self {
            store: store.into_shared_drive_store(),
        }
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
        ctx: &InvocationCtx,
    ) -> tm_host::Result<ResourceContent> {
        authorize_resource_entry(&self.store, uri, ctx).await?;
        self.store
            .resource_content(uri, selector)
            .await
            .map_err(drive_error_to_host)
    }

    async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> tm_host::Result<ResourceContent> {
        authorize_resource_entry(&self.store, uri, ctx).await?;
        let mut content = self
            .store
            .resource_content(uri, None)
            .await
            .map_err(drive_error_to_host)?;
        content.content.clear();
        Ok(content)
    }

    async fn list(
        &self,
        uri: Option<&str>,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Vec<ResourceEntry>> {
        let authority = drive_authority(ctx)?;
        let uri = uri.unwrap_or("drive://");
        let path = if uri == "drive://" || uri == "drive:///" {
            None
        } else {
            Some(uri.trim_start_matches("drive://").to_string())
        };
        let mut drive_entries = self
            .store
            .list(DriveListOptions {
                path,
                recursive: true,
                limit: 1000,
                include_archived: false,
            })
            .await
            .map_err(drive_error_to_host)?;
        drive_entries.retain(|entry| authority.permits_entry(entry));
        let document_entries = drive_entries
            .into_iter()
            .map(resource_entry)
            .collect::<Vec<_>>();
        if uri == "drive://" || uri == "drive:///" {
            let mut entries = virtual_roots();
            entries.extend(document_entries);
            return Ok(entries);
        }
        Ok(document_entries)
    }
}

fn resource_entry(entry: DriveEntry) -> ResourceEntry {
    let name = entry
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&entry.path)
        .to_string();
    let kind = entry
        .doc_kind
        .clone()
        .unwrap_or_else(|| "drive_document".to_string());
    ResourceEntry {
        uri: entry.uri,
        name,
        kind,
        title: entry.title,
        size_bytes: Some(entry.size_bytes),
        modified_at: Some(entry.updated_at.to_rfc3339()),
    }
}

async fn authorize_resource_entry(
    store: &SharedDriveStore,
    uri: &str,
    ctx: &InvocationCtx,
) -> tm_host::Result<()> {
    let authority = drive_authority(ctx)?;
    let path = crate::drive_uri_path(uri).map_err(drive_error_to_host)?;
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
        .find(|entry| entry.path == path);
    let Some(entry) = entry else {
        return if authority.is_trusted() {
            Ok(())
        } else {
            Err(HostError::NotFound(uri.to_string()))
        };
    };
    if authority.permits_entry(&entry) {
        Ok(())
    } else {
        Err(HostError::CapabilityDenied(format!(
            "drive resource {uri} is outside the authorized session scope"
        )))
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
        DriveError::Conflict { .. } => HostError::HostCall(err.to_string()),
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

    #[tokio::test]
    async fn resource_handler_filters_global_and_project_authority() {
        let dir = tempfile::tempdir().unwrap();
        let store = InMemoryDriveStore::new(ArtifactStore::open(dir.path(), "drive").unwrap());
        let global = store
            .put_bytes(
                b"global",
                DrivePutOptions {
                    suggested_path: Some("notes/global.txt".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let alpha = store
            .put_bytes(
                b"alpha",
                DrivePutOptions {
                    suggested_path: Some("projects/alpha/note.txt".to_string()),
                    project: Some("alpha".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let beta = store
            .put_bytes(
                b"beta",
                DrivePutOptions {
                    suggested_path: Some("projects/beta/note.txt".to_string()),
                    project: Some("beta".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let mut registry = ResourceRegistry::new();
        registry.register(std::sync::Arc::new(DriveResourceHandler::new(store)));
        let grants = CapabilityGrants::default().allow("resources.read:drive");
        let global_ctx = InvocationCtx::new(grants.clone())
            .with_session_id("global-session")
            .with_session_scope("global");
        let global_list = registry.list(Some("drive://"), &global_ctx).await.unwrap();
        assert!(global_list.iter().any(|entry| entry.uri == global.uri));
        assert!(!global_list.iter().any(|entry| entry.uri == alpha.uri));
        assert!(matches!(
            registry.read(&alpha.uri, None, &global_ctx).await,
            Err(HostError::CapabilityDenied(_))
        ));

        let alpha_ctx = InvocationCtx::new(grants)
            .with_session_id("alpha-session")
            .with_session_scope("project:alpha");
        assert_eq!(
            registry
                .read(&alpha.uri, None, &alpha_ctx)
                .await
                .unwrap()
                .content,
            "alpha"
        );
        assert!(matches!(
            registry.read(&beta.uri, None, &alpha_ctx).await,
            Err(HostError::CapabilityDenied(_))
        ));
        let alpha_list = registry.list(Some("drive://"), &alpha_ctx).await.unwrap();
        assert!(alpha_list.iter().any(|entry| entry.uri == alpha.uri));
        assert!(!alpha_list.iter().any(|entry| entry.uri == global.uri));
        assert!(!alpha_list.iter().any(|entry| entry.uri == beta.uri));
        let alpha_virtual = registry
            .list(Some("drive://by-project/alpha"), &alpha_ctx)
            .await
            .unwrap();
        assert_eq!(alpha_virtual.len(), 1);
        assert_eq!(alpha_virtual[0].uri, alpha.uri);
        let beta_virtual = registry
            .list(Some("drive://by-project/beta"), &alpha_ctx)
            .await
            .unwrap();
        assert!(beta_virtual.is_empty());
        let recent = registry
            .list(Some("drive://recent"), &alpha_ctx)
            .await
            .unwrap();
        assert!(recent.iter().any(|entry| entry.uri == alpha.uri));
        assert!(!recent.iter().any(|entry| entry.uri == beta.uri));
    }

    #[tokio::test]
    async fn resource_handler_applies_bounded_default_and_hard_paging_limits() {
        let dir = tempfile::tempdir().unwrap();
        let store = InMemoryDriveStore::new(ArtifactStore::open(dir.path(), "drive").unwrap());
        let text = (1..=400)
            .map(|line| format!("line {line}: {}", "x".repeat(400)))
            .collect::<Vec<_>>()
            .join("\n");
        let filed = store
            .put_bytes(
                text.as_bytes(),
                DrivePutOptions {
                    suggested_path: Some("notes/paged.txt".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let handler = DriveResourceHandler::new(store);
        let ctx = InvocationCtx::new(CapabilityGrants::default());
        let default_page = handler.read(&filed.uri, None, &ctx).await.unwrap();
        assert!(default_page.has_more);
        assert!(default_page.content.len() <= 64 * 1024);
        assert!(default_page.content.lines().count() <= 200);

        let hard_page = handler
            .read(&filed.uri, Some("1-1000"), &ctx)
            .await
            .unwrap();
        assert!(hard_page.content.len() <= 256 * 1024);
        let error = handler
            .read(&filed.uri, Some("1-1001"), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(error, HostError::InvalidArgs(_)));
    }
}

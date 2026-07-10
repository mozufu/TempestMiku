use std::sync::Arc;

use sha2::{Digest, Sha256};
use tm_artifacts::ArtifactStore;

use super::{
    core::normalize_canonical_path_path_or_uri,
    metadata::DriveMetadataStore,
    types::{DriveRead, DriveService, InMemoryDriveMetadataStore},
};
use crate::{DriveError, DriveMetadataSnapshot};

impl<M> DriveService<M> {
    pub fn with_metadata(artifacts: ArtifactStore, metadata: M) -> Self {
        Self::with_shared_metadata(artifacts, Arc::new(metadata))
    }

    pub fn with_shared_metadata(artifacts: ArtifactStore, metadata: Arc<M>) -> Self {
        Self {
            artifacts,
            metadata,
        }
    }

    pub fn metadata_store(&self) -> Arc<M> {
        Arc::clone(&self.metadata)
    }

    pub fn artifact_store(&self) -> &ArtifactStore {
        &self.artifacts
    }
}

impl<M> DriveService<M>
where
    M: DriveMetadataStore,
{
    pub async fn metadata_snapshot(&self) -> crate::Result<DriveMetadataSnapshot> {
        self.metadata.snapshot().await
    }

    pub async fn read_metadata_entry(&self, path_or_uri: &str) -> crate::Result<DriveRead> {
        let path = normalize_canonical_path_path_or_uri(path_or_uri)?;
        let entry = self
            .metadata
            .entry_by_path(&path)
            .await?
            .ok_or_else(|| DriveError::NotFound(path_or_uri.to_string()))?;
        let bytes = self
            .artifacts
            .read_blob(&entry.blob_uri)
            .map_err(|err| DriveError::NotFound(err.to_string()))?;
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != entry.content_hash {
            return Err(DriveError::Integrity {
                path: entry.path,
                expected: entry.content_hash,
                actual,
            });
        }
        Ok(DriveRead { entry, bytes })
    }
}

impl DriveService<InMemoryDriveMetadataStore> {
    pub fn new(artifacts: ArtifactStore) -> Self {
        Self::with_metadata(artifacts, InMemoryDriveMetadataStore::default())
    }

    pub fn from_snapshot(
        artifacts: ArtifactStore,
        snapshot: DriveMetadataSnapshot,
    ) -> crate::Result<Self> {
        Ok(Self::with_metadata(
            artifacts,
            InMemoryDriveMetadataStore::from_snapshot(snapshot)?,
        ))
    }
}

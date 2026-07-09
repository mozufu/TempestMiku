use async_trait::async_trait;
use tm_artifacts::{ArtifactStore, ResourceContent};
use url::Url;

use super::{
    capability::InvocationCtx,
    error::{HostError, Result},
    registry::{ResourceEntry, ResourceHandler},
};

pub struct ArtifactResourceHandler {
    store: ArtifactStore,
}

impl ArtifactResourceHandler {
    pub fn new(store: ArtifactStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ResourceHandler for ArtifactResourceHandler {
    fn scheme(&self) -> &str {
        "artifact"
    }

    fn capability(&self) -> &str {
        "resources.read:artifact"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> Result<ResourceContent> {
        self.store
            .read(uri, selector)
            .map_err(|err| HostError::NotFound(err.to_string()))
    }

    async fn list(&self, _uri: Option<&str>, _ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        Ok(self
            .store
            .list()
            .into_iter()
            .map(|artifact| ResourceEntry {
                uri: artifact.uri,
                name: artifact.id,
                kind: artifact.kind,
                title: artifact.title,
                size_bytes: Some(artifact.size_bytes),
                modified_at: None,
            })
            .collect())
    }
}

pub(crate) fn parse_scheme(uri: &str) -> Result<String> {
    if let Ok(url) = Url::parse(uri) {
        return Ok(url.scheme().to_string());
    }
    uri.split_once("://")
        .map(|(scheme, _)| scheme.to_string())
        .ok_or_else(|| HostError::InvalidArgs(format!("missing URI scheme in {uri}")))
}

#[cfg(test)]
mod tests;

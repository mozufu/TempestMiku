use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tm_artifacts::ResourceContent;

use super::resource::parse_scheme;
use super::{
    capability::InvocationCtx,
    error::{HostError, Result},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceEntry {
    pub uri: String,
    pub name: String,
    pub kind: String,
    pub title: Option<String>,
    pub size_bytes: Option<usize>,
    pub modified_at: Option<String>,
}

#[async_trait]
pub trait ResourceHandler: Send + Sync {
    fn scheme(&self) -> &str;
    fn capability(&self) -> &str;
    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> Result<ResourceContent>;

    async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> Result<ResourceContent> {
        let mut content = self.read(uri, None, ctx).await?;
        content.content.clear();
        Ok(content)
    }

    async fn list(&self, uri: Option<&str>, _ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        Err(HostError::NotFound(format!(
            "resource list unsupported for {} {}",
            self.scheme(),
            uri.unwrap_or("")
        )))
    }
}

#[derive(Default, Clone)]
pub struct ResourceRegistry {
    handlers: BTreeMap<String, Arc<dyn ResourceHandler>>,
}

impl ResourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, handler: Arc<dyn ResourceHandler>) {
        self.handlers.insert(handler.scheme().to_string(), handler);
    }

    pub fn schemes(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    pub fn capabilities(&self) -> Vec<(String, String)> {
        self.handlers
            .iter()
            .map(|(scheme, handler)| (scheme.clone(), handler.capability().to_string()))
            .collect()
    }

    pub async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> Result<ResourceContent> {
        let handler = self.handler_for(uri, ctx)?;
        handler.read(uri, selector, ctx).await
    }

    pub async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> Result<ResourceContent> {
        let handler = self.handler_for(uri, ctx)?;
        handler.preview(uri, ctx).await
    }

    pub async fn list(&self, uri: Option<&str>, ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        let Some(uri) = uri.filter(|uri| !uri.is_empty()) else {
            return Ok(self
                .handlers
                .keys()
                .map(|scheme| ResourceEntry {
                    uri: format!("{scheme}://"),
                    name: scheme.clone(),
                    kind: "scheme".to_string(),
                    title: None,
                    size_bytes: None,
                    modified_at: None,
                })
                .collect());
        };
        let handler = self.handler_for(uri, ctx)?;
        handler.list(Some(uri), ctx).await
    }

    fn handler_for(&self, uri: &str, ctx: &InvocationCtx) -> Result<Arc<dyn ResourceHandler>> {
        let scheme = parse_scheme(uri)?;
        let handler = self
            .handlers
            .get(&scheme)
            .ok_or_else(|| HostError::UnknownScheme {
                scheme: scheme.clone(),
                registered: self.schemes(),
            })?;
        if !ctx.grants.permits(handler.capability()) {
            return Err(HostError::CapabilityDenied(
                handler.capability().to_string(),
            ));
        }
        Ok(Arc::clone(handler))
    }
}

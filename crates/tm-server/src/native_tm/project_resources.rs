use std::sync::Arc;

use async_trait::async_trait;
use tm_artifacts::{ResourceContent, preview};
use tm_host::{HostError, InvocationCtx, ResourceEntry, ResourceHandler};

use crate::{ProjectStatus, Store};

pub struct ProjectEnvironmentResourceHandler<S> {
    store: Arc<S>,
    owner_subject: String,
    project_id: String,
}

impl<S> ProjectEnvironmentResourceHandler<S> {
    pub fn new(store: Arc<S>, owner_subject: String, project_id: String) -> Self {
        Self {
            store,
            owner_subject,
            project_id,
        }
    }

    fn uri(&self) -> String {
        format!("project://{}/environment", self.project_id)
    }
}

#[async_trait]
impl<S: Store> ResourceHandler for ProjectEnvironmentResourceHandler<S> {
    fn scheme(&self) -> &str {
        "project"
    }

    fn capability(&self) -> &str {
        "resources.read:project"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<ResourceContent> {
        if selector.is_some() {
            return Err(HostError::InvalidArgs(
                "project environment resources do not support selectors".to_string(),
            ));
        }
        let expected = self.uri();
        if uri != expected {
            return Err(HostError::NotFound(format!("project resource {uri}")));
        }
        if ctx.project_id.as_deref() != Some(self.project_id.as_str()) {
            return Err(HostError::CapabilityDenied(format!(
                "project resource {uri} is outside authorized project"
            )));
        }
        match self
            .store
            .project(&self.project_id)
            .await
            .map_err(map_store_error)?
        {
            Some(project) if project.status == ProjectStatus::Active => {}
            _ => {
                return Err(HostError::NotFound(format!(
                    "active project {}",
                    self.project_id
                )));
            }
        }
        let memory_scope = format!("project:{}", self.project_id);
        let payload = match self
            .store
            .environment_cognition(&self.owner_subject, &memory_scope)
            .await
            .map_err(map_store_error)?
        {
            Some(cognition) => serde_json::to_value(cognition)
                .map_err(|error| HostError::HostCall(error.to_string()))?,
            None => serde_json::json!({ "status": "empty" }),
        };
        let content = serde_json::to_string_pretty(&payload)
            .map_err(|error| HostError::HostCall(error.to_string()))?;
        Ok(ResourceContent {
            uri: expected,
            kind: "project_environment".to_string(),
            mime: "application/json".to_string(),
            title: Some(format!("Environment cognition for {}", self.project_id)),
            size_bytes: content.len(),
            selector: None,
            has_more: false,
            preview: preview(&content, 1024),
            content,
        })
    }

    async fn list(
        &self,
        uri: Option<&str>,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Vec<ResourceEntry>> {
        if ctx.project_id.as_deref() != Some(self.project_id.as_str()) {
            return Err(HostError::CapabilityDenied(
                "project resources require the authorized project".to_string(),
            ));
        }
        let expected = self.uri();
        if uri.is_some_and(|uri| uri != "project://" && uri != expected) {
            return Err(HostError::NotFound(format!(
                "project resource {}",
                uri.unwrap_or_default()
            )));
        }
        match self
            .store
            .project(&self.project_id)
            .await
            .map_err(map_store_error)?
        {
            Some(project) if project.status == ProjectStatus::Active => Ok(vec![ResourceEntry {
                uri: expected,
                name: "environment".to_string(),
                kind: "project_environment".to_string(),
                title: Some(format!("Environment cognition for {}", self.project_id)),
                size_bytes: None,
                modified_at: None,
            }]),
            _ => Err(HostError::NotFound(format!(
                "active project {}",
                self.project_id
            ))),
        }
    }
}

fn map_store_error(error: crate::ServerError) -> HostError {
    match error {
        crate::ServerError::NotFound(target) => HostError::NotFound(target),
        other => HostError::HostCall(other.to_string()),
    }
}

use async_trait::async_trait;
use tm_artifacts::{ResourceContent, preview};
use tm_host::{HostError, InvocationCtx, ResourceEntry, ResourceHandler};

use crate::{ManagedSkillError, ModesConfig};

#[derive(Debug, Clone)]
pub struct SkillResourceHandler {
    config: ModesConfig,
}

impl SkillResourceHandler {
    pub fn new(config: ModesConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ResourceHandler for SkillResourceHandler {
    fn scheme(&self) -> &str {
        "skill"
    }

    fn capability(&self) -> &str {
        "resources.read:skill"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> tm_host::Result<ResourceContent> {
        if selector.is_some() {
            return Err(HostError::InvalidArgs(
                "skill resources do not support selectors".to_string(),
            ));
        }
        let (kind, title, content) = match parse_skill_uri(uri)? {
            SkillUri::Root => (
                "skill_catalog",
                Some("Managed skills".to_string()),
                serde_json::to_string_pretty(&self.config.managed_skills().map_err(map_error)?)
                    .map_err(|error| HostError::HostCall(error.to_string()))?,
            ),
            SkillUri::Active { name } => {
                let (version, body) = self.config.managed_skill_body(&name).map_err(map_error)?;
                (
                    "managed_skill",
                    Some(format!("{} ({})", version.name, version.content_digest)),
                    body,
                )
            }
            SkillUri::Versions { name } => (
                "skill_versions",
                Some(format!("{name} versions")),
                serde_json::to_string_pretty(&self.config.managed_skill(&name).map_err(map_error)?)
                    .map_err(|error| HostError::HostCall(error.to_string()))?,
            ),
            SkillUri::Version { name, digest } => {
                let (version, body) = self
                    .config
                    .managed_skill_version_body(&name, &digest)
                    .map_err(map_error)?;
                (
                    "managed_skill_version",
                    Some(format!("{} ({})", version.name, version.content_digest)),
                    body,
                )
            }
        };
        let mime = if kind == "skill_catalog" || kind == "skill_versions" {
            "application/json"
        } else {
            "text/markdown; charset=utf-8"
        };
        Ok(ResourceContent {
            uri: uri.to_string(),
            kind: kind.to_string(),
            mime: mime.to_string(),
            title,
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
        _ctx: &InvocationCtx,
    ) -> tm_host::Result<Vec<ResourceEntry>> {
        let uri = uri.unwrap_or("skill://");
        match parse_skill_uri(uri)? {
            SkillUri::Root => Ok(self
                .config
                .managed_skills()
                .map_err(map_error)?
                .into_iter()
                .map(|skill| ResourceEntry {
                    uri: format!("skill://{}", skill.active.name),
                    name: skill.active.name,
                    kind: "managed_skill".to_string(),
                    title: Some(skill.active.description),
                    size_bytes: None,
                    modified_at: None,
                })
                .collect()),
            SkillUri::Active { name } | SkillUri::Versions { name } => Ok(self
                .config
                .managed_skill(&name)
                .map_err(map_error)?
                .versions
                .into_iter()
                .map(|version| ResourceEntry {
                    uri: format!(
                        "skill://{}/versions/{}",
                        version.name,
                        version.content_digest.trim_start_matches("sha256:")
                    ),
                    name: version.content_digest,
                    kind: "managed_skill_version".to_string(),
                    title: Some(version.description),
                    size_bytes: None,
                    modified_at: None,
                })
                .collect()),
            SkillUri::Version { .. } => Err(HostError::InvalidArgs(
                "skill version listing requires skill://<name>/versions".to_string(),
            )),
        }
    }
}

enum SkillUri {
    Root,
    Active { name: String },
    Versions { name: String },
    Version { name: String, digest: String },
}

fn parse_skill_uri(uri: &str) -> tm_host::Result<SkillUri> {
    let path = uri
        .strip_prefix("skill://")
        .ok_or_else(|| HostError::InvalidArgs(format!("unsupported skill uri {uri}")))?;
    if path.is_empty() || path == "root" {
        return Ok(SkillUri::Root);
    }
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        [name] if valid_name(name) => Ok(SkillUri::Active {
            name: (*name).to_string(),
        }),
        [name, "versions"] if valid_name(name) => Ok(SkillUri::Versions {
            name: (*name).to_string(),
        }),
        [name, "versions", digest]
            if valid_name(name)
                && digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) =>
        {
            Ok(SkillUri::Version {
                name: (*name).to_string(),
                digest: format!("sha256:{digest}"),
            })
        }
        _ => Err(HostError::InvalidArgs(format!(
            "unsupported skill uri {uri}"
        ))),
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn map_error(error: ManagedSkillError) -> HostError {
    HostError::NotFound(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sha2::{Digest, Sha256};
    use tm_host::{CapabilityGrants, ResourceRegistry};

    use super::*;
    use crate::ManagedSkillInstall;

    #[tokio::test]
    async fn managed_skill_resources_are_capability_gated_and_versioned() {
        let dir = tempfile::tempdir().unwrap();
        let body = "# Release notes\n\nGather commits and draft concise notes.\n";
        let digest = format!("sha256:{:x}", Sha256::digest(body.as_bytes()));
        let config = ModesConfig::default().with_managed_skills_path(dir.path());
        config
            .install_managed_skill(ManagedSkillInstall {
                name: "release-notes".to_string(),
                body: body.to_string(),
                content_digest: digest.clone(),
                source_proposal_id: "proposal-1".to_string(),
                description: "Draft release notes".to_string(),
                triggers: vec!["release notes".to_string()],
                use_criteria: "Use when asked for release notes.".to_string(),
            })
            .unwrap();

        let mut registry = ResourceRegistry::new();
        registry.register(Arc::new(SkillResourceHandler::new(config)));
        let denied = registry
            .read(
                "skill://release-notes",
                None,
                &InvocationCtx::new(CapabilityGrants::default()),
            )
            .await
            .unwrap_err();
        assert!(matches!(denied, HostError::CapabilityDenied(_)));

        let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:skill"));
        let active = registry
            .read("skill://release-notes", None, &ctx)
            .await
            .unwrap();
        assert_eq!(active.content, body);
        let listed = registry.list(Some("skill://"), &ctx).await.unwrap();
        assert_eq!(listed[0].uri, "skill://release-notes");
        let version_uri = format!(
            "skill://release-notes/versions/{}",
            digest.trim_start_matches("sha256:")
        );
        assert_eq!(
            registry
                .read(&version_uri, None, &ctx)
                .await
                .unwrap()
                .content,
            body
        );
    }
}

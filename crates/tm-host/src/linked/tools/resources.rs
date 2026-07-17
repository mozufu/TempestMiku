use super::*;

pub struct LinkedResourceHandler {
    linked: LinkedFolders,
}

impl LinkedResourceHandler {
    pub fn new(linked: LinkedFolders) -> Self {
        Self { linked }
    }
}

#[async_trait]
impl ResourceHandler for LinkedResourceHandler {
    fn scheme(&self) -> &str {
        "linked"
    }

    fn capability(&self) -> &str {
        "resources.read:linked"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> Result<ResourceContent> {
        ctx.require_linked_alias(&parse_linked_path(uri)?.alias)?;
        let linked = self.linked.clone();
        let uri = uri.to_string();
        let selector = selector.map(str::to_string);
        tokio::task::spawn_blocking(move || linked.read_resource(&uri, selector.as_deref()))
            .await
            .map_err(|err| HostError::HostCall(format!("linked read worker failed: {err}")))?
    }

    async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> Result<ResourceContent> {
        ctx.require_linked_alias(&parse_linked_path(uri)?.alias)?;
        let linked = self.linked.clone();
        let uri = uri.to_string();
        let mut content = tokio::task::spawn_blocking(move || linked.read_resource(&uri, None))
            .await
            .map_err(|err| HostError::HostCall(format!("linked preview worker failed: {err}")))??;
        content.content.clear();
        Ok(content)
    }

    async fn list(&self, uri: Option<&str>, ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        let resolved = self.linked.resolve_spec(uri)?;
        ctx.require_linked_alias(&resolved.alias)?;
        let revision = self.linked.revision();
        let linked = self.linked.clone();
        let stable_path = display_path(&resolved.alias, &resolved.relative);
        let entries = tokio::task::spawn_blocking(move || {
            linked.with_stable_policy_snapshot(revision, |linked| {
                let resolved = linked.resolve_spec(Some(&stable_path))?;
                list_entries(&resolved, false, 1000, false)
            })
        })
        .await
        .map_err(|err| HostError::HostCall(format!("linked list worker failed: {err}")))??;
        Ok(entries
            .into_iter()
            .map(|entry| ResourceEntry {
                uri: entry.uri,
                name: entry.name,
                kind: entry.kind,
                title: None,
                size_bytes: entry.size_bytes.map(|n| n as usize),
                modified_at: entry.modified_at,
            })
            .collect())
    }
}

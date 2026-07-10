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
        self.linked.read_resource(uri, selector)
    }

    async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> Result<ResourceContent> {
        ctx.require_linked_alias(&parse_linked_path(uri)?.alias)?;
        let mut content = self.linked.read_resource(uri, None)?;
        content.content.clear();
        Ok(content)
    }

    async fn list(&self, uri: Option<&str>, ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        let resolved = self.linked.resolve_existing(uri)?;
        ctx.require_linked_alias(&resolved.alias)?;
        list_entries(&resolved, false, 1000, false).map(|entries| {
            entries
                .into_iter()
                .map(|entry| ResourceEntry {
                    uri: entry.uri,
                    name: entry.name,
                    kind: entry.kind,
                    title: None,
                    size_bytes: entry.size_bytes.map(|n| n as usize),
                    modified_at: entry.modified_at,
                })
                .collect()
        })
    }
}

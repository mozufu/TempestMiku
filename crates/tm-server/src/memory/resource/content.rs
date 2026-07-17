use tm_artifacts::{ResourceContent, preview};
use tm_host::{HostError, Result as HostResult};

use crate::Store;

use super::MemoryResourceHandler;

impl<S> MemoryResourceHandler<S>
where
    S: Store,
{
    pub(super) fn text_resource(
        &self,
        uri: &str,
        kind: &str,
        title: Option<String>,
        content: String,
        selector: Option<&str>,
    ) -> HostResult<ResourceContent> {
        let size_bytes = content.len();
        let (selected, has_more) = select_memory_text(&content, selector)?;
        Ok(ResourceContent {
            uri: uri.to_string(),
            kind: kind.to_string(),
            mime: "text/plain".to_string(),
            title,
            size_bytes,
            selector: selector.map(str::to_string),
            has_more,
            preview: preview(&selected, self.preview_bytes),
            content: selected,
        })
    }

    pub(super) fn json_resource(
        &self,
        uri: &str,
        kind: &str,
        title: Option<String>,
        value: serde_json::Value,
        selector: Option<&str>,
    ) -> HostResult<ResourceContent> {
        let content = serde_json::to_string_pretty(&value)
            .map_err(|error| HostError::HostCall(error.to_string()))?;
        let mut resource = self.text_resource(uri, kind, title, content, selector)?;
        resource.mime = "application/json".to_string();
        Ok(resource)
    }
}

fn select_memory_text(content: &str, selector: Option<&str>) -> HostResult<(String, bool)> {
    let Some(selector) = selector else {
        return Ok((content.to_string(), false));
    };
    let (start, end) = selector
        .split_once('-')
        .ok_or_else(|| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let start: usize = start
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let end: usize = end
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    if start == 0 || end < start {
        return Err(HostError::InvalidArgs(format!(
            "invalid selector {selector}"
        )));
    }
    let lines = content.lines().collect::<Vec<_>>();
    let selected = lines
        .iter()
        .skip(start - 1)
        .take(end - start + 1)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Ok((selected, end < lines.len()))
}

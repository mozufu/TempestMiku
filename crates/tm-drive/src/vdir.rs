use crate::{DriveSearchOptions, DriveVirtualQuery, types::DriveError};

pub fn parse_virtual_dir(path: &str) -> Option<DriveVirtualQuery> {
    let path = path.trim();
    let path = path.strip_prefix("drive://").unwrap_or(path);
    let path = if path.is_empty() { "/recent" } else { path };
    let path = path.strip_prefix('/').unwrap_or(path);
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [] | ["recent"] => Some(DriveVirtualQuery {
            kind: "recent".to_string(),
            value: None,
            year: None,
            month: None,
        }),
        ["by-project", project] => Some(DriveVirtualQuery {
            kind: "by-project".to_string(),
            value: Some((*project).to_string()),
            year: None,
            month: None,
        }),
        ["by-type", doc_kind] => Some(DriveVirtualQuery {
            kind: "by-type".to_string(),
            value: Some((*doc_kind).to_string()),
            year: None,
            month: None,
        }),
        ["by-tag", tag] => Some(DriveVirtualQuery {
            kind: "by-tag".to_string(),
            value: Some((*tag).to_string()),
            year: None,
            month: None,
        }),
        ["by-date", year, month] => {
            let year = year.parse::<i32>().ok()?;
            let month = month.parse::<u32>().ok()?;
            (1..=12).contains(&month).then_some(DriveVirtualQuery {
                kind: "by-date".to_string(),
                value: None,
                year: Some(year),
                month: Some(month),
            })
        }
        _ => None,
    }
}

pub fn virtual_query_to_search(query: &DriveVirtualQuery, limit: usize) -> DriveSearchOptions {
    match query.kind.as_str() {
        "by-project" => DriveSearchOptions {
            project: query.value.clone(),
            limit,
            ..DriveSearchOptions::default()
        },
        "by-type" => DriveSearchOptions {
            doc_kind: query.value.clone(),
            limit,
            ..DriveSearchOptions::default()
        },
        "by-tag" => DriveSearchOptions {
            tags: query.value.clone().into_iter().collect(),
            limit,
            ..DriveSearchOptions::default()
        },
        _ => DriveSearchOptions {
            limit,
            ..DriveSearchOptions::default()
        },
    }
}

pub fn drive_uri_path(uri: &str) -> Result<String, DriveError> {
    uri.strip_prefix("drive://")
        .map(str::to_string)
        .ok_or_else(|| DriveError::InvalidPath(format!("unsupported drive uri {uri}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_virtual_dirs_to_queries() {
        assert_eq!(
            parse_virtual_dir("/by-project/TempestMiku").unwrap().kind,
            "by-project"
        );
        assert_eq!(
            parse_virtual_dir("drive://by-type/invoice")
                .unwrap()
                .value
                .as_deref(),
            Some("invoice")
        );
        assert!(parse_virtual_dir("/by-date/2026/07").is_some());
        assert!(parse_virtual_dir("/by-date/2026/99").is_none());
    }
}

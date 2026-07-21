use super::*;

pub(crate) fn sanitize_drive_bytes(bytes: &[u8]) -> crate::Result<Cow<'_, [u8]>> {
    match std::str::from_utf8(bytes) {
        Ok(text) => {
            let report = tm_memory::redact_dream_text(text);
            if report.redactions.is_empty() {
                Ok(Cow::Borrowed(bytes))
            } else {
                Ok(Cow::Owned(report.text.into_bytes()))
            }
        }
        Err(_) => {
            let searchable = String::from_utf8_lossy(bytes);
            if tm_memory::contains_sensitive_data(&searchable) {
                Err(DriveError::InvalidArgs(
                    "binary drive content matched the secret detector and was rejected".to_string(),
                ))
            } else {
                Ok(Cow::Borrowed(bytes))
            }
        }
    }
}

pub(crate) fn sanitize_drive_put_options(
    mut options: DrivePutOptions,
) -> crate::Result<DrivePutOptions> {
    for (field, value) in [
        ("suggestedPath", options.suggested_path.as_deref()),
        ("project", options.project.as_deref()),
        ("docKind", options.doc_kind.as_deref()),
        ("sourceUri", options.source_uri.as_deref()),
        ("mime", options.mime.as_deref()),
        ("sessionId", options.session_id.as_deref()),
        (
            "conventions.project",
            options.conventions.project.as_deref(),
        ),
        (
            "conventions.finance",
            options.conventions.finance.as_deref(),
        ),
        ("conventions.inbox", options.conventions.inbox.as_deref()),
        (
            "modelExtraction.role",
            options.model_extraction.role.as_deref(),
        ),
    ] {
        reject_sensitive_drive_identifier(field, value)?;
    }
    for tag in &options.tags {
        reject_sensitive_drive_identifier("tags", Some(tag))?;
    }
    for field in &options.model_extraction.fields {
        reject_sensitive_drive_identifier("modelExtraction.fields", Some(field))?;
    }
    options.title = options
        .title
        .map(|title| tm_memory::redact_dream_text(&title).text);
    Ok(options)
}

fn reject_sensitive_drive_identifier(field: &str, value: Option<&str>) -> crate::Result<()> {
    if value.is_some_and(tm_memory::contains_sensitive_data) {
        return Err(DriveError::InvalidArgs(format!(
            "drive {field} contains sensitive data"
        )));
    }
    Ok(())
}

pub(crate) fn validate_drive_identifier(field: &str, value: &str) -> crate::Result<()> {
    reject_sensitive_drive_identifier(field, Some(value))
}

pub fn normalize_canonical_path(input: &str) -> crate::Result<String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(DriveError::InvalidPath("empty drive path".to_string()));
    }
    if raw.contains('\0') {
        return Err(DriveError::InvalidPath(
            "path contains NUL byte".to_string(),
        ));
    }
    if raw.starts_with("linked://")
        || raw.starts_with("workspace://")
        || raw.starts_with("artifact://")
    {
        return Err(DriveError::InvalidPath(format!(
            "drive paths cannot be host/resource refs: {raw}"
        )));
    }
    let raw = raw.strip_prefix("drive://").unwrap_or(raw);
    let raw = raw.replace('\\', "/");
    if raw.starts_with('/')
        || raw.starts_with("~/")
        || raw
            .split('/')
            .next()
            .is_some_and(|first| first.ends_with(':'))
    {
        return Err(DriveError::InvalidPath(format!(
            "raw host paths are not allowed: {input}"
        )));
    }
    let path = Path::new(&raw);
    if path.is_absolute() {
        return Err(DriveError::InvalidPath(format!(
            "absolute paths are not allowed: {input}"
        )));
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let Some(part) = part.to_str() else {
                    return Err(DriveError::InvalidPath(format!(
                        "path is not valid UTF-8: {input}"
                    )));
                };
                if part.trim().is_empty() {
                    continue;
                }
                parts.push(part.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(DriveError::InvalidPath(format!(
                    "path traversal is not allowed: {input}"
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(DriveError::InvalidPath("empty drive path".to_string()));
    }
    Ok(parts.join("/"))
}

pub(crate) fn normalize_canonical_path_path_or_uri(input: &str) -> crate::Result<String> {
    normalize_canonical_path(input)
}

pub(crate) fn normalize_optional_prefix(input: &str) -> crate::Result<String> {
    if input.trim().is_empty() || input.trim() == "/" || input.trim() == "drive://" {
        Ok(String::new())
    } else {
        normalize_canonical_path(input)
    }
}

pub(crate) fn drive_error_to_host(err: DriveError) -> HostError {
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

pub(crate) fn drive_put_requires_approval(options: &DrivePutOptions) -> bool {
    options.approval_mode == crate::DriveApprovalMode::RequireApproval
        || (options.auto && options.approval_mode == crate::DriveApprovalMode::Propose)
        || options.overwrite
        || options.collision == DriveCollisionStrategy::Overwrite
}

pub(crate) fn host_drive_put_options(mut options: DrivePutOptions) -> DrivePutOptions {
    if options.approval_mode == crate::DriveApprovalMode::Auto {
        options.approval_mode = crate::DriveApprovalMode::Propose;
    }
    options
}

pub(crate) fn linked_alias_from_target(target: &str) -> tm_host::Result<String> {
    let target = target.trim();
    if target.is_empty() {
        return Err(HostError::InvalidArgs(
            "project.unlink requires a linked folder alias".to_string(),
        ));
    }
    if let Some(rest) = target.strip_prefix("linked://") {
        let alias = rest
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or_default()
            .trim();
        if alias.is_empty() {
            return Err(HostError::InvalidPath(format!(
                "invalid linked folder uri {target}"
            )));
        }
        return Ok(alias.to_string());
    }
    if target.contains("://") || target.contains('/') || target.contains('\\') {
        return Err(HostError::InvalidPath(format!(
            "project.unlink expects an alias or linked:// URI, got {target}"
        )));
    }
    Ok(target.to_string())
}

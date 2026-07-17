use super::*;
pub(super) static SESSION_WRITE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
    OnceLock::new();

pub(super) fn session_write_lock(path: &Path) -> Arc<Mutex<()>> {
    let locks = SESSION_WRITE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock();
    if let Some(existing) = locks.get(path).and_then(Weak::upgrade) {
        return existing;
    }
    locks.retain(|_, lock| lock.strong_count() > 0);
    let lock = Arc::new(Mutex::new(()));
    locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
    lock
}

#[derive(Debug)]
pub(super) struct ArtifactNamespaceUsage {
    pub(super) refs: Vec<ArtifactRef>,
    pub(super) count: usize,
    pub(super) metadata_bytes: usize,
    pub(super) content_bytes: usize,
    pub(super) max_id: Option<u64>,
}

/// Scan a session namespace with hard entry, count, metadata, and content bounds.
///
/// Counting every directory entry, including interrupted temporary/lock files and
/// unknown names, prevents an attacker with access to the storage directory from
/// turning an otherwise bounded artifact operation into an unbounded scan.
pub(super) fn artifact_namespace_usage(
    session_dir: &Path,
    limits: ArtifactLimits,
) -> Result<ArtifactNamespaceUsage> {
    let max_directory_entries = limits
        .max_artifact_count
        .checked_mul(3)
        .and_then(|count| count.checked_add(64))
        .ok_or_else(|| ArtifactError::InvalidLimits(format!("{limits:?}")))?;
    let mut directory_entries = 0usize;
    let mut refs = Vec::new();
    let mut metadata_bytes = 0usize;
    let mut content_bytes = 0usize;
    let mut max_id = None;

    for entry in fs::read_dir(session_dir)? {
        let entry = entry?;
        directory_entries =
            directory_entries
                .checked_add(1)
                .ok_or(ArtifactError::QuotaExceeded {
                    resource: "artifact namespace entries",
                    attempted: usize::MAX,
                    limit: max_directory_entries,
                })?;
        if directory_entries > max_directory_entries {
            return Err(ArtifactError::QuotaExceeded {
                resource: "artifact namespace entries",
                attempted: directory_entries,
                limit: max_directory_entries,
            });
        }

        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("meta") {
            continue;
        }
        if refs.len() >= limits.max_artifact_count {
            return Err(ArtifactError::QuotaExceeded {
                resource: "artifact count",
                attempted: refs.len().saturating_add(1),
                limit: limits.max_artifact_count,
            });
        }
        let metadata = ensure_managed_file(session_dir, &path, &path.display().to_string())?;
        let metadata_len = usize::try_from(metadata.len())
            .map_err(|_| ArtifactError::Integrity(path.display().to_string()))?;
        metadata_bytes =
            metadata_bytes
                .checked_add(metadata_len)
                .ok_or(ArtifactError::QuotaExceeded {
                    resource: "session artifact metadata",
                    attempted: usize::MAX,
                    limit: limits.max_session_metadata_bytes,
                })?;
        if metadata_bytes > limits.max_session_metadata_bytes {
            return Err(ArtifactError::QuotaExceeded {
                resource: "session artifact metadata",
                attempted: metadata_bytes,
                limit: limits.max_session_metadata_bytes,
            });
        }

        let encoded = read_bounded_file(
            &path,
            MAX_ARTIFACT_METADATA_BYTES,
            &path.display().to_string(),
        )?;
        let artifact: ArtifactRef = serde_json::from_slice(&encoded).map_err(io::Error::other)?;
        validate_loaded_ref(&path, &artifact)?;
        let content_path = session_dir.join(format!("{}.txt", artifact.id));
        let content_metadata = ensure_managed_file(session_dir, &content_path, &artifact.uri)?;
        let actual_size = usize::try_from(content_metadata.len())
            .map_err(|_| ArtifactError::Integrity(artifact.uri.clone()))?;
        if actual_size != artifact.size_bytes {
            return Err(ArtifactError::Integrity(artifact.uri));
        }
        content_bytes =
            content_bytes
                .checked_add(actual_size)
                .ok_or(ArtifactError::QuotaExceeded {
                    resource: "session",
                    attempted: usize::MAX,
                    limit: limits.max_session_bytes,
                })?;
        if content_bytes > limits.max_session_bytes {
            return Err(ArtifactError::QuotaExceeded {
                resource: "session",
                attempted: content_bytes,
                limit: limits.max_session_bytes,
            });
        }
        let id = ArtifactId::parse(&artifact.id)?.get();
        max_id = Some(max_id.map_or(id, |current: u64| current.max(id)));
        refs.push(artifact);
    }
    refs.sort_by_key(|artifact| ArtifactId::parse(&artifact.id).map_or(u64::MAX, ArtifactId::get));

    Ok(ArtifactNamespaceUsage {
        count: refs.len(),
        refs,
        metadata_bytes,
        content_bytes,
        max_id,
    })
}

#[derive(Debug, Default)]
pub(super) struct BlobReferenceUsage {
    pub(super) count: usize,
    pub(super) metadata_bytes: usize,
    pub(super) content_bytes: usize,
}

pub(super) fn blob_reference_usage(
    blob_refs_dir: &Path,
    blob_dir: &Path,
    limits: ArtifactLimits,
) -> Result<BlobReferenceUsage> {
    let max_directory_entries = limits
        .max_blob_count
        .checked_mul(2)
        .and_then(|count| count.checked_add(32))
        .ok_or_else(|| ArtifactError::InvalidLimits(format!("{limits:?}")))?;
    let mut directory_entries = 0usize;
    let mut usage = BlobReferenceUsage::default();
    for entry in fs::read_dir(blob_refs_dir)? {
        let entry = entry?;
        directory_entries =
            directory_entries
                .checked_add(1)
                .ok_or(ArtifactError::QuotaExceeded {
                    resource: "blob-reference namespace entries",
                    attempted: usize::MAX,
                    limit: max_directory_entries,
                })?;
        if directory_entries > max_directory_entries {
            return Err(ArtifactError::QuotaExceeded {
                resource: "blob-reference namespace entries",
                attempted: directory_entries,
                limit: max_directory_entries,
            });
        }
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("ref") {
            continue;
        }
        if usage.count >= limits.max_blob_count {
            return Err(ArtifactError::QuotaExceeded {
                resource: "blob reference count",
                attempted: usage.count.saturating_add(1),
                limit: limits.max_blob_count,
            });
        }
        let metadata = ensure_managed_file(blob_refs_dir, &path, &path.display().to_string())?;
        let metadata_len = usize::try_from(metadata.len())
            .map_err(|_| ArtifactError::Integrity(path.display().to_string()))?;
        usage.metadata_bytes =
            usage
                .metadata_bytes
                .checked_add(metadata_len)
                .ok_or(ArtifactError::QuotaExceeded {
                    resource: "session blob-reference metadata",
                    attempted: usize::MAX,
                    limit: limits.max_session_blob_ref_bytes,
                })?;
        if usage.metadata_bytes > limits.max_session_blob_ref_bytes {
            return Err(ArtifactError::QuotaExceeded {
                resource: "session blob-reference metadata",
                attempted: usage.metadata_bytes,
                limit: limits.max_session_blob_ref_bytes,
            });
        }
        let size = validate_blob_reference(&path, blob_dir, limits)?;
        usage.content_bytes =
            usage
                .content_bytes
                .checked_add(size)
                .ok_or(ArtifactError::QuotaExceeded {
                    resource: "session",
                    attempted: usize::MAX,
                    limit: limits.max_session_bytes,
                })?;
        if usage.content_bytes > limits.max_session_bytes {
            return Err(ArtifactError::QuotaExceeded {
                resource: "session",
                attempted: usage.content_bytes,
                limit: limits.max_session_bytes,
            });
        }
        usage.count = usage.count.saturating_add(1);
    }
    Ok(usage)
}

pub(super) fn validate_blob_reference(
    reference_path: &Path,
    blob_dir: &Path,
    limits: ArtifactLimits,
) -> Result<usize> {
    ensure_managed_file(
        reference_path
            .parent()
            .ok_or_else(|| ArtifactError::Integrity(reference_path.display().to_string()))?,
        reference_path,
        &reference_path.display().to_string(),
    )?;
    let stem = reference_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| ArtifactError::Integrity(reference_path.display().to_string()))?;
    let blob_id = BlobId::parse_hash(stem)
        .map_err(|_| ArtifactError::Integrity(reference_path.display().to_string()))?;
    let stored = read_bounded_file(reference_path, 32, &reference_path.display().to_string())?;
    let stored = std::str::from_utf8(&stored)
        .map_err(|_| ArtifactError::Integrity(reference_path.display().to_string()))?;
    let size = stored
        .parse::<usize>()
        .map_err(|_| ArtifactError::Integrity(reference_path.display().to_string()))?;
    if size.to_string() != stored || size > limits.max_blob_bytes {
        return Err(ArtifactError::Integrity(
            reference_path.display().to_string(),
        ));
    }
    let blob_path = blob_dir.join(blob_id.hash());
    let metadata = ensure_managed_file(blob_dir, &blob_path, &blob_id.uri())?;
    if metadata.len() != size as u64 {
        return Err(ArtifactError::Integrity(blob_id.uri()));
    }
    Ok(size)
}

pub(super) fn validate_limits(limits: ArtifactLimits) -> Result<()> {
    if limits.max_artifact_bytes == 0
        || limits.max_blob_bytes == 0
        || limits.max_session_bytes == 0
        || limits.max_artifact_count == 0
        || limits.max_session_metadata_bytes == 0
        || limits.max_blob_count == 0
        || limits.max_session_blob_ref_bytes == 0
        || limits.default_page_lines == 0
        || limits.default_page_bytes == 0
        || limits.max_page_lines == 0
        || limits.max_page_bytes == 0
        || limits.default_page_lines > limits.max_page_lines
        || limits.default_page_bytes > limits.max_page_bytes
        || limits.max_artifact_bytes > limits.max_session_bytes
    {
        return Err(ArtifactError::InvalidLimits(format!("{limits:?}")));
    }
    Ok(())
}

pub(super) fn validate_loaded_ref(meta_path: &Path, artifact: &ArtifactRef) -> Result<()> {
    let stem = meta_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| ArtifactError::Integrity(meta_path.display().to_string()))?;
    let id = ArtifactId::parse_uri(&artifact.uri)?;
    if artifact.id != stem || id.to_string() != stem || artifact.uri != format!("artifact://{stem}")
    {
        return Err(ArtifactError::Integrity(artifact.uri.clone()));
    }
    if artifact.kind != "text"
        || artifact.mime.len() > MAX_ARTIFACT_MIME_BYTES
        || artifact
            .title
            .as_ref()
            .is_some_and(|title| title.len() > MAX_ARTIFACT_TITLE_BYTES)
        || artifact.preview.len() > preview_marker_bound(1024)
    {
        return Err(ArtifactError::Integrity(artifact.uri.clone()));
    }
    Ok(())
}

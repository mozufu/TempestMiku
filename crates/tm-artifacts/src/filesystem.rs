use super::*;
pub(super) fn preview_marker_bound(cap: usize) -> usize {
    // `preview` appends a newline, ellipsis, decimal byte count, and a fixed suffix.
    cap.saturating_add(64)
}

pub(super) fn ensure_regular_file(path: &Path, identity: &str) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(ArtifactError::Integrity(identity.to_string()));
    }
    Ok(metadata)
}

pub(super) fn ensure_directory(path: &Path, identity: &str) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(ArtifactError::Integrity(identity.to_string()));
    }
    Ok(metadata)
}

pub(super) fn ensure_managed_directory(root: &Path, path: &Path) -> Result<fs::Metadata> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err.into()),
        },
        Err(err) => return Err(err.into()),
    }
    let metadata = ensure_directory(path, &path.display().to_string())?;
    ensure_canonical_containment(root, path, &path.display().to_string())?;
    Ok(metadata)
}

pub(super) fn ensure_managed_file(
    root: &Path,
    path: &Path,
    identity: &str,
) -> Result<fs::Metadata> {
    let metadata = ensure_regular_file(path, identity)?;
    ensure_canonical_containment(root, path, identity)?;
    Ok(metadata)
}

pub(super) fn ensure_canonical_containment(root: &Path, path: &Path, identity: &str) -> Result<()> {
    let canonical_root = fs::canonicalize(root)?;
    let canonical_path = fs::canonicalize(path)?;
    if canonical_path.starts_with(&canonical_root) {
        Ok(())
    } else {
        Err(ArtifactError::Integrity(identity.to_string()))
    }
}

static TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("artifact path has no parent"))?;
    fs::create_dir_all(parent)?;
    let suffix = TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("artifact path is not valid UTF-8"))?;
    let temp = parent.join(format!(".{file_name}.tmp-{}-{suffix}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        // Linking the fully-written temporary file into place gives us an atomic
        // create-if-absent operation. `rename` is not suitable here because it
        // replaces an existing destination on Unix.
        match fs::hard_link(&temp, path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = ensure_regular_file(path, &path.display().to_string())?;
                if metadata.len() != bytes.len() as u64 {
                    return Err(ArtifactError::Integrity(path.display().to_string()));
                }
                let existing = fs::read(path)?;
                if existing == bytes {
                    Ok(())
                } else {
                    Err(ArtifactError::Integrity(path.display().to_string()))
                }
            }
            Err(err) => Err(err.into()),
        }
    })();
    let _ = fs::remove_file(temp);
    result
}

pub(super) fn read_bounded_file(path: &Path, limit: usize, identity: &str) -> Result<Vec<u8>> {
    let metadata = ensure_regular_file(path, identity)?;
    if metadata.len() > limit as u64 {
        return Err(ArtifactError::Integrity(identity.to_string()));
    }
    let file = File::open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(ArtifactError::Integrity(identity.to_string()));
    }
    Ok(bytes)
}

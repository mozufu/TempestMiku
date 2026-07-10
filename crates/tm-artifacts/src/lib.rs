//! Repo-local artifact and blob storage.
//!
//! Artifacts are session-local monotonic `artifact://<id>` handles backed by files.
//! Blobs are global content-addressed `blob:sha256:<hash>` files for deduplication.

use std::{
    collections::HashMap,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub type Result<T, E = ArtifactError> = std::result::Result<T, E>;

const MAX_ARTIFACT_METADATA_BYTES: usize = 64 * 1024;

/// Validated identifier for one artifact namespace.
///
/// Session identifiers are deliberately path-hostile: they are non-empty, bounded ASCII and may
/// never contain separators or the special `.` / `..` path components.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    pub fn parse(value: &str) -> Result<Self> {
        let valid = !value.is_empty()
            && value.len() <= 128
            && value != "."
            && value != ".."
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
        if !valid {
            return Err(ArtifactError::InvalidUri(format!(
                "invalid session id {value:?}"
            )));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Canonical decimal artifact identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArtifactId(u64);

impl ArtifactId {
    pub fn parse(value: &str) -> Result<Self> {
        let parsed = value
            .parse::<u64>()
            .map_err(|_| ArtifactError::InvalidUri(value.to_string()))?;
        if parsed.to_string() != value {
            return Err(ArtifactError::InvalidUri(value.to_string()));
        }
        Ok(Self(parsed))
    }

    pub fn parse_uri(uri: &str) -> Result<Self> {
        let value = uri
            .strip_prefix("artifact://")
            .ok_or_else(|| ArtifactError::InvalidUri(uri.to_string()))?;
        Self::parse(value).map_err(|_| ArtifactError::InvalidUri(uri.to_string()))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Canonical lowercase SHA-256 blob identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlobId(String);

impl BlobId {
    pub fn parse_hash(hash: &str) -> Result<Self> {
        if hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(hash.to_string()))
        } else {
            Err(ArtifactError::InvalidUri(format!("blob:sha256:{hash}")))
        }
    }

    pub fn parse_uri(uri: &str) -> Result<Self> {
        let hash = uri
            .strip_prefix("blob:sha256:")
            .ok_or_else(|| ArtifactError::InvalidUri(uri.to_string()))?;
        Self::parse_hash(hash).map_err(|_| ArtifactError::InvalidUri(uri.to_string()))
    }

    pub fn for_bytes(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    pub fn hash(&self) -> &str {
        &self.0
    }

    pub fn uri(&self) -> String {
        format!("blob:sha256:{}", self.hash())
    }
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("artifact not found: {uri}; available: {available:?}")]
    NotFound { uri: String, available: Vec<String> },
    #[error("invalid artifact uri: {0}")]
    InvalidUri(String),
    #[error("invalid selector: {0}")]
    InvalidSelector(String),
    #[error(
        "artifact quota exceeded for {resource}: attempted {attempted} bytes, limit {limit} bytes"
    )]
    QuotaExceeded {
        resource: &'static str,
        attempted: usize,
        limit: usize,
    },
    #[error("artifact integrity check failed for {0}")]
    Integrity(String),
    #[error("invalid artifact limits: {0}")]
    InvalidLimits(String),
}

/// Storage and paging limits for one artifact-store handle.
///
/// The defaults keep existing small artifacts fully readable while bounding any
/// single model-visible page and preventing unbounded disk growth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactLimits {
    pub max_artifact_bytes: usize,
    pub max_blob_bytes: usize,
    pub max_session_bytes: usize,
    pub default_page_lines: usize,
    pub default_page_bytes: usize,
    pub max_page_lines: usize,
    pub max_page_bytes: usize,
}

impl Default for ArtifactLimits {
    fn default() -> Self {
        Self {
            max_artifact_bytes: 4 * 1024 * 1024,
            max_blob_bytes: 64 * 1024 * 1024,
            max_session_bytes: 256 * 1024 * 1024,
            default_page_lines: 200,
            default_page_bytes: 64 * 1024,
            max_page_lines: 1_000,
            max_page_bytes: 256 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub uri: String,
    pub id: String,
    pub kind: String,
    pub mime: String,
    pub title: Option<String>,
    pub size_bytes: usize,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    pub kind: String,
    pub mime: String,
    pub title: Option<String>,
    pub size_bytes: usize,
    pub selector: Option<String>,
    pub has_more: bool,
    pub content: String,
    pub preview: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
    session_id: SessionId,
    limits: ArtifactLimits,
    inner: Arc<Mutex<Inner>>,
    session_write_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Default)]
struct Inner {
    next_id: u64,
    total_bytes: usize,
    refs: Vec<ArtifactRef>,
}

impl ArtifactStore {
    pub fn open(root: impl Into<PathBuf>, session_id: impl AsRef<str>) -> Result<Self> {
        Self::open_with_limits(root, session_id, ArtifactLimits::default())
    }

    pub fn open_with_limits(
        root: impl Into<PathBuf>,
        session_id: impl AsRef<str>,
        limits: ArtifactLimits,
    ) -> Result<Self> {
        let session_id = SessionId::parse(session_id.as_ref())?;
        validate_limits(limits)?;
        let root = root.into();
        fs::create_dir_all(&root)?;
        ensure_directory(&root, &root.display().to_string())?;
        let root = fs::canonicalize(root)?;
        let sessions_dir = root.join("sessions");
        let blob_dir = root.join("blobs");
        ensure_managed_directory(&root, &sessions_dir)?;
        ensure_managed_directory(&root, &blob_dir)?;
        let session_root = sessions_dir.join(session_id.as_str());
        ensure_managed_directory(&sessions_dir, &session_root)?;
        let session_dir = session_root.join("artifacts");
        ensure_managed_directory(&sessions_dir, &session_dir)?;
        let blob_refs_dir = session_root.join("blob_refs");
        ensure_managed_directory(&session_root, &blob_refs_dir)?;
        let session_write_lock = session_write_lock(&fs::canonicalize(&session_dir)?);
        let _write_guard = session_write_lock.lock();

        let mut refs = Vec::new();
        let mut max_id = None;
        for entry in fs::read_dir(&session_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("meta") {
                continue;
            }
            ensure_managed_file(&session_dir, &path, &path.display().to_string())?;
            let meta = read_bounded_file(
                &path,
                MAX_ARTIFACT_METADATA_BYTES,
                &path.display().to_string(),
            )?;
            let artifact: ArtifactRef = serde_json::from_slice(&meta).map_err(io::Error::other)?;
            validate_loaded_ref(&path, &artifact)?;
            let content_path = session_dir.join(format!("{}.txt", artifact.id));
            let content_metadata = ensure_managed_file(&session_dir, &content_path, &artifact.uri)?;
            let actual_size = usize::try_from(content_metadata.len())
                .map_err(|_| ArtifactError::Integrity(format!("artifact://{}", artifact.id)))?;
            if actual_size != artifact.size_bytes {
                return Err(ArtifactError::Integrity(artifact.uri));
            }
            if let Ok(id) = artifact.id.parse::<u64>() {
                max_id = Some(max_id.map_or(id, |m: u64| m.max(id)));
            }
            refs.push(artifact);
        }
        refs.sort_by_key(|r| r.id.parse::<u64>().unwrap_or(u64::MAX));
        let next_id = max_id.map_or(0, |id| id + 1);
        let artifact_bytes = refs.iter().try_fold(0usize, |total, artifact| {
            total
                .checked_add(artifact.size_bytes)
                .ok_or(ArtifactError::QuotaExceeded {
                    resource: "session",
                    attempted: usize::MAX,
                    limit: limits.max_session_bytes,
                })
        })?;
        let blob_bytes = blob_reference_bytes(&blob_refs_dir, &blob_dir, limits)?;
        let total_bytes =
            artifact_bytes
                .checked_add(blob_bytes)
                .ok_or(ArtifactError::QuotaExceeded {
                    resource: "session",
                    attempted: usize::MAX,
                    limit: limits.max_session_bytes,
                })?;
        if total_bytes > limits.max_session_bytes {
            return Err(ArtifactError::QuotaExceeded {
                resource: "session",
                attempted: total_bytes,
                limit: limits.max_session_bytes,
            });
        }

        Ok(Self {
            root,
            session_id,
            limits,
            inner: Arc::new(Mutex::new(Inner {
                next_id,
                total_bytes,
                refs,
            })),
            session_write_lock: Arc::clone(&session_write_lock),
        })
    }

    pub fn put_text(
        &self,
        content: impl AsRef<str>,
        title: Option<String>,
        mime: &str,
    ) -> Result<ArtifactRef> {
        let content = content.as_ref();
        let bytes = content.as_bytes();
        self.check_item_quota(bytes.len(), "artifact", self.limits.max_artifact_bytes)?;
        let dir = self.validated_session_artifact_dir()?;
        let blob_refs_dir = self.validated_session_blob_ref_dir()?;
        let blob_dir = self.validated_blob_dir()?;
        let _write_guard = self.session_write_lock.lock();
        let (id, artifact, lock_path) = {
            let mut inner = self.inner.lock();
            let current_bytes =
                session_bytes_on_disk(&dir, &blob_refs_dir, &blob_dir, self.limits)?;
            let attempted =
                current_bytes
                    .checked_add(bytes.len())
                    .ok_or(ArtifactError::QuotaExceeded {
                        resource: "session",
                        attempted: usize::MAX,
                        limit: self.limits.max_session_bytes,
                    })?;
            if attempted > self.limits.max_session_bytes {
                return Err(ArtifactError::QuotaExceeded {
                    resource: "session",
                    attempted,
                    limit: self.limits.max_session_bytes,
                });
            }
            let mut id = inner
                .next_id
                .max(max_artifact_id(&dir)?.map_or(0, |id| id + 1));
            loop {
                let lock_path = dir.join(format!("{id}.lock"));
                match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&lock_path)
                {
                    Ok(_) => {
                        if dir.join(format!("{id}.meta")).exists() {
                            let _ = fs::remove_file(&lock_path);
                            id += 1;
                            continue;
                        }
                        inner.next_id = id + 1;
                        let id_s = id.to_string();
                        let artifact = ArtifactRef {
                            uri: format!("artifact://{id_s}"),
                            id: id_s,
                            kind: "text".to_string(),
                            mime: mime.to_string(),
                            title,
                            size_bytes: bytes.len(),
                            preview: preview(content, 1024),
                        };
                        break (id, artifact, lock_path);
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                        id += 1;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        };

        let write_result = (|| -> Result<()> {
            write_atomic(&dir.join(format!("{id}.txt")), bytes)?;
            let metadata = serde_json::to_vec_pretty(&artifact).map_err(io::Error::other)?;
            self.check_item_quota(
                metadata.len(),
                "artifact metadata",
                MAX_ARTIFACT_METADATA_BYTES,
            )?;
            write_atomic(&dir.join(format!("{id}.meta")), &metadata)?;
            Ok(())
        })();
        let _ = fs::remove_file(&lock_path);
        if let Err(err) = write_result {
            let _ = fs::remove_file(dir.join(format!("{id}.txt")));
            let _ = fs::remove_file(dir.join(format!("{id}.meta")));
            return Err(err);
        }
        let mut inner = self.inner.lock();
        inner.total_bytes = inner.total_bytes.saturating_add(bytes.len());
        inner.refs.push(artifact.clone());
        Ok(artifact)
    }

    pub fn put_blob(&self, bytes: &[u8]) -> Result<String> {
        self.check_item_quota(bytes.len(), "blob", self.limits.max_blob_bytes)?;
        let id = BlobId::for_bytes(bytes);
        let uri = id.uri();
        let blob_dir = self.validated_blob_dir()?;
        let artifact_dir = self.validated_session_artifact_dir()?;
        let blob_refs_dir = self.validated_session_blob_ref_dir()?;
        let reference_path = blob_refs_dir.join(format!("{}.ref", id.hash()));
        let _write_guard = self.session_write_lock.lock();
        let new_reference = match fs::symlink_metadata(&reference_path) {
            Ok(_) => {
                validate_blob_reference(&reference_path, &blob_dir, self.limits)?;
                false
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let current_bytes =
                    session_bytes_on_disk(&artifact_dir, &blob_refs_dir, &blob_dir, self.limits)?;
                let attempted =
                    current_bytes
                        .checked_add(bytes.len())
                        .ok_or(ArtifactError::QuotaExceeded {
                            resource: "session",
                            attempted: usize::MAX,
                            limit: self.limits.max_session_bytes,
                        })?;
                if attempted > self.limits.max_session_bytes {
                    return Err(ArtifactError::QuotaExceeded {
                        resource: "session",
                        attempted,
                        limit: self.limits.max_session_bytes,
                    });
                }
                true
            }
            Err(err) => return Err(err.into()),
        };
        let path = blob_dir.join(id.hash());
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                self.read_blob(&uri)?;
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => write_atomic(&path, bytes)?,
            Err(err) => return Err(err.into()),
        }
        if new_reference {
            write_atomic(&reference_path, bytes.len().to_string().as_bytes())?;
            let mut inner = self.inner.lock();
            inner.total_bytes = inner.total_bytes.saturating_add(bytes.len());
        }
        Ok(uri)
    }

    pub fn read_blob(&self, uri: &str) -> Result<Vec<u8>> {
        let id = BlobId::parse_uri(uri)?;
        let blob_dir = self.root.join("blobs");
        ensure_managed_directory(&self.root, &blob_dir)?;
        let path = blob_dir.join(id.hash());
        let metadata = match ensure_regular_file(&path, uri) {
            Ok(metadata) => metadata,
            Err(ArtifactError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
                return Err(ArtifactError::NotFound {
                    uri: uri.to_string(),
                    available: Vec::new(),
                });
            }
            Err(err) => return Err(err),
        };
        let size = usize::try_from(metadata.len())
            .map_err(|_| ArtifactError::Integrity(uri.to_string()))?;
        self.check_item_quota(size, "blob", self.limits.max_blob_bytes)?;
        ensure_managed_file(&blob_dir, &path, uri)?;
        let file = File::open(&path).map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                ArtifactError::NotFound {
                    uri: uri.to_string(),
                    available: Vec::new(),
                }
            } else {
                ArtifactError::Io(err)
            }
        })?;
        let mut bytes = Vec::with_capacity(size);
        file.take(self.limits.max_blob_bytes as u64 + 1)
            .read_to_end(&mut bytes)?;
        self.check_item_quota(bytes.len(), "blob", self.limits.max_blob_bytes)?;
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != id.hash() {
            return Err(ArtifactError::Integrity(uri.to_string()));
        }
        Ok(bytes)
    }

    pub fn read(&self, uri: &str, selector: Option<&str>) -> Result<ResourceContent> {
        let id = ArtifactId::parse_uri(uri)?;
        let (artifact, available) = {
            let inner = self.inner.lock();
            (
                inner
                    .refs
                    .iter()
                    .find(|artifact| artifact.id == id.to_string())
                    .cloned(),
                inner.refs.iter().map(|a| a.uri.clone()).collect::<Vec<_>>(),
            )
        };
        let artifact = artifact.ok_or_else(|| ArtifactError::NotFound {
            uri: uri.to_string(),
            available,
        })?;
        let artifact_dir = self.validated_session_artifact_dir()?;
        let content_path = artifact_dir.join(format!("{id}.txt"));
        ensure_managed_file(&artifact_dir, &content_path, uri)?;
        let (selected, has_more) = select_text_file(&content_path, selector, self.limits)?;
        Ok(ResourceContent {
            uri: artifact.uri,
            kind: artifact.kind,
            mime: artifact.mime,
            title: artifact.title,
            size_bytes: artifact.size_bytes,
            selector: selector.map(str::to_string),
            has_more,
            preview: artifact.preview,
            content: selected,
        })
    }

    pub fn list(&self) -> Vec<ArtifactRef> {
        self.inner.lock().refs.clone()
    }

    pub fn limits(&self) -> ArtifactLimits {
        self.limits
    }

    fn check_item_quota(&self, size: usize, resource: &'static str, limit: usize) -> Result<()> {
        if size > limit {
            return Err(ArtifactError::QuotaExceeded {
                resource,
                attempted: size,
                limit,
            });
        }
        Ok(())
    }

    fn session_artifact_dir(&self) -> PathBuf {
        self.session_root().join("artifacts")
    }

    fn session_root(&self) -> PathBuf {
        self.root.join("sessions").join(self.session_id.as_str())
    }

    fn validated_session_artifact_dir(&self) -> Result<PathBuf> {
        let sessions = self.root.join("sessions");
        ensure_managed_directory(&self.root, &sessions)?;
        let session = sessions.join(self.session_id.as_str());
        ensure_managed_directory(&sessions, &session)?;
        let artifacts = self.session_artifact_dir();
        ensure_managed_directory(&session, &artifacts)?;
        Ok(artifacts)
    }

    fn validated_session_blob_ref_dir(&self) -> Result<PathBuf> {
        let sessions = self.root.join("sessions");
        ensure_managed_directory(&self.root, &sessions)?;
        let session = self.session_root();
        ensure_managed_directory(&sessions, &session)?;
        let references = session.join("blob_refs");
        ensure_managed_directory(&session, &references)?;
        Ok(references)
    }

    fn validated_blob_dir(&self) -> Result<PathBuf> {
        let blobs = self.root.join("blobs");
        ensure_managed_directory(&self.root, &blobs)?;
        Ok(blobs)
    }
}

static SESSION_WRITE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();

fn session_write_lock(path: &Path) -> Arc<Mutex<()>> {
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

fn session_bytes_on_disk(
    session_dir: &Path,
    blob_refs_dir: &Path,
    blob_dir: &Path,
    limits: ArtifactLimits,
) -> Result<usize> {
    let mut total = 0usize;
    for entry in fs::read_dir(session_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("txt") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if stem.parse::<u64>().is_err() {
            continue;
        }
        let metadata = ensure_regular_file(&path, &path.display().to_string())?;
        let size = usize::try_from(metadata.len())
            .map_err(|_| ArtifactError::Integrity(path.display().to_string()))?;
        total = total
            .checked_add(size)
            .ok_or(ArtifactError::QuotaExceeded {
                resource: "session",
                attempted: usize::MAX,
                limit: usize::MAX,
            })?;
    }
    total
        .checked_add(blob_reference_bytes(blob_refs_dir, blob_dir, limits)?)
        .ok_or(ArtifactError::QuotaExceeded {
            resource: "session",
            attempted: usize::MAX,
            limit: limits.max_session_bytes,
        })
}

fn blob_reference_bytes(
    blob_refs_dir: &Path,
    blob_dir: &Path,
    limits: ArtifactLimits,
) -> Result<usize> {
    let mut total = 0usize;
    for entry in fs::read_dir(blob_refs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("ref") {
            continue;
        }
        let size = validate_blob_reference(&path, blob_dir, limits)?;
        total = total
            .checked_add(size)
            .ok_or(ArtifactError::QuotaExceeded {
                resource: "session",
                attempted: usize::MAX,
                limit: limits.max_session_bytes,
            })?;
    }
    Ok(total)
}

fn validate_blob_reference(
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

fn max_artifact_id(session_dir: &Path) -> io::Result<Option<u64>> {
    let mut max_id = None;
    for entry in fs::read_dir(session_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("meta") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if let Ok(id) = stem.parse::<u64>() {
            max_id = Some(max_id.map_or(id, |m: u64| m.max(id)));
        }
    }
    Ok(max_id)
}

fn validate_limits(limits: ArtifactLimits) -> Result<()> {
    if limits.max_artifact_bytes == 0
        || limits.max_blob_bytes == 0
        || limits.max_session_bytes == 0
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

fn validate_loaded_ref(meta_path: &Path, artifact: &ArtifactRef) -> Result<()> {
    let stem = meta_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| ArtifactError::Integrity(meta_path.display().to_string()))?;
    let id = ArtifactId::parse_uri(&artifact.uri)?;
    if artifact.id != stem || id.to_string() != stem || artifact.uri != format!("artifact://{stem}")
    {
        return Err(ArtifactError::Integrity(artifact.uri.clone()));
    }
    Ok(())
}

fn ensure_regular_file(path: &Path, identity: &str) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(ArtifactError::Integrity(identity.to_string()));
    }
    Ok(metadata)
}

fn ensure_directory(path: &Path, identity: &str) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(ArtifactError::Integrity(identity.to_string()));
    }
    Ok(metadata)
}

fn ensure_managed_directory(root: &Path, path: &Path) -> Result<fs::Metadata> {
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

fn ensure_managed_file(root: &Path, path: &Path, identity: &str) -> Result<fs::Metadata> {
    let metadata = ensure_regular_file(path, identity)?;
    ensure_canonical_containment(root, path, identity)?;
    Ok(metadata)
}

fn ensure_canonical_containment(root: &Path, path: &Path, identity: &str) -> Result<()> {
    let canonical_root = fs::canonicalize(root)?;
    let canonical_path = fs::canonicalize(path)?;
    if canonical_path.starts_with(&canonical_root) {
        Ok(())
    } else {
        Err(ArtifactError::Integrity(identity.to_string()))
    }
}

static TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
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

fn read_bounded_file(path: &Path, limit: usize, identity: &str) -> Result<Vec<u8>> {
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

pub fn default_root() -> PathBuf {
    Path::new(".tempestmiku").to_path_buf()
}

pub fn preview(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let end = floor_boundary(s, cap);
    format!("{}\n… ({} bytes total)", &s[..end], s.len())
}

fn select_text_file(
    path: &Path,
    selector: Option<&str>,
    limits: ArtifactLimits,
) -> Result<(String, bool)> {
    ensure_regular_file(path, &path.display().to_string())?;
    let (start, end) = parse_selector(selector, limits)?;
    let max_page_bytes = if selector.is_some() {
        limits.max_page_bytes
    } else {
        limits.default_page_bytes
    };
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    for _ in 1..start {
        if reader.skip_until(b'\n')? == 0 {
            return Ok((String::new(), false));
        }
    }

    let mut selected = Vec::new();
    let mut truncated = false;
    for (selected_lines, _) in (start..=end).enumerate() {
        let separator = usize::from(selected_lines > 0);
        let remaining = max_page_bytes.saturating_sub(selected.len().saturating_add(separator));
        let Some((line, line_truncated)) = read_bounded_line(&mut reader, remaining)? else {
            break;
        };
        if separator == 1 {
            selected.push(b'\n');
        }
        selected.extend_from_slice(&line);
        if line_truncated {
            truncated = true;
            break;
        }
    }

    let has_more = truncated || !reader.fill_buf()?.is_empty();
    if let Err(err) = std::str::from_utf8(&selected) {
        selected.truncate(err.valid_up_to());
    }
    let selected = String::from_utf8(selected)
        .map_err(|_| ArtifactError::Integrity(path.display().to_string()))?;
    Ok((selected, has_more))
}

fn parse_selector(selector: Option<&str>, limits: ArtifactLimits) -> Result<(usize, usize)> {
    let Some(selector) = selector else {
        return Ok((1, limits.default_page_lines));
    };
    let (start, end) = selector
        .split_once('-')
        .ok_or_else(|| ArtifactError::InvalidSelector(selector.to_string()))?;
    let start: usize = start
        .parse()
        .map_err(|_| ArtifactError::InvalidSelector(selector.to_string()))?;
    let end: usize = end
        .parse()
        .map_err(|_| ArtifactError::InvalidSelector(selector.to_string()))?;
    let line_count = end
        .checked_sub(start)
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| ArtifactError::InvalidSelector(selector.to_string()))?;
    if start == 0 || end < start || line_count > limits.max_page_lines {
        return Err(ArtifactError::InvalidSelector(selector.to_string()));
    }
    Ok((start, end))
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    limit: usize,
) -> io::Result<Option<(Vec<u8>, bool)>> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some((line, false)))
            };
        }
        if let Some(newline) = buf.iter().position(|byte| *byte == b'\n') {
            let content = &buf[..newline];
            let available = limit.saturating_sub(line.len());
            let copied = content.len().min(available);
            line.extend_from_slice(&content[..copied]);
            if copied < content.len() {
                reader.consume(copied);
                return Ok(Some((line, true)));
            }
            reader.consume(newline + 1);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some((line, false)));
        }

        let available = limit.saturating_sub(line.len());
        let buf_len = buf.len();
        let copied = buf_len.min(available);
        line.extend_from_slice(&buf[..copied]);
        reader.consume(copied);
        if copied < buf_len || available == 0 {
            return Ok(Some((line, true)));
        }
    }
}

fn floor_boundary(s: &str, mut idx: usize) -> usize {
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn storage_identifiers_accept_only_canonical_forms() {
        assert_eq!(SessionId::parse("session-1").unwrap().as_str(), "session-1");
        for invalid in ["", ".", "..", "/absolute", "a/b", "a\\b", "bad id"] {
            assert!(SessionId::parse(invalid).is_err(), "accepted {invalid:?}");
        }

        assert_eq!(ArtifactId::parse_uri("artifact://42").unwrap().get(), 42);
        for invalid in [
            "artifact://",
            "artifact://01",
            "artifact://-1",
            "artifact://1/x",
        ] {
            assert!(
                ArtifactId::parse_uri(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }

        let hash = "a".repeat(64);
        assert_eq!(
            BlobId::parse_uri(&format!("blob:sha256:{hash}"))
                .unwrap()
                .hash(),
            hash
        );
        assert!(BlobId::parse_uri(&format!("blob:sha256:{}", "A".repeat(64))).is_err());
        assert!(BlobId::parse_uri(&format!("blob:sha256:{}", "a".repeat(63))).is_err());
    }

    #[test]
    fn stores_artifact_and_resolves_by_uri() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(dir.path(), "default").unwrap();
        let artifact = store
            .put_text("one\ntwo\nthree", Some("test".into()), "text/plain")
            .unwrap();

        assert_eq!(artifact.uri, "artifact://0");
        let content = store.read(&artifact.uri, Some("2-2")).unwrap();
        assert_eq!(content.content, "two");
        assert!(content.has_more);
    }

    #[test]
    fn blobs_are_content_addressed() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(dir.path(), "default").unwrap();
        let one = store.put_blob(b"same").unwrap();
        let two = store.put_blob(b"same").unwrap();
        assert_eq!(one, two);
        assert!(one.starts_with("blob:sha256:"));
        assert_eq!(store.read_blob(&one).unwrap(), b"same");
    }

    #[test]
    fn concurrent_store_instances_allocate_distinct_artifact_ids() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();

        for label in ["one", "two"] {
            let root = root.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let store = ArtifactStore::open(root, "default").unwrap();
                barrier.wait();
                store
                    .put_text(label, Some(label.to_string()), "text/plain")
                    .unwrap()
                    .uri
            }));
        }

        let mut uris = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        uris.sort();
        assert_eq!(uris, ["artifact://0", "artifact://1"]);
    }

    #[test]
    fn aggregate_session_quota_is_shared_across_store_handles() {
        let dir = tempfile::tempdir().unwrap();
        let limits = ArtifactLimits {
            max_artifact_bytes: 8,
            max_session_bytes: 10,
            ..ArtifactLimits::default()
        };
        let first = ArtifactStore::open_with_limits(dir.path(), "shared", limits).unwrap();
        let second = ArtifactStore::open_with_limits(dir.path(), "shared", limits).unwrap();

        first.put_text("12345678", None, "text/plain").unwrap();
        assert!(matches!(
            second.put_text("abcd", None, "text/plain"),
            Err(ArtifactError::QuotaExceeded {
                resource: "session",
                attempted: 12,
                limit: 10,
            })
        ));
    }

    #[test]
    fn aggregate_session_quota_counts_blob_references_once_per_session() {
        let dir = tempfile::tempdir().unwrap();
        let limits = ArtifactLimits {
            max_artifact_bytes: 8,
            max_blob_bytes: 8,
            max_session_bytes: 10,
            ..ArtifactLimits::default()
        };
        let first = ArtifactStore::open_with_limits(dir.path(), "shared", limits).unwrap();
        let second = ArtifactStore::open_with_limits(dir.path(), "shared", limits).unwrap();

        let uri = first.put_blob(b"12345678").unwrap();
        assert_eq!(second.put_blob(b"12345678").unwrap(), uri);
        second.put_text("12", None, "text/plain").unwrap();
        assert!(matches!(
            first.put_blob(b"abcd"),
            Err(ArtifactError::QuotaExceeded {
                resource: "session",
                attempted: 14,
                limit: 10,
            })
        ));

        let other = ArtifactStore::open_with_limits(dir.path(), "other", limits).unwrap();
        assert_eq!(other.put_blob(b"12345678").unwrap(), uri);
        other.put_text("12", None, "text/plain").unwrap();
    }

    #[test]
    fn rejects_unsafe_storage_identifiers() {
        let dir = tempfile::tempdir().unwrap();
        for session_id in ["", ".", "..", "../escape", "/tmp/escape", "nested/id"] {
            assert!(
                matches!(
                    ArtifactStore::open(dir.path(), session_id),
                    Err(ArtifactError::InvalidUri(_))
                ),
                "session id should be rejected: {session_id:?}"
            );
        }

        let store = ArtifactStore::open(dir.path(), "safe").unwrap();
        for uri in [
            "blob:sha256:/etc/passwd",
            "blob:sha256:../../outside",
            "blob:sha256:ABCDEF",
            "blob:sha256:abc",
        ] {
            assert!(matches!(
                store.read_blob(uri),
                Err(ArtifactError::InvalidUri(_))
            ));
        }
        assert!(matches!(
            store.read("artifact://../0", None),
            Err(ArtifactError::InvalidUri(_))
        ));
    }

    #[test]
    fn detects_tampered_blob_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(dir.path(), "default").unwrap();
        let uri = store.put_blob(b"trusted").unwrap();
        let hash = uri.strip_prefix("blob:sha256:").unwrap();
        fs::write(dir.path().join("blobs").join(hash), b"tampered").unwrap();

        assert!(matches!(
            store.read_blob(&uri),
            Err(ArtifactError::Integrity(_))
        ));
    }

    #[test]
    fn rejects_oversized_artifact_metadata_without_loading_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(dir.path(), "default").unwrap();
        let artifact = store.put_text("trusted", None, "text/plain").unwrap();
        drop(store);
        let metadata_path = dir
            .path()
            .join("sessions/default/artifacts")
            .join(format!("{}.meta", artifact.id));
        fs::write(metadata_path, vec![b'x'; MAX_ARTIFACT_METADATA_BYTES + 1]).unwrap();

        assert!(matches!(
            ArtifactStore::open(dir.path(), "default"),
            Err(ArtifactError::Integrity(_))
        ));
    }

    #[test]
    fn rejects_tampered_blob_quota_references() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(dir.path(), "default").unwrap();
        let uri = store.put_blob(b"trusted").unwrap();
        let hash = uri.strip_prefix("blob:sha256:").unwrap();
        drop(store);
        fs::write(
            dir.path()
                .join("sessions/default/blob_refs")
                .join(format!("{hash}.ref")),
            b"1",
        )
        .unwrap();

        assert!(matches!(
            ArtifactStore::open(dir.path(), "default"),
            Err(ArtifactError::Integrity(_))
        ));
    }

    #[test]
    fn rejects_oversized_artifact_metadata_before_writing_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(dir.path(), "default").unwrap();

        assert!(matches!(
            store.put_text(
                "small content",
                Some("x".repeat(MAX_ARTIFACT_METADATA_BYTES)),
                "text/plain",
            ),
            Err(ArtifactError::QuotaExceeded {
                resource: "artifact metadata",
                ..
            })
        ));
        assert!(store.list().is_empty());
        assert!(
            ArtifactStore::open(dir.path(), "default")
                .unwrap()
                .list()
                .is_empty()
        );
    }

    #[test]
    fn missing_artifact_returns_without_relocking() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(dir.path(), "default").unwrap();
        store.put_text("present", None, "text/plain").unwrap();

        let err = store.read("artifact://999", None).unwrap_err();
        assert!(matches!(err, ArtifactError::NotFound { .. }));
        assert!(err.to_string().contains("artifact://0"));
    }

    #[test]
    fn enforces_item_session_and_page_limits() {
        let dir = tempfile::tempdir().unwrap();
        let limits = ArtifactLimits {
            max_artifact_bytes: 16,
            max_blob_bytes: 32,
            max_session_bytes: 24,
            default_page_lines: 2,
            default_page_bytes: 8,
            max_page_lines: 3,
            max_page_bytes: 8,
        };
        let store = ArtifactStore::open_with_limits(dir.path(), "default", limits).unwrap();
        let artifact = store
            .put_text("one\ntwo\nthree", None, "text/plain")
            .unwrap();

        let first = store.read(&artifact.uri, None).unwrap();
        assert_eq!(first.content, "one\ntwo");
        assert!(first.has_more);
        let third = store.read(&artifact.uri, Some("3-3")).unwrap();
        assert_eq!(third.content, "three");
        assert!(!third.has_more);
        assert!(matches!(
            store.read(&artifact.uri, Some("1-4")),
            Err(ArtifactError::InvalidSelector(_))
        ));
        assert!(matches!(
            store.put_text("x".repeat(17), None, "text/plain"),
            Err(ArtifactError::QuotaExceeded {
                resource: "artifact",
                ..
            })
        ));
        assert!(matches!(
            store.put_text("y".repeat(12), None, "text/plain"),
            Err(ArtifactError::QuotaExceeded {
                resource: "session",
                ..
            })
        ));
    }

    #[test]
    fn byte_limited_pages_end_on_valid_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open_with_limits(
            dir.path(),
            "default",
            ArtifactLimits {
                max_artifact_bytes: 64,
                max_blob_bytes: 64,
                max_session_bytes: 64,
                default_page_lines: 2,
                default_page_bytes: 3,
                max_page_lines: 2,
                max_page_bytes: 3,
            },
        )
        .unwrap();
        let artifact = store.put_text("ééé", None, "text/plain").unwrap();
        let page = store.read(&artifact.uri, None).unwrap();
        assert_eq!(page.content, "é");
        assert!(page.has_more);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_blob_and_artifact_content() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, "outside").unwrap();
        let store = ArtifactStore::open(dir.path(), "default").unwrap();

        let blob_uri = store.put_blob(b"blob").unwrap();
        let hash = blob_uri.strip_prefix("blob:sha256:").unwrap();
        let blob_path = dir.path().join("blobs").join(hash);
        fs::remove_file(&blob_path).unwrap();
        symlink(&outside, &blob_path).unwrap();
        assert!(matches!(
            store.read_blob(&blob_uri),
            Err(ArtifactError::Integrity(_))
        ));

        // Use a fresh store for the artifact case: once the blob is tampered, every
        // quota-sensitive write for that session must fail closed on the corrupt ref.
        let artifact_dir = tempfile::tempdir().unwrap();
        let artifact_store = ArtifactStore::open(artifact_dir.path(), "default").unwrap();
        let artifact = artifact_store
            .put_text("inside", None, "text/plain")
            .unwrap();
        let artifact_path = artifact_dir
            .path()
            .join("sessions/default/artifacts")
            .join(format!("{}.txt", artifact.id));
        fs::remove_file(&artifact_path).unwrap();
        symlink(&outside, &artifact_path).unwrap();
        assert!(matches!(
            artifact_store.read(&artifact.uri, None),
            Err(ArtifactError::Integrity(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_managed_storage_directories() {
        use std::os::unix::fs::symlink;

        let blob_root = tempfile::tempdir().unwrap();
        let blob_outside = tempfile::tempdir().unwrap();
        let blob_store = ArtifactStore::open(blob_root.path(), "default").unwrap();
        fs::remove_dir(blob_root.path().join("blobs")).unwrap();
        symlink(blob_outside.path(), blob_root.path().join("blobs")).unwrap();
        assert!(matches!(
            blob_store.put_blob(b"must stay inside"),
            Err(ArtifactError::Integrity(_))
        ));

        let artifact_root = tempfile::tempdir().unwrap();
        let artifact_outside = tempfile::tempdir().unwrap();
        let artifact_store = ArtifactStore::open(artifact_root.path(), "default").unwrap();
        let artifact_dir = artifact_root.path().join("sessions/default/artifacts");
        fs::remove_dir(&artifact_dir).unwrap();
        symlink(artifact_outside.path(), &artifact_dir).unwrap();
        assert!(matches!(
            artifact_store.put_text("must stay inside", None, "text/plain"),
            Err(ArtifactError::Integrity(_))
        ));
    }
}

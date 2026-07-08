//! Repo-local artifact and blob storage.
//!
//! Artifacts are session-local monotonic `artifact://<id>` handles backed by files.
//! Blobs are global content-addressed `blob:sha256:<hash>` files for deduplication.

use std::{
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub type Result<T, E = ArtifactError> = std::result::Result<T, E>;

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
    session_id: String,
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    next_id: u64,
    refs: Vec<ArtifactRef>,
}

impl ArtifactStore {
    pub fn open(root: impl Into<PathBuf>, session_id: impl AsRef<str>) -> Result<Self> {
        let root = root.into();
        let session_dir = root
            .join("sessions")
            .join(session_id.as_ref())
            .join("artifacts");
        let blob_dir = root.join("blobs");
        fs::create_dir_all(&session_dir)?;
        fs::create_dir_all(&blob_dir)?;

        let mut refs = Vec::new();
        let mut max_id = None;
        for entry in fs::read_dir(&session_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("meta") {
                continue;
            }
            let meta = fs::read_to_string(&path)?;
            let artifact: ArtifactRef = serde_json::from_str(&meta).map_err(io::Error::other)?;
            if let Ok(id) = artifact.id.parse::<u64>() {
                max_id = Some(max_id.map_or(id, |m: u64| m.max(id)));
            }
            refs.push(artifact);
        }
        refs.sort_by_key(|r| r.id.parse::<u64>().unwrap_or(u64::MAX));
        let next_id = max_id.map_or(0, |id| id + 1);

        Ok(Self {
            root,
            session_id: session_id.as_ref().to_string(),
            inner: Arc::new(Mutex::new(Inner { next_id, refs })),
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
        let dir = self.session_artifact_dir();
        fs::create_dir_all(&dir)?;
        let (id, artifact) = {
            let mut inner = self.inner.lock();
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
                        break (id, artifact);
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                        id += 1;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        };

        let write_result = (|| -> Result<()> {
            fs::write(dir.join(format!("{id}.txt")), bytes)?;
            fs::write(
                dir.join(format!("{id}.meta")),
                serde_json::to_vec_pretty(&artifact).map_err(io::Error::other)?,
            )?;
            Ok(())
        })();
        write_result?;
        self.inner.lock().refs.push(artifact.clone());
        Ok(artifact)
    }

    pub fn put_blob(&self, bytes: &[u8]) -> Result<String> {
        let hash = hex::encode(Sha256::digest(bytes));
        let uri = format!("blob:sha256:{hash}");
        let path = self.root.join("blobs").join(&hash);
        if !path.exists() {
            fs::write(path, bytes)?;
        }
        Ok(uri)
    }

    pub fn read_blob(&self, uri: &str) -> Result<Vec<u8>> {
        let hash = uri
            .strip_prefix("blob:sha256:")
            .ok_or_else(|| ArtifactError::InvalidUri(uri.to_string()))?;
        let path = self.root.join("blobs").join(hash);
        fs::read(&path).map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                ArtifactError::NotFound {
                    uri: uri.to_string(),
                    available: Vec::new(),
                }
            } else {
                ArtifactError::Io(err)
            }
        })
    }

    pub fn read(&self, uri: &str, selector: Option<&str>) -> Result<ResourceContent> {
        let id = uri
            .strip_prefix("artifact://")
            .ok_or_else(|| ArtifactError::InvalidUri(uri.to_string()))?;
        let artifact = self
            .inner
            .lock()
            .refs
            .iter()
            .find(|artifact| artifact.id == id)
            .cloned()
            .ok_or_else(|| ArtifactError::NotFound {
                uri: uri.to_string(),
                available: self.list().into_iter().map(|a| a.uri).collect(),
            })?;
        let content = fs::read_to_string(self.session_artifact_dir().join(format!("{id}.txt")))?;
        let (selected, has_more) = select_text(&content, selector)?;
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

    fn session_artifact_dir(&self) -> PathBuf {
        self.root
            .join("sessions")
            .join(&self.session_id)
            .join("artifacts")
    }
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

fn select_text(content: &str, selector: Option<&str>) -> Result<(String, bool)> {
    let Some(selector) = selector else {
        return Ok((content.to_string(), false));
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
    if start == 0 || end < start {
        return Err(ArtifactError::InvalidSelector(selector.to_string()));
    }
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let selected = lines
        .iter()
        .skip(start - 1)
        .take(end - start + 1)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Ok((selected, end < total))
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
}

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
const MAX_ARTIFACT_TITLE_BYTES: usize = 1024;
const MAX_ARTIFACT_MIME_BYTES: usize = 256;

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
    #[error("artifact quota exceeded for {resource}: attempted {attempted}, limit {limit}")]
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
    pub max_artifact_count: usize,
    pub max_session_metadata_bytes: usize,
    pub max_blob_count: usize,
    pub max_session_blob_ref_bytes: usize,
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
            max_artifact_count: 4_096,
            max_session_metadata_bytes: 16 * 1024 * 1024,
            max_blob_count: 4_096,
            max_session_blob_ref_bytes: 256 * 1024,
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

mod filesystem;
mod preview;
mod quota;
mod store;

use filesystem::*;
use preview::select_text_file;
pub use preview::{default_root, preview};
use quota::*;

#[cfg(test)]
mod tests;

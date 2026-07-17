use super::*;
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

        let usage = artifact_namespace_usage(&session_dir, limits)?;
        let next_id = usage
            .max_id
            .map(|id| {
                id.checked_add(1)
                    .ok_or_else(|| ArtifactError::Integrity(format!("artifact://{id}")))
            })
            .transpose()?
            .unwrap_or(0);
        let blob_usage = blob_reference_usage(&blob_refs_dir, &blob_dir, limits)?;
        let total_bytes = usage
            .content_bytes
            .checked_add(blob_usage.content_bytes)
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
                refs: usage.refs,
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
        if let Some(title) = title.as_deref() {
            self.check_item_quota(title.len(), "artifact title", MAX_ARTIFACT_TITLE_BYTES)?;
        }
        self.check_item_quota(mime.len(), "artifact MIME", MAX_ARTIFACT_MIME_BYTES)?;
        let dir = self.validated_session_artifact_dir()?;
        let blob_refs_dir = self.validated_session_blob_ref_dir()?;
        let blob_dir = self.validated_blob_dir()?;
        let _write_guard = self.session_write_lock.lock();
        let (id, artifact, lock_path, prior_metadata_bytes) = {
            let mut inner = self.inner.lock();
            let usage = artifact_namespace_usage(&dir, self.limits)?;
            if usage.count >= self.limits.max_artifact_count {
                return Err(ArtifactError::QuotaExceeded {
                    resource: "artifact count",
                    attempted: usage.count.saturating_add(1),
                    limit: self.limits.max_artifact_count,
                });
            }
            let current_bytes = usage
                .content_bytes
                .checked_add(
                    blob_reference_usage(&blob_refs_dir, &blob_dir, self.limits)?.content_bytes,
                )
                .ok_or(ArtifactError::QuotaExceeded {
                    resource: "session",
                    attempted: usize::MAX,
                    limit: self.limits.max_session_bytes,
                })?;
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
            let mut id = inner.next_id.max(
                usage
                    .max_id
                    .map(|id| {
                        id.checked_add(1)
                            .ok_or_else(|| ArtifactError::Integrity(format!("artifact://{id}")))
                    })
                    .transpose()?
                    .unwrap_or(0),
            );
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
                            id = id.checked_add(1).ok_or_else(|| {
                                ArtifactError::Integrity(format!("artifact://{id}"))
                            })?;
                            continue;
                        }
                        inner.next_id = id
                            .checked_add(1)
                            .ok_or_else(|| ArtifactError::Integrity(format!("artifact://{id}")))?;
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
                        break (id, artifact, lock_path, usage.metadata_bytes);
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                        id = id
                            .checked_add(1)
                            .ok_or_else(|| ArtifactError::Integrity(format!("artifact://{id}")))?;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        };

        let metadata = serde_json::to_vec_pretty(&artifact).map_err(io::Error::other)?;
        self.check_item_quota(
            metadata.len(),
            "artifact metadata",
            MAX_ARTIFACT_METADATA_BYTES,
        )?;
        let attempted_metadata = prior_metadata_bytes.checked_add(metadata.len()).ok_or(
            ArtifactError::QuotaExceeded {
                resource: "session artifact metadata",
                attempted: usize::MAX,
                limit: self.limits.max_session_metadata_bytes,
            },
        )?;
        if attempted_metadata > self.limits.max_session_metadata_bytes {
            let _ = fs::remove_file(&lock_path);
            return Err(ArtifactError::QuotaExceeded {
                resource: "session artifact metadata",
                attempted: attempted_metadata,
                limit: self.limits.max_session_metadata_bytes,
            });
        }

        let write_result = (|| -> Result<()> {
            write_atomic(&dir.join(format!("{id}.txt")), bytes)?;
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
                let artifact_usage = artifact_namespace_usage(&artifact_dir, self.limits)?;
                let blob_usage = blob_reference_usage(&blob_refs_dir, &blob_dir, self.limits)?;
                if blob_usage.count >= self.limits.max_blob_count {
                    return Err(ArtifactError::QuotaExceeded {
                        resource: "blob reference count",
                        attempted: blob_usage.count.saturating_add(1),
                        limit: self.limits.max_blob_count,
                    });
                }
                let reference_bytes = bytes.len().to_string().len();
                let attempted_reference_bytes = blob_usage
                    .metadata_bytes
                    .checked_add(reference_bytes)
                    .ok_or(ArtifactError::QuotaExceeded {
                        resource: "session blob-reference metadata",
                        attempted: usize::MAX,
                        limit: self.limits.max_session_blob_ref_bytes,
                    })?;
                if attempted_reference_bytes > self.limits.max_session_blob_ref_bytes {
                    return Err(ArtifactError::QuotaExceeded {
                        resource: "session blob-reference metadata",
                        attempted: attempted_reference_bytes,
                        limit: self.limits.max_session_blob_ref_bytes,
                    });
                }
                let current_bytes = artifact_usage
                    .content_bytes
                    .checked_add(blob_usage.content_bytes)
                    .ok_or(ArtifactError::QuotaExceeded {
                        resource: "session",
                        attempted: usize::MAX,
                        limit: self.limits.max_session_bytes,
                    })?;
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

    /// Return the in-memory bounded namespace snapshot loaded by this handle.
    ///
    /// Model-visible and async callers should prefer [`Self::list_page`], which
    /// refreshes from disk and never clones the whole namespace.
    pub fn list(&self) -> Vec<ArtifactRef> {
        self.inner.lock().refs.clone()
    }

    /// Refresh the artifact namespace and return one bounded page plus a continuation flag.
    pub fn list_page(&self, offset: usize, limit: usize) -> Result<(Vec<ArtifactRef>, bool)> {
        if limit == 0 || limit > self.limits.max_artifact_count {
            return Err(ArtifactError::InvalidSelector(format!(
                "artifact list limit must be in 1..={}",
                self.limits.max_artifact_count
            )));
        }
        let dir = self.validated_session_artifact_dir()?;
        let _write_guard = self.session_write_lock.lock();
        let usage = artifact_namespace_usage(&dir, self.limits)?;
        let end = offset.saturating_add(limit).min(usage.refs.len());
        let items = usage.refs.get(offset..end).unwrap_or_default().to_vec();
        Ok((items, end < usage.refs.len()))
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

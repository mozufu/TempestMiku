use super::*;

impl DriveService<InMemoryDriveMetadataStore> {
    pub fn put_bytes(
        &self,
        bytes: &[u8],
        options: DrivePutOptions,
    ) -> crate::Result<DrivePutResult> {
        let options = sanitize_drive_put_options(options)?;
        let bytes = sanitize_drive_bytes(bytes)?;
        let bytes = bytes.as_ref();
        let plan = self.plan_put_bytes(bytes, &options)?;
        self.commit_put_bytes(bytes, options, plan)
    }

    pub(crate) fn plan_put_bytes(
        &self,
        bytes: &[u8],
        options: &DrivePutOptions,
    ) -> crate::Result<DrivePutPlan> {
        if bytes.len() > 5 * 1024 * 1024 {
            return Err(DriveError::InvalidArgs(
                "inline drive.put content is capped at 5 MiB in P5 v1".to_string(),
            ));
        }
        let filename_hint = filename_hint(options);
        let transduction = transduce_document(TransducerInput {
            bytes,
            filename: filename_hint.as_deref(),
            options,
        })?;
        let proposed_path = propose_path(&transduction, options, filename_hint.as_deref());
        let path = normalize_canonical_path(&proposed_path)?;
        Ok(DrivePutPlan {
            transduction,
            proposed_path,
            path,
            now: Utc::now(),
        })
    }

    pub(crate) fn commit_put_bytes(
        &self,
        bytes: &[u8],
        options: DrivePutOptions,
        plan: DrivePutPlan,
    ) -> crate::Result<DrivePutResult> {
        let DrivePutPlan {
            transduction,
            proposed_path,
            mut path,
            now,
        } = plan;
        let blob_uri = self
            .artifacts
            .put_blob(bytes)
            .map_err(|err| DriveError::Store(err.to_string()))?;
        let mut inner = self.metadata.inner.lock();

        if let Some(existing_id) = inner.path_to_id.get(&path).copied() {
            let existing = inner.entries.get(&existing_id).cloned().ok_or_else(|| {
                DriveError::Store(format!("path index points to missing entry {existing_id}"))
            })?;
            if existing.content_hash == transduction.content_hash
                && options.dedupe == DriveDedupeMode::ContentHash
            {
                return Ok(DrivePutResult {
                    uri: existing.uri.clone(),
                    proposed_path,
                    filed: true,
                    proposal: None,
                    entry: existing,
                });
            }
            if options.overwrite || options.collision == DriveCollisionStrategy::Overwrite {
                let mut entry = existing;
                entry.blob_uri = blob_uri;
                entry.content_hash = transduction.content_hash.clone();
                entry.mime = transduction.mime.clone();
                entry.size_bytes = bytes.len();
                entry.title = transduction.title.clone();
                entry.doc_kind = transduction.doc_kind.clone();
                entry.project = transduction.project.clone();
                entry.entities = transduction.entities.clone();
                entry.dates = transduction.dates.clone();
                entry.amounts = transduction.amounts.clone();
                entry.tags = transduction.tags.clone();
                entry.source_uri = options.source_uri.clone();
                entry.attributes = transduction.attributes.clone();
                entry.summary = transduction.summary.clone();
                entry.updated_at = now;
                bump_version(&mut entry.version);
                entry.provenance.push(provenance(
                    &options,
                    &transduction.content_hash,
                    &transduction.extractor,
                    now,
                ));
                inner.entries.insert(entry.id, entry.clone());
                return Ok(DrivePutResult {
                    uri: entry.uri.clone(),
                    proposed_path,
                    filed: true,
                    proposal: None,
                    entry,
                });
            }
            if options.collision == DriveCollisionStrategy::Reject {
                return Err(DriveError::Collision(path));
            }
            path = unique_path(&inner.path_to_id, &path);
        }

        let id = Uuid::new_v4();
        let uri = DriveEntry::drive_uri(&path);
        let entry = DriveEntry {
            id,
            version: initial_record_version(),
            path: path.clone(),
            uri: uri.clone(),
            blob_uri,
            content_hash: transduction.content_hash.clone(),
            mime: transduction.mime.clone(),
            size_bytes: bytes.len(),
            title: transduction.title.clone(),
            doc_kind: transduction.doc_kind.clone(),
            project: transduction.project.clone(),
            entities: transduction.entities.clone(),
            dates: transduction.dates.clone(),
            amounts: transduction.amounts.clone(),
            tags: transduction.tags.clone(),
            embedding: None,
            source_uri: options.source_uri.clone(),
            provenance: vec![provenance(
                &options,
                &transduction.content_hash,
                &transduction.extractor,
                now,
            )],
            created_at: now,
            updated_at: now,
            status: DriveEntryStatus::Active,
            attributes: transduction.attributes,
            summary: transduction.summary,
        };
        inner.path_to_id.insert(path, id);
        inner.entries.insert(id, entry.clone());

        let proposal = if options.auto && options.approval_mode == crate::DriveApprovalMode::Propose
        {
            Some(write_proposal(&entry, proposed_path.clone(), now))
        } else {
            None
        };
        if let Some(proposal) = proposal.clone() {
            inner.proposals.insert(proposal.id, proposal);
        }

        Ok(DrivePutResult {
            entry,
            uri,
            proposed_path,
            filed: true,
            proposal,
        })
    }

    pub fn move_entry(
        &self,
        from: &str,
        to: &str,
        collision: DriveCollisionStrategy,
    ) -> crate::Result<DriveEntry> {
        validate_drive_identifier("move source", from)?;
        validate_drive_identifier("move destination", to)?;
        let from = normalize_canonical_path_path_or_uri(from)?;
        let mut to = normalize_canonical_path_path_or_uri(to)?;
        let now = Utc::now();
        let mut inner = self.metadata.inner.lock();
        let id = *inner
            .path_to_id
            .get(&from)
            .ok_or_else(|| DriveError::NotFound(from.clone()))?;
        if inner.path_to_id.contains_key(&to) {
            match collision {
                DriveCollisionStrategy::Reject => return Err(DriveError::Collision(to)),
                DriveCollisionStrategy::Overwrite => {
                    let overwritten = inner.path_to_id.remove(&to).unwrap();
                    inner.entries.remove(&overwritten);
                }
                DriveCollisionStrategy::KeepBoth => {
                    to = unique_path(&inner.path_to_id, &to);
                }
            }
        }
        inner.path_to_id.remove(&from);
        inner.path_to_id.insert(to.clone(), id);
        let entry = inner
            .entries
            .get_mut(&id)
            .ok_or_else(|| DriveError::NotFound(from.clone()))?;
        entry.path = to.clone();
        entry.uri = DriveEntry::drive_uri(&to);
        entry.updated_at = now;
        bump_version(&mut entry.version);
        let updated = entry.clone();
        inner.corrections.push(DriveCorrectionRecord {
            id: Uuid::new_v4(),
            version: initial_record_version(),
            from,
            to,
            created_at: now,
        });
        Ok(updated)
    }

    pub fn tag_entry(&self, path_or_uri: &str, tags: Vec<String>) -> crate::Result<DriveEntry> {
        validate_drive_identifier("tag path", path_or_uri)?;
        for tag in &tags {
            validate_drive_identifier("tag", tag)?;
        }
        let path = normalize_canonical_path_path_or_uri(path_or_uri)?;
        let now = Utc::now();
        let mut inner = self.metadata.inner.lock();
        let id = *inner
            .path_to_id
            .get(&path)
            .ok_or_else(|| DriveError::NotFound(path.clone()))?;
        let entry = inner
            .entries
            .get_mut(&id)
            .ok_or_else(|| DriveError::NotFound(path.clone()))?;
        entry.tags = apply_tags(&entry.tags, &tags);
        entry.updated_at = now;
        bump_version(&mut entry.version);
        Ok(entry.clone())
    }
}

pub(crate) fn filename_hint(options: &DrivePutOptions) -> Option<String> {
    options
        .suggested_path
        .as_ref()
        .or(options.source_uri.as_ref())
        .and_then(|path| path.rsplit('/').next())
        .map(str::to_string)
}

pub(crate) fn provenance(
    options: &DrivePutOptions,
    content_hash: &str,
    extractor: &str,
    created_at: chrono::DateTime<Utc>,
) -> DriveProvenance {
    DriveProvenance {
        source_uri: options.source_uri.clone(),
        session_id: options.session_id.clone(),
        event_seq: options.event_seq,
        actor_id: None,
        source_run_id: None,
        content_hash: content_hash.to_string(),
        extractor: extractor.to_string(),
        created_at,
    }
}

pub(crate) fn write_proposal(
    entry: &DriveEntry,
    proposed_path: String,
    now: chrono::DateTime<Utc>,
) -> OrganizerProposal {
    OrganizerProposal {
        id: Uuid::new_v4(),
        version: initial_record_version(),
        action: OrganizerActionKind::Move,
        entry_id: entry.id,
        source_path: entry.path.clone(),
        proposed_path: Some(proposed_path),
        proposed_tags: Vec::new(),
        proposed_doc_kind: entry.doc_kind.clone(),
        proposed_project: entry.project.clone(),
        evidence: vec![DriveEvidence {
            snippet: entry.summary.clone().unwrap_or_else(|| entry.path.clone()),
            selector: Some("1-3".to_string()),
        }],
        confidence: 0.75,
        policy_decision: PolicyDecision::ApprovalRequired,
        approval_id: None,
        status: ProposalStatus::Pending,
        source_run_id: Uuid::new_v4(),
        replay_metadata: BTreeMap::new(),
        created_at: now,
        updated_at: now,
    }
}

pub(crate) fn unique_path(paths: &BTreeMap<String, DriveEntryId>, path: &str) -> String {
    let (stem, ext) = path
        .rsplit_once('.')
        .map_or((path, ""), |(stem, ext)| (stem, ext));
    for index in 2.. {
        let candidate = if ext.is_empty() {
            format!("{stem}-{index}")
        } else {
            format!("{stem}-{index}.{ext}")
        };
        if !paths.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!("infinite keep-both path search")
}

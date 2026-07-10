use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tm_artifacts::{ResourceContent, preview};
use tm_host::{HostError, ResourceEntry};
use uuid::Uuid;

use super::types::{DrivePutPlan, DriveRead, DriveService, InMemoryDriveMetadataStore, Inner};
use crate::types::DriveError;
use crate::{
    DriveAutomationTier, DriveCollisionStrategy, DriveCorrectionRecord, DriveDedupeMode,
    DriveEntry, DriveEntryId, DriveEntryStatus, DriveEvidence, DriveListOptions,
    DriveOrganizerConfig, DriveOrganizerRunId, DriveProvenance, DrivePutOptions, DrivePutResult,
    DriveSearchOptions, DriveSearchResult, OrganizerActionKind, OrganizerProposal, OrganizerRun,
    OrganizerRunStatus, PolicyDecision, ProposalStatus, TransducerInput, apply_tags,
    drive_uri_path, generate_organizer_proposals_for_run, initial_record_version,
    parse_virtual_dir, propose_path, transduce_document, vdir::virtual_query_to_search,
};

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

    pub fn get(&self, path_or_uri: &str) -> crate::Result<DriveEntry> {
        let path = normalize_canonical_path_path_or_uri(path_or_uri)?;
        let inner = self.metadata.inner.lock();
        let id = inner
            .path_to_id
            .get(&path)
            .ok_or_else(|| drive_not_found(&inner, path_or_uri, &path))?;
        inner
            .entries
            .get(id)
            .cloned()
            .ok_or_else(|| DriveError::NotFound(path_or_uri.to_string()))
    }

    pub fn read(&self, path_or_uri: &str) -> crate::Result<DriveRead> {
        let entry = self.get(path_or_uri)?;
        let bytes = self
            .artifacts
            .read_blob(&entry.blob_uri)
            .map_err(|err| DriveError::NotFound(err.to_string()))?;
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != entry.content_hash {
            return Err(DriveError::Integrity {
                path: entry.path,
                expected: entry.content_hash,
                actual,
            });
        }
        Ok(DriveRead { entry, bytes })
    }

    pub fn resource_content(
        &self,
        uri: &str,
        selector: Option<&str>,
    ) -> crate::Result<ResourceContent> {
        let path = drive_uri_path(uri)?;
        let read = self.read(&path)?;
        let title = read.entry.title.clone();
        if read.entry.mime.starts_with("text/")
            || read.entry.mime == "application/json"
            || read.entry.mime == "text/markdown"
        {
            let text = String::from_utf8(read.bytes)
                .map_err(|_| DriveError::InvalidArgs("drive text resource is not UTF-8".into()))?;
            let (selected, has_more) = select_text(&text, selector)?;
            Ok(ResourceContent {
                uri: read.entry.uri,
                kind: "drive_document".to_string(),
                mime: read.entry.mime,
                title,
                size_bytes: read.entry.size_bytes,
                selector: selector.map(str::to_string),
                has_more,
                preview: preview(&selected, 1024),
                content: selected,
            })
        } else {
            Ok(ResourceContent {
                uri: read.entry.uri,
                kind: "drive_binary".to_string(),
                mime: read.entry.mime,
                title,
                size_bytes: read.entry.size_bytes,
                selector: None,
                has_more: false,
                preview: format!(
                    "binary drive resource: {} ({} bytes, {})",
                    read.entry.path, read.entry.size_bytes, read.entry.content_hash
                ),
                content: String::new(),
            })
        }
    }

    pub fn list(&self, options: DriveListOptions) -> crate::Result<Vec<DriveEntry>> {
        let limit = options.limit.max(1);
        if let Some(path) = options.path.as_deref()
            && let Some(query) = parse_virtual_dir(path)
        {
            return Ok(self
                .search(virtual_query_to_search(&query, limit))?
                .into_iter()
                .filter_map(|result| self.get(&result.uri).ok())
                .collect());
        }

        let prefix = options
            .path
            .as_deref()
            .map(normalize_optional_prefix)
            .transpose()?
            .unwrap_or_default();
        let inner = self.metadata.inner.lock();
        let mut entries = inner
            .entries
            .values()
            .filter(|entry| options.include_archived || entry.status == DriveEntryStatus::Active)
            .filter(|entry| {
                prefix.is_empty()
                    || entry.path == prefix
                    || entry.path.starts_with(&format!("{prefix}/"))
            })
            .filter(|entry| {
                if options.recursive || prefix.is_empty() {
                    return true;
                }
                let rest = entry
                    .path
                    .strip_prefix(&prefix)
                    .unwrap_or(&entry.path)
                    .trim_start_matches('/');
                !rest.contains('/')
            })
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.path.cmp(&b.path))
        });
        entries.truncate(limit);
        Ok(entries)
    }

    pub fn search(&self, options: DriveSearchOptions) -> crate::Result<Vec<DriveSearchResult>> {
        let query = options.query.as_ref().map(|q| q.to_ascii_lowercase());
        let query_terms = query
            .as_deref()
            .map(|q| q.split_whitespace().map(str::to_string).collect::<Vec<_>>())
            .unwrap_or_default();
        let tag_filter = options
            .tags
            .iter()
            .map(|tag| tag.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let inner = self.metadata.inner.lock();
        let mut results = Vec::new();
        for entry in inner.entries.values() {
            if !options.include_archived && entry.status != DriveEntryStatus::Active {
                continue;
            }
            if options.project.as_ref().is_some_and(|project| {
                entry
                    .project
                    .as_ref()
                    .map(|value| value.to_ascii_lowercase())
                    != Some(project.to_ascii_lowercase())
            }) {
                continue;
            }
            if options.doc_kind.as_ref().is_some_and(|kind| {
                entry
                    .doc_kind
                    .as_ref()
                    .map(|value| value.to_ascii_lowercase())
                    != Some(kind.to_ascii_lowercase())
            }) {
                continue;
            }
            if !tag_filter.is_empty() {
                let tags = entry
                    .tags
                    .iter()
                    .map(|tag| tag.to_ascii_lowercase())
                    .collect::<BTreeSet<_>>();
                if !tag_filter.is_subset(&tags) {
                    continue;
                }
            }
            if let Some(since) = options.since
                && entry.updated_at < since
            {
                continue;
            }
            if let Some(until) = options.until
                && entry.updated_at > until
            {
                continue;
            }
            let mut score = recency_score(entry);
            if !query_terms.is_empty() {
                let lexical = lexical_score(entry, &query_terms);
                if lexical <= 0.01 {
                    continue;
                }
                score += lexical;
            }
            results.push(DriveSearchResult {
                uri: entry.uri.clone(),
                path: entry.path.clone(),
                title: entry.title.clone(),
                doc_kind: entry.doc_kind.clone(),
                project: entry.project.clone(),
                tags: entry.tags.clone(),
                content_hash: entry.content_hash.clone(),
                score,
                snippet: options
                    .return_snippets
                    .then(|| snippet_for(entry, query.as_deref())),
                selector: options.return_snippets.then(|| "1-3".to_string()),
            });
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });
        results.truncate(options.limit.max(1));
        Ok(results)
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

    pub fn organize(&self) -> crate::Result<Vec<OrganizerProposal>> {
        self.organize_with_config(DriveOrganizerConfig::default())
    }

    pub fn organize_with_config(
        &self,
        config: DriveOrganizerConfig,
    ) -> crate::Result<Vec<OrganizerProposal>> {
        let now = Utc::now();
        self.enqueue_organizer_run("manual", now);
        let run = self
            .claim_ready_organizer_run(now, Duration::seconds(30))?
            .ok_or_else(|| DriveError::Store("organizer worker already running".to_string()))?;
        let mut proposals = self.generate_organizer_proposals_for_run(run.id)?;
        let auto_apply_ids = self.mark_auto_apply_proposals(&proposals, &config);
        if !auto_apply_ids.is_empty() {
            let applied = self.apply_organizer_proposals(&auto_apply_ids)?;
            for updated in applied {
                if let Some(proposal) = proposals
                    .iter_mut()
                    .find(|proposal| proposal.id == updated.id)
                {
                    *proposal = updated;
                }
            }
        }
        let proposal_ids = proposals
            .iter()
            .map(|proposal| proposal.id)
            .collect::<Vec<_>>();
        self.complete_organizer_run(run.id, proposal_ids, Utc::now())?;
        Ok(proposals)
    }

    pub fn enqueue_organizer_run(
        &self,
        trigger: impl Into<String>,
        now: DateTime<Utc>,
    ) -> OrganizerRun {
        let mut inner = self.metadata.inner.lock();
        if let Some(existing) = inner
            .organizer_runs
            .values()
            .find(|run| {
                matches!(
                    run.status,
                    OrganizerRunStatus::Queued | OrganizerRunStatus::Running
                )
            })
            .cloned()
        {
            return existing;
        }
        let run = OrganizerRun {
            id: Uuid::new_v4(),
            version: initial_record_version(),
            trigger: trigger.into(),
            status: OrganizerRunStatus::Queued,
            attempts: 0,
            proposal_ids: Vec::new(),
            created_at: now,
            available_at: now,
            locked_at: None,
            completed_at: None,
            last_error: None,
        };
        inner.organizer_runs.insert(run.id, run.clone());
        run
    }

    pub fn claim_ready_organizer_run(
        &self,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> crate::Result<Option<OrganizerRun>> {
        let stale_before = now - lease_timeout;
        let mut inner = self.metadata.inner.lock();
        let Some(run_id) = inner
            .organizer_runs
            .values()
            .filter(|run| {
                (run.status == OrganizerRunStatus::Queued && run.available_at <= now)
                    || (run.status == OrganizerRunStatus::Running
                        && run
                            .locked_at
                            .is_some_and(|locked_at| locked_at <= stale_before))
            })
            .min_by(|a, b| {
                a.available_at
                    .cmp(&b.available_at)
                    .then_with(|| a.created_at.cmp(&b.created_at))
                    .then_with(|| a.id.cmp(&b.id))
            })
            .map(|run| run.id)
        else {
            return Ok(None);
        };
        let run = inner
            .organizer_runs
            .get_mut(&run_id)
            .ok_or_else(|| DriveError::NotFound(format!("organizer run {run_id}")))?;
        run.status = OrganizerRunStatus::Running;
        run.attempts += 1;
        bump_version(&mut run.version);
        run.locked_at = Some(now);
        run.completed_at = None;
        run.last_error = None;
        Ok(Some(run.clone()))
    }

    pub fn heartbeat_organizer_run(
        &self,
        run_id: DriveOrganizerRunId,
        now: DateTime<Utc>,
    ) -> crate::Result<OrganizerRun> {
        let mut inner = self.metadata.inner.lock();
        let run = running_organizer_run_mut(&mut inner, run_id)?;
        run.locked_at = Some(now);
        bump_version(&mut run.version);
        Ok(run.clone())
    }

    pub fn complete_organizer_run(
        &self,
        run_id: DriveOrganizerRunId,
        proposal_ids: Vec<Uuid>,
        now: DateTime<Utc>,
    ) -> crate::Result<OrganizerRun> {
        let mut inner = self.metadata.inner.lock();
        let run = running_organizer_run_mut(&mut inner, run_id)?;
        run.status = OrganizerRunStatus::Completed;
        bump_version(&mut run.version);
        run.proposal_ids = proposal_ids;
        run.available_at = now;
        run.locked_at = None;
        run.completed_at = Some(now);
        run.last_error = None;
        Ok(run.clone())
    }

    pub fn fail_organizer_run(
        &self,
        run_id: DriveOrganizerRunId,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: u32,
    ) -> crate::Result<OrganizerRun> {
        let error = tm_memory::redact_dream_text(&error).text;
        let mut inner = self.metadata.inner.lock();
        let run = running_organizer_run_mut(&mut inner, run_id)?;
        run.status = if run.attempts >= max_attempts {
            OrganizerRunStatus::Failed
        } else {
            OrganizerRunStatus::Queued
        };
        bump_version(&mut run.version);
        run.available_at = next_available_at;
        run.locked_at = None;
        run.completed_at = None;
        run.last_error = Some(error);
        Ok(run.clone())
    }

    pub fn organizer_runs(&self) -> Vec<OrganizerRun> {
        self.metadata
            .inner
            .lock()
            .organizer_runs
            .values()
            .cloned()
            .collect()
    }

    fn generate_organizer_proposals_for_run(
        &self,
        run_id: DriveOrganizerRunId,
    ) -> crate::Result<Vec<OrganizerProposal>> {
        let mut inner = self.metadata.inner.lock();
        if !inner
            .organizer_runs
            .get(&run_id)
            .is_some_and(|run| run.status == OrganizerRunStatus::Running)
        {
            return Err(DriveError::NotFound(format!(
                "running organizer run {run_id}"
            )));
        }
        let entries = inner.entries.values().cloned().collect::<Vec<_>>();
        let proposals = generate_organizer_proposals_for_run(&entries, run_id);
        for proposal in &proposals {
            inner.proposals.insert(proposal.id, proposal.clone());
        }
        Ok(proposals)
    }

    fn mark_auto_apply_proposals(
        &self,
        proposals: &[OrganizerProposal],
        config: &DriveOrganizerConfig,
    ) -> Vec<Uuid> {
        let mut inner = self.metadata.inner.lock();
        let mut ids = Vec::new();
        for proposal in proposals {
            if !organizer_auto_apply_allowed(proposal, config) {
                continue;
            }
            if let Some(stored) = inner.proposals.get_mut(&proposal.id) {
                stored.policy_decision = PolicyDecision::AutoApply;
                bump_version(&mut stored.version);
                ids.push(stored.id);
            }
        }
        ids
    }

    pub fn pending_proposal_ids(&self) -> Vec<Uuid> {
        self.metadata
            .inner
            .lock()
            .proposals
            .values()
            .filter(|proposal| {
                matches!(
                    proposal.status,
                    ProposalStatus::Pending | ProposalStatus::Approved
                )
            })
            .map(|proposal| proposal.id)
            .collect()
    }

    pub fn mark_proposals_status(&self, ids: &[Uuid], status: ProposalStatus) {
        let now = Utc::now();
        let mut inner = self.metadata.inner.lock();
        for id in ids {
            if let Some(proposal) = inner.proposals.get_mut(id) {
                proposal.status = status.clone();
                proposal.updated_at = now;
                bump_version(&mut proposal.version);
            }
        }
    }

    pub fn apply_organizer_proposals(&self, ids: &[Uuid]) -> crate::Result<Vec<OrganizerProposal>> {
        let now = Utc::now();
        let mut inner = self.metadata.inner.lock();
        let mut applied = Vec::new();
        for id in ids {
            let Some(mut proposal) = inner.proposals.get(id).cloned() else {
                continue;
            };
            if !matches!(
                proposal.status,
                ProposalStatus::Pending | ProposalStatus::Approved
            ) {
                applied.push(proposal);
                continue;
            }
            proposal.status = apply_organizer_proposal(&mut inner, &proposal, now);
            proposal.updated_at = now;
            bump_version(&mut proposal.version);
            inner.proposals.insert(*id, proposal.clone());
            applied.push(proposal);
        }
        Ok(applied)
    }

    pub fn proposals(&self) -> Vec<OrganizerProposal> {
        self.metadata
            .inner
            .lock()
            .proposals
            .values()
            .cloned()
            .collect()
    }

    pub fn resource_entries(&self, uri: Option<&str>) -> crate::Result<Vec<ResourceEntry>> {
        let path = uri.map(|uri| uri.trim_start_matches("drive://").to_string());
        let entries = self.list(DriveListOptions {
            path,
            recursive: true,
            limit: 1000,
            include_archived: false,
        })?;
        Ok(entries
            .into_iter()
            .map(|entry| ResourceEntry {
                uri: entry.uri,
                name: entry
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&entry.path)
                    .to_string(),
                kind: entry
                    .doc_kind
                    .unwrap_or_else(|| "drive_document".to_string()),
                title: entry.title,
                size_bytes: Some(entry.size_bytes),
                modified_at: Some(entry.updated_at.to_rfc3339()),
            })
            .collect())
    }

    pub fn correction_signals(&self) -> Vec<(String, String, chrono::DateTime<Utc>)> {
        self.metadata
            .inner
            .lock()
            .corrections
            .iter()
            .map(|correction| {
                (
                    correction.from.clone(),
                    correction.to.clone(),
                    correction.created_at,
                )
            })
            .collect()
    }
}

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

pub(crate) fn running_organizer_run_mut(
    inner: &mut Inner,
    run_id: DriveOrganizerRunId,
) -> crate::Result<&mut OrganizerRun> {
    let run = inner
        .organizer_runs
        .get_mut(&run_id)
        .ok_or_else(|| DriveError::NotFound(format!("organizer run {run_id}")))?;
    if run.status != OrganizerRunStatus::Running {
        return Err(DriveError::NotFound(format!(
            "running organizer run {run_id}"
        )));
    }
    Ok(run)
}

pub(crate) fn organizer_auto_apply_allowed(
    proposal: &OrganizerProposal,
    config: &DriveOrganizerConfig,
) -> bool {
    if matches!(config.tier, DriveAutomationTier::Conservative) {
        return false;
    }
    config.auto_apply.iter().any(|rule| {
        !rule.actions.is_empty()
            && rule.actions.iter().any(|action| action == &proposal.action)
            && proposal.confidence >= rule.min_confidence
            && optional_class_match(&rule.doc_kinds, proposal.proposed_doc_kind.as_deref())
            && optional_class_match(&rule.projects, proposal.proposed_project.as_deref())
    })
}

pub(crate) fn optional_class_match(allowed: &[String], value: Option<&str>) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let Some(value) = value else {
        return false;
    };
    let value = value.trim();
    allowed
        .iter()
        .any(|allowed| allowed.trim().eq_ignore_ascii_case(value))
}

pub(crate) fn apply_organizer_proposal(
    inner: &mut Inner,
    proposal: &OrganizerProposal,
    now: chrono::DateTime<Utc>,
) -> ProposalStatus {
    let Some(entry) = inner.entries.get(&proposal.entry_id).cloned() else {
        return ProposalStatus::Stale;
    };
    if entry.path != proposal.source_path {
        return ProposalStatus::Stale;
    }
    if proposal
        .replay_metadata
        .get("contentHash")
        .and_then(Value::as_str)
        .is_some_and(|expected| expected != entry.content_hash)
    {
        return ProposalStatus::Stale;
    }
    match proposal.action {
        OrganizerActionKind::Move => {
            let Some(target) = proposal.proposed_path.as_deref() else {
                return ProposalStatus::Failed;
            };
            let Ok(target) = normalize_canonical_path(target) else {
                return ProposalStatus::Failed;
            };
            if inner
                .path_to_id
                .get(&target)
                .is_some_and(|id| *id != proposal.entry_id)
            {
                return ProposalStatus::Failed;
            }
            inner.path_to_id.remove(&entry.path);
            inner.path_to_id.insert(target.clone(), entry.id);
            let Some(entry) = inner.entries.get_mut(&proposal.entry_id) else {
                return ProposalStatus::Stale;
            };
            let from = entry.path.clone();
            entry.path = target.clone();
            entry.uri = DriveEntry::drive_uri(&target);
            entry.updated_at = now;
            bump_version(&mut entry.version);
            inner.corrections.push(DriveCorrectionRecord {
                id: Uuid::new_v4(),
                version: initial_record_version(),
                from,
                to: target,
                created_at: now,
            });
            ProposalStatus::Applied
        }
        OrganizerActionKind::Tag => {
            let Some(entry) = inner.entries.get_mut(&proposal.entry_id) else {
                return ProposalStatus::Stale;
            };
            entry.tags = apply_tags(&entry.tags, &proposal.proposed_tags);
            entry.updated_at = now;
            bump_version(&mut entry.version);
            ProposalStatus::Applied
        }
        OrganizerActionKind::Archive => {
            let Some(entry) = inner.entries.get_mut(&proposal.entry_id) else {
                return ProposalStatus::Stale;
            };
            entry.status = DriveEntryStatus::Archived;
            entry.updated_at = now;
            bump_version(&mut entry.version);
            ProposalStatus::Applied
        }
        OrganizerActionKind::SetDocKind => {
            let Some(kind) = proposal.proposed_doc_kind.as_ref() else {
                return ProposalStatus::Failed;
            };
            let Some(entry) = inner.entries.get_mut(&proposal.entry_id) else {
                return ProposalStatus::Stale;
            };
            entry.doc_kind = Some(kind.clone());
            entry.updated_at = now;
            bump_version(&mut entry.version);
            ProposalStatus::Applied
        }
        OrganizerActionKind::SetProject => {
            let Some(project) = proposal.proposed_project.as_ref() else {
                return ProposalStatus::Failed;
            };
            let Some(entry) = inner.entries.get_mut(&proposal.entry_id) else {
                return ProposalStatus::Stale;
            };
            entry.project = Some(project.clone());
            entry.updated_at = now;
            bump_version(&mut entry.version);
            ProposalStatus::Applied
        }
        OrganizerActionKind::Dedupe => ProposalStatus::Failed,
    }
}

fn bump_version(version: &mut u64) {
    *version = version.saturating_add(1);
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

pub(crate) fn select_text(content: &str, selector: Option<&str>) -> crate::Result<(String, bool)> {
    const DEFAULT_LINE_LIMIT: usize = 200;
    const DEFAULT_BYTE_LIMIT: usize = 64 * 1024;
    const HARD_LINE_LIMIT: usize = 1_000;
    const HARD_BYTE_LIMIT: usize = 256 * 1024;

    let (start, end, byte_limit) = if let Some(selector) = selector {
        let (start, end) = selector
            .split_once('-')
            .ok_or_else(|| DriveError::InvalidArgs(format!("invalid selector {selector}")))?;
        let start = start
            .parse::<usize>()
            .map_err(|_| DriveError::InvalidArgs(format!("invalid selector {selector}")))?;
        let end = end
            .parse::<usize>()
            .map_err(|_| DriveError::InvalidArgs(format!("invalid selector {selector}")))?;
        if start == 0 || end < start || end - start + 1 > HARD_LINE_LIMIT {
            return Err(DriveError::InvalidArgs(format!(
                "selector {selector} exceeds the 1000-line paging limit"
            )));
        }
        (start, end, HARD_BYTE_LIMIT)
    } else {
        if content.len() <= DEFAULT_BYTE_LIMIT
            && content.lines().take(DEFAULT_LINE_LIMIT + 1).count() <= DEFAULT_LINE_LIMIT
        {
            return Ok((content.to_string(), false));
        }
        (1, DEFAULT_LINE_LIMIT, DEFAULT_BYTE_LIMIT)
    };

    let mut selected = String::new();
    let mut has_more = false;
    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        if line_number < start {
            continue;
        }
        if line_number > end {
            has_more = true;
            break;
        }
        let separator_bytes = usize::from(!selected.is_empty());
        if selected.len() + separator_bytes + line.len() > byte_limit {
            if separator_bytes == 1 && selected.len() < byte_limit {
                selected.push('\n');
            }
            let remaining = byte_limit.saturating_sub(selected.len());
            let boundary = line
                .char_indices()
                .map(|(index, _)| index)
                .take_while(|index| *index <= remaining)
                .last()
                .unwrap_or(0);
            let boundary = if line.len() <= remaining {
                line.len()
            } else {
                boundary
            };
            selected.push_str(&line[..boundary]);
            has_more = true;
            break;
        }
        if separator_bytes == 1 {
            selected.push('\n');
        }
        selected.push_str(line);
    }
    Ok((selected, has_more))
}

pub(crate) fn recency_score(entry: &DriveEntry) -> f32 {
    let age = Utc::now()
        .signed_duration_since(entry.updated_at)
        .num_days()
        .max(0) as f32;
    1.0 / (1.0 + age)
}

pub(crate) fn lexical_score(entry: &DriveEntry, terms: &[String]) -> f32 {
    let haystack = format!(
        "{} {} {} {} {} {}",
        entry.path,
        entry.title.clone().unwrap_or_default(),
        entry.summary.clone().unwrap_or_default(),
        entry.doc_kind.clone().unwrap_or_default(),
        entry.project.clone().unwrap_or_default(),
        entry.tags.join(" ")
    )
    .to_ascii_lowercase();
    terms
        .iter()
        .map(|term| {
            if haystack.contains(term) {
                if entry
                    .title
                    .as_ref()
                    .is_some_and(|title| title.to_ascii_lowercase().contains(term))
                {
                    4.0
                } else if entry.path.to_ascii_lowercase().contains(term) {
                    3.0
                } else {
                    1.0
                }
            } else {
                0.0
            }
        })
        .sum()
}

pub(crate) fn snippet_for(entry: &DriveEntry, query: Option<&str>) -> String {
    let summary = entry
        .summary
        .clone()
        .or_else(|| entry.title.clone())
        .unwrap_or_else(|| entry.path.clone());
    if let Some(query) = query {
        let query = query.to_ascii_lowercase();
        for line in summary.lines() {
            if line.to_ascii_lowercase().contains(&query) {
                return preview(line, 240);
            }
        }
    }
    preview(&summary, 240)
}

pub(crate) fn drive_not_found(
    inner: &Inner,
    requested_display: &str,
    normalized_path: &str,
) -> DriveError {
    let suggestions = nearby_drive_paths(inner, normalized_path);
    if suggestions.is_empty() {
        DriveError::NotFound(requested_display.to_string())
    } else {
        DriveError::NotFound(format!(
            "{requested_display}; nearby paths: {}",
            suggestions.join(", ")
        ))
    }
}

pub(crate) fn nearby_drive_paths(inner: &Inner, normalized_path: &str) -> Vec<String> {
    let requested_lower = normalized_path.to_ascii_lowercase();
    let requested_name = normalized_path
        .rsplit('/')
        .next()
        .unwrap_or(normalized_path)
        .to_ascii_lowercase();
    let parent_prefix = normalized_path
        .rsplit_once('/')
        .map(|(parent, _)| format!("{parent}/"));
    let mut scored = inner
        .path_to_id
        .keys()
        .filter_map(|path| {
            let path_lower = path.to_ascii_lowercase();
            let path_name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
            let mut score = 0usize;
            if parent_prefix
                .as_ref()
                .is_some_and(|prefix| path.starts_with(prefix))
            {
                score += 100;
            }
            if path_lower.contains(&requested_lower) || requested_lower.contains(&path_lower) {
                score += 50;
            }
            score += common_prefix_chars(&requested_name, &path_name).min(30);
            (score > 0).then(|| (score, path.clone()))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left_path), (right_score, right_path)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_path.cmp(right_path))
    });
    scored.into_iter().map(|(_, path)| path).take(3).collect()
}

pub(crate) fn common_prefix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
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

pub(crate) fn validate_host_organizer_config(config: &DriveOrganizerConfig) -> tm_host::Result<()> {
    if config.tier != DriveAutomationTier::Conservative {
        return Err(HostError::InvalidArgs(
            "drive.organize host calls only accept conservative tier; auto-apply is server-policy only"
                .to_string(),
        ));
    }
    if !config.auto_apply.is_empty() {
        return Err(HostError::InvalidArgs(
            "drive.organize host calls cannot include autoApply rules; auto-apply is server-policy only"
                .to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn linked_alias_from_target(target: &str) -> tm_host::Result<String> {
    let target = target.trim();
    if target.is_empty() {
        return Err(HostError::InvalidArgs(
            "drive.unlink requires a linked folder alias".to_string(),
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
            "drive.unlink expects an alias or linked:// URI, got {target}"
        )));
    }
    Ok(target.to_string())
}

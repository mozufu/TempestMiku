use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
    sync::Arc,
};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tm_artifacts::{ArtifactStore, ResourceContent, preview};
use tm_host::{
    GrantDoc, HostError, HostFn, HostRegistry, InvocationCtx, LinkedFolders, ResourceEntry,
    ResourceRegistry, ToolDocs, ToolErrorDoc, ToolExample,
};
use uuid::Uuid;

use crate::{
    DriveAutomationTier, DriveCollisionStrategy, DriveEntry, DriveEntryId, DriveEntryStatus,
    DriveEvidence, DriveLinkPlan, DriveListOptions, DriveOrganizerConfig, DriveOrganizerRunId,
    DriveProvenance, DrivePutOptions, DrivePutResult, DriveSearchOptions, DriveSearchResult,
    DriveUnlinkResult, OrganizerActionKind, OrganizerProposal, OrganizerRun, OrganizerRunStatus,
    PolicyDecision, ProposalStatus, TransducerInput, Transduction, apply_tags, drive_link_policy,
    drive_uri_path, generate_organizer_proposals_for_run, memory_scope_for_project,
    parse_virtual_dir, propose_path, resources::DriveResourceHandler, transduce_document,
    vdir::virtual_query_to_search,
};
use crate::{DriveDedupeMode, types::DriveError};

#[derive(Debug, Clone)]
pub struct InMemoryDriveStore {
    artifacts: ArtifactStore,
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    entries: BTreeMap<DriveEntryId, DriveEntry>,
    path_to_id: BTreeMap<String, DriveEntryId>,
    proposals: BTreeMap<Uuid, OrganizerProposal>,
    organizer_runs: BTreeMap<DriveOrganizerRunId, OrganizerRun>,
    corrections: Vec<DriveCorrection>,
}

#[derive(Debug, Clone)]
struct DriveCorrection {
    from: String,
    to: String,
    created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DriveRead {
    pub entry: DriveEntry,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct DrivePutPlan {
    transduction: Transduction,
    proposed_path: String,
    path: String,
    now: DateTime<Utc>,
}

impl InMemoryDriveStore {
    pub fn new(artifacts: ArtifactStore) -> Self {
        Self {
            artifacts,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub fn put_bytes(
        &self,
        bytes: &[u8],
        options: DrivePutOptions,
    ) -> crate::Result<DrivePutResult> {
        let plan = self.plan_put_bytes(bytes, &options)?;
        self.commit_put_bytes(bytes, options, plan)
    }

    fn plan_put_bytes(
        &self,
        bytes: &[u8],
        options: &DrivePutOptions,
    ) -> crate::Result<DrivePutPlan> {
        if bytes.len() > 5 * 1024 * 1024 {
            return Err(DriveError::InvalidArgs(
                "inline drive.put content is capped at 5 MiB in P5 v1".to_string(),
            ));
        }
        let filename_hint = filename_hint(&options);
        let transduction = transduce_document(TransducerInput {
            bytes,
            filename: filename_hint.as_deref(),
            options: &options,
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

    fn commit_put_bytes(
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
        let mut inner = self.inner.lock();

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
        let inner = self.inner.lock();
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
        let inner = self.inner.lock();
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
        let inner = self.inner.lock();
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
        let from = normalize_canonical_path_path_or_uri(from)?;
        let mut to = normalize_canonical_path_path_or_uri(to)?;
        let now = Utc::now();
        let mut inner = self.inner.lock();
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
        let updated = entry.clone();
        inner.corrections.push(DriveCorrection {
            from,
            to,
            created_at: now,
        });
        Ok(updated)
    }

    pub fn tag_entry(&self, path_or_uri: &str, tags: Vec<String>) -> crate::Result<DriveEntry> {
        let path = normalize_canonical_path_path_or_uri(path_or_uri)?;
        let now = Utc::now();
        let mut inner = self.inner.lock();
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
        let mut inner = self.inner.lock();
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
        let mut inner = self.inner.lock();
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
        let mut inner = self.inner.lock();
        let run = running_organizer_run_mut(&mut inner, run_id)?;
        run.locked_at = Some(now);
        Ok(run.clone())
    }

    pub fn complete_organizer_run(
        &self,
        run_id: DriveOrganizerRunId,
        proposal_ids: Vec<Uuid>,
        now: DateTime<Utc>,
    ) -> crate::Result<OrganizerRun> {
        let mut inner = self.inner.lock();
        let run = running_organizer_run_mut(&mut inner, run_id)?;
        run.status = OrganizerRunStatus::Completed;
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
        let mut inner = self.inner.lock();
        let run = running_organizer_run_mut(&mut inner, run_id)?;
        run.status = if run.attempts >= max_attempts {
            OrganizerRunStatus::Failed
        } else {
            OrganizerRunStatus::Queued
        };
        run.available_at = next_available_at;
        run.locked_at = None;
        run.completed_at = None;
        run.last_error = Some(error);
        Ok(run.clone())
    }

    pub fn organizer_runs(&self) -> Vec<OrganizerRun> {
        self.inner.lock().organizer_runs.values().cloned().collect()
    }

    fn generate_organizer_proposals_for_run(
        &self,
        run_id: DriveOrganizerRunId,
    ) -> crate::Result<Vec<OrganizerProposal>> {
        let mut inner = self.inner.lock();
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
        let mut inner = self.inner.lock();
        let mut ids = Vec::new();
        for proposal in proposals {
            if !organizer_auto_apply_allowed(proposal, config) {
                continue;
            }
            if let Some(stored) = inner.proposals.get_mut(&proposal.id) {
                stored.policy_decision = PolicyDecision::AutoApply;
                ids.push(stored.id);
            }
        }
        ids
    }

    pub fn pending_proposal_ids(&self) -> Vec<Uuid> {
        self.inner
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
        let mut inner = self.inner.lock();
        for id in ids {
            if let Some(proposal) = inner.proposals.get_mut(id) {
                proposal.status = status.clone();
                proposal.updated_at = now;
            }
        }
    }

    pub fn apply_organizer_proposals(&self, ids: &[Uuid]) -> crate::Result<Vec<OrganizerProposal>> {
        let now = Utc::now();
        let mut inner = self.inner.lock();
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
            inner.proposals.insert(*id, proposal.clone());
            applied.push(proposal);
        }
        Ok(applied)
    }

    pub fn proposals(&self) -> Vec<OrganizerProposal> {
        self.inner.lock().proposals.values().cloned().collect()
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
        self.inner
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

pub fn register_drive_functions(
    host_registry: &mut HostRegistry,
    resource_registry: &mut ResourceRegistry,
    store: InMemoryDriveStore,
    linked_folders: Option<LinkedFolders>,
) {
    let linked_for_unlink = linked_folders.clone();
    host_registry.register(Arc::new(DrivePutFn::new(store.clone())));
    host_registry.register(Arc::new(DriveGetFn::new(store.clone())));
    host_registry.register(Arc::new(DriveLsFn::new(store.clone())));
    host_registry.register(Arc::new(DriveMoveFn::new(store.clone())));
    host_registry.register(Arc::new(DriveSearchFn::new(store.clone())));
    host_registry.register(Arc::new(DriveTagFn::new(store.clone())));
    host_registry.register(Arc::new(DriveLinkFn::new(linked_folders)));
    host_registry.register(Arc::new(DriveUnlinkFn::new(linked_for_unlink)));
    host_registry.register(Arc::new(DriveOrganizeFn::new(store.clone())));
    resource_registry.register(Arc::new(DriveResourceHandler::new(store)));
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

fn normalize_canonical_path_path_or_uri(input: &str) -> crate::Result<String> {
    normalize_canonical_path(input)
}

fn normalize_optional_prefix(input: &str) -> crate::Result<String> {
    if input.trim().is_empty() || input.trim() == "/" || input.trim() == "drive://" {
        Ok(String::new())
    } else {
        normalize_canonical_path(input)
    }
}

fn filename_hint(options: &DrivePutOptions) -> Option<String> {
    options
        .suggested_path
        .as_ref()
        .or(options.source_uri.as_ref())
        .and_then(|path| path.rsplit('/').next())
        .map(str::to_string)
}

fn provenance(
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

fn write_proposal(
    entry: &DriveEntry,
    proposed_path: String,
    now: chrono::DateTime<Utc>,
) -> OrganizerProposal {
    OrganizerProposal {
        id: Uuid::new_v4(),
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

fn running_organizer_run_mut(
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

fn organizer_auto_apply_allowed(
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

fn optional_class_match(allowed: &[String], value: Option<&str>) -> bool {
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

fn apply_organizer_proposal(
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
            inner.corrections.push(DriveCorrection {
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
            ProposalStatus::Applied
        }
        OrganizerActionKind::Archive => {
            let Some(entry) = inner.entries.get_mut(&proposal.entry_id) else {
                return ProposalStatus::Stale;
            };
            entry.status = DriveEntryStatus::Archived;
            entry.updated_at = now;
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
            ProposalStatus::Applied
        }
        OrganizerActionKind::Dedupe => ProposalStatus::Failed,
    }
}

fn unique_path(paths: &BTreeMap<String, DriveEntryId>, path: &str) -> String {
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

fn select_text(content: &str, selector: Option<&str>) -> crate::Result<(String, bool)> {
    let Some(selector) = selector else {
        return Ok((content.to_string(), false));
    };
    let (start, end) = selector
        .split_once('-')
        .ok_or_else(|| DriveError::InvalidArgs(format!("invalid selector {selector}")))?;
    let start = start
        .parse::<usize>()
        .map_err(|_| DriveError::InvalidArgs(format!("invalid selector {selector}")))?;
    let end = end
        .parse::<usize>()
        .map_err(|_| DriveError::InvalidArgs(format!("invalid selector {selector}")))?;
    if start == 0 || end < start {
        return Err(DriveError::InvalidArgs(format!(
            "invalid selector {selector}"
        )));
    }
    let lines = content.lines().collect::<Vec<_>>();
    let selected = lines
        .iter()
        .skip(start - 1)
        .take(end - start + 1)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Ok((selected, end < lines.len()))
}

fn recency_score(entry: &DriveEntry) -> f32 {
    let age = Utc::now()
        .signed_duration_since(entry.updated_at)
        .num_days()
        .max(0) as f32;
    1.0 / (1.0 + age)
}

fn lexical_score(entry: &DriveEntry, terms: &[String]) -> f32 {
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

fn snippet_for(entry: &DriveEntry, query: Option<&str>) -> String {
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

fn drive_not_found(inner: &Inner, requested_display: &str, normalized_path: &str) -> DriveError {
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

fn nearby_drive_paths(inner: &Inner, normalized_path: &str) -> Vec<String> {
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

fn common_prefix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

fn drive_error_to_host(err: DriveError) -> HostError {
    match err {
        DriveError::NotFound(target) => HostError::NotFound(target),
        DriveError::InvalidArgs(message) => HostError::InvalidArgs(message),
        DriveError::InvalidPath(path) => HostError::InvalidPath(path),
        DriveError::Collision(path) => HostError::InvalidArgs(format!("drive path exists: {path}")),
        DriveError::Integrity { .. } => HostError::HostCall(err.to_string()),
        DriveError::Store(message) => HostError::HostCall(message),
    }
}

fn drive_put_requires_approval(options: &DrivePutOptions) -> bool {
    options.approval_mode == crate::DriveApprovalMode::RequireApproval
        || (options.auto && options.approval_mode == crate::DriveApprovalMode::Propose)
        || options.overwrite
        || options.collision == DriveCollisionStrategy::Overwrite
}

fn host_drive_put_options(mut options: DrivePutOptions) -> DrivePutOptions {
    if options.approval_mode == crate::DriveApprovalMode::Auto {
        options.approval_mode = crate::DriveApprovalMode::Propose;
    }
    options
}

fn validate_host_organizer_config(config: &DriveOrganizerConfig) -> tm_host::Result<()> {
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

fn linked_alias_from_target(target: &str) -> tm_host::Result<String> {
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrivePutArgs {
    content: Value,
    #[serde(default)]
    options: DrivePutOptions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrivePathArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    selector: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveMoveArgs {
    from: String,
    to: String,
    #[serde(default)]
    collision: DriveCollisionStrategy,
    #[serde(default)]
    overwrite: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveTagArgs {
    path: String,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveLinkArgs {
    host_path: String,
    #[serde(default = "default_link_mode")]
    mode: String,
    #[serde(default)]
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveUnlinkArgs {
    alias: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DriveOrganizeArgs {
    #[serde(default)]
    apply: bool,
    #[serde(default)]
    config: DriveOrganizerConfig,
}

fn default_link_mode() -> String {
    "ro".to_string()
}

macro_rules! drive_fn {
    ($name:ident, $cap:literal, $summary:literal, $approval:literal, $sensitive:expr) => {
        pub struct $name {
            docs: ToolDocs,
            store: Option<InMemoryDriveStore>,
        }

        impl $name {
            fn new(store: InMemoryDriveStore) -> Self {
                Self {
                    docs: drive_docs($cap, $summary, $approval, $sensitive),
                    store: Some(store),
                }
            }
        }

        #[async_trait]
        impl HostFn for $name {
            fn docs(&self) -> &ToolDocs {
                &self.docs
            }

            async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
                self.call_drive(args, ctx).await
            }
        }
    };
}

drive_fn!(
    DrivePutFn,
    "drive.put",
    "Store a document in the local-first drive",
    "policy",
    true
);
drive_fn!(
    DriveGetFn,
    "drive.get",
    "Read a drive document by path or drive:// URI",
    "none",
    false
);
drive_fn!(
    DriveLsFn,
    "drive.ls",
    "List canonical drive paths or virtual directories",
    "none",
    false
);
drive_fn!(
    DriveMoveFn,
    "drive.move",
    "Move a filed drive document",
    "on-write",
    true
);
drive_fn!(
    DriveSearchFn,
    "drive.search",
    "Search filed drive documents",
    "none",
    false
);
drive_fn!(
    DriveTagFn,
    "drive.tag",
    "Add tags to a filed drive document",
    "on-write",
    true
);
drive_fn!(
    DriveOrganizeFn,
    "drive.organize",
    "Generate organizer proposals for filed documents",
    "policy",
    true
);

pub struct DriveLinkFn {
    docs: ToolDocs,
    linked_folders: Option<LinkedFolders>,
}

impl DriveLinkFn {
    fn new(linked_folders: Option<LinkedFolders>) -> Self {
        Self {
            docs: drive_docs(
                "drive.link",
                "Register an approval-gated linked folder plus project memory scope",
                "always",
                true,
            ),
            linked_folders,
        }
    }
}

#[async_trait]
impl HostFn for DriveLinkFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveLinkArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        ctx.require_approval(&format!("drive.link {}", args.host_path))
            .await?;
        let mode = match args.mode.as_str() {
            "ro" => tm_host::FsMode::Ro,
            "rw" => tm_host::FsMode::Rw,
            other => {
                return Err(HostError::InvalidArgs(format!(
                    "drive.link mode must be ro or rw, got {other}"
                )));
            }
        };
        let (plan, policy) = drive_link_policy(&args.host_path, mode, args.project.as_deref())
            .map_err(drive_error_to_host)?;
        if let Some(linked_folders) = &self.linked_folders {
            linked_folders.insert_policy(policy)?;
        }
        ctx.emit_event("drive_linked", drive_linked_payload(&plan))
            .await?;
        serde_json::to_value(plan).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

pub struct DriveUnlinkFn {
    docs: ToolDocs,
    linked_folders: Option<LinkedFolders>,
}

impl DriveUnlinkFn {
    fn new(linked_folders: Option<LinkedFolders>) -> Self {
        Self {
            docs: drive_docs(
                "drive.unlink",
                "Revoke an approval-gated linked folder and its project memory scope",
                "always",
                true,
            ),
            linked_folders,
        }
    }
}

#[async_trait]
impl HostFn for DriveUnlinkFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveUnlinkArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let alias = linked_alias_from_target(&args.alias)?;
        ctx.require_approval(&format!("drive.unlink {alias}"))
            .await?;
        let linked_folders = self
            .linked_folders
            .as_ref()
            .ok_or_else(|| HostError::InvalidPath("no linked folders configured".to_string()))?;
        let policy = linked_folders.remove_policy(&alias)?;
        let result = DriveUnlinkResult {
            alias: alias.clone(),
            canonical_root: policy.root.display().to_string(),
            linked_uri: format!("linked://{alias}/"),
            memory_scope: memory_scope_for_project(&alias),
            revoked_at: Utc::now(),
        };
        ctx.emit_event("drive_unlinked", drive_unlinked_payload(&result))
            .await?;
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DrivePutFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DrivePutArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let mut options = host_drive_put_options(args.options);
        if options.session_id.is_none() && !ctx.session_id.is_empty() {
            options.session_id = Some(ctx.session_id.clone());
        }
        let bytes = content_to_bytes(&self.store, &args.content)?;
        let store = self.store.as_ref().expect("drive store");
        let plan = store
            .plan_put_bytes(&bytes, &options)
            .map_err(drive_error_to_host)?;
        if drive_put_requires_approval(&options) {
            ctx.require_approval(&format!("drive.put {}", plan.path))
                .await?;
        }
        let result = self
            .store
            .as_ref()
            .expect("drive store")
            .commit_put_bytes(&bytes, options, plan)
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_put", drive_entry_event_payload("put", &result.entry))
            .await?;
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveGetFn {
    async fn call_drive(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DrivePathArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let target = args
            .uri
            .or(args.path)
            .ok_or_else(|| HostError::InvalidArgs("drive.get requires path or uri".to_string()))?;
        let uri = if target.starts_with("drive://") {
            target
        } else {
            format!(
                "drive://{}",
                normalize_canonical_path(&target).map_err(drive_error_to_host)?
            )
        };
        let content = self
            .store
            .as_ref()
            .expect("drive store")
            .resource_content(&uri, args.selector.as_deref())
            .map_err(drive_error_to_host)?;
        serde_json::to_value(content).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveLsFn {
    async fn call_drive(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let options: DriveListOptions =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let entries = self
            .store
            .as_ref()
            .expect("drive store")
            .list(options)
            .map_err(drive_error_to_host)?;
        serde_json::to_value(entries).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveMoveFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveMoveArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        ctx.require_approval(&format!("drive.move {} -> {}", args.from, args.to))
            .await?;
        let collision = if args.overwrite {
            DriveCollisionStrategy::Overwrite
        } else {
            args.collision
        };
        let entry = self
            .store
            .as_ref()
            .expect("drive store")
            .move_entry(&args.from, &args.to, collision)
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_moved", drive_moved_payload(&args.from, &entry))
            .await?;
        serde_json::to_value(entry).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveSearchFn {
    async fn call_drive(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let options: DriveSearchOptions =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let results = self
            .store
            .as_ref()
            .expect("drive store")
            .search(options)
            .map_err(drive_error_to_host)?;
        serde_json::to_value(results).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveTagFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveTagArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        ctx.require_approval(&format!("drive.tag {}", args.path))
            .await?;
        let entry = self
            .store
            .as_ref()
            .expect("drive store")
            .tag_entry(&args.path, args.tags)
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_tagged", drive_entry_event_payload("tag", &entry))
            .await?;
        serde_json::to_value(entry).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveOrganizeFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveOrganizeArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        validate_host_organizer_config(&args.config)?;
        let store = self.store.as_ref().expect("drive store");
        ctx.emit_event(
            "drive_organizer_started",
            organizer_started_payload(args.apply, &args.config),
        )
        .await?;
        if !args.apply {
            let proposals = match store.organize_with_config(args.config.clone()) {
                Ok(proposals) => proposals,
                Err(err) => {
                    ctx.emit_event(
                        "drive_organizer_failed",
                        organizer_failed_payload(args.apply, &args.config, &err.to_string()),
                    )
                    .await?;
                    return Err(drive_error_to_host(err));
                }
            };
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &args.config, &proposals),
            )
            .await?;
            return serde_json::to_value(proposals)
                .map_err(|err| HostError::HostCall(err.to_string()));
        }

        let mut ids = store.pending_proposal_ids();
        let mut generated = Vec::<OrganizerProposal>::new();
        if ids.is_empty() {
            generated = match store.organize_with_config(args.config.clone()) {
                Ok(proposals) => proposals,
                Err(err) => {
                    ctx.emit_event(
                        "drive_organizer_failed",
                        organizer_failed_payload(args.apply, &args.config, &err.to_string()),
                    )
                    .await?;
                    return Err(drive_error_to_host(err));
                }
            };
            ids = generated
                .iter()
                .filter(|proposal| {
                    matches!(
                        proposal.status,
                        ProposalStatus::Pending | ProposalStatus::Approved
                    )
                })
                .into_iter()
                .map(|proposal| proposal.id)
                .collect();
        }
        if ids.is_empty() {
            emit_drive_write_proposals(ctx, &generated).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &args.config, &generated),
            )
            .await?;
            return serde_json::to_value(generated)
                .map_err(|err| HostError::HostCall(err.to_string()));
        }
        if let Err(err) = ctx.require_approval("drive.organize apply").await {
            let status = match err {
                HostError::ApprovalDenied(_) => ProposalStatus::Denied,
                HostError::ApprovalTimeout(_) => ProposalStatus::Failed,
                _ => ProposalStatus::Failed,
            };
            store.mark_proposals_status(&ids, status);
            let proposals = store
                .proposals()
                .into_iter()
                .filter(|proposal| ids.contains(&proposal.id))
                .collect::<Vec<_>>();
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_failed",
                organizer_failed_payload_with_proposals(
                    args.apply,
                    &args.config,
                    &err.to_string(),
                    &proposals,
                ),
            )
            .await?;
            return Err(err);
        }
        let proposals = match store.apply_organizer_proposals(&ids) {
            Ok(proposals) => proposals,
            Err(err) => {
                ctx.emit_event(
                    "drive_organizer_failed",
                    organizer_failed_payload(args.apply, &args.config, &err.to_string()),
                )
                .await?;
                return Err(drive_error_to_host(err));
            }
        };
        if generated.is_empty() {
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &args.config, &proposals),
            )
            .await?;
            return serde_json::to_value(proposals)
                .map_err(|err| HostError::HostCall(err.to_string()));
        }
        for updated in proposals {
            if let Some(proposal) = generated
                .iter_mut()
                .find(|proposal| proposal.id == updated.id)
            {
                *proposal = updated;
            } else {
                generated.push(updated);
            }
        }
        emit_drive_write_proposals(ctx, &generated).await?;
        ctx.emit_event(
            "drive_organizer_completed",
            organizer_completed_payload(args.apply, &args.config, &generated),
        )
        .await?;
        serde_json::to_value(generated).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

async fn emit_drive_write_proposals(
    ctx: &InvocationCtx,
    proposals: &[OrganizerProposal],
) -> tm_host::Result<()> {
    for proposal in proposals {
        ctx.emit_event("write_proposal", drive_write_proposal_payload(proposal))
            .await?;
    }
    Ok(())
}

fn organizer_started_payload(apply: bool, config: &DriveOrganizerConfig) -> Value {
    json!({
        "apply": apply,
        "tier": config.tier,
        "autoApplyRules": config.auto_apply.len(),
    })
}

fn organizer_completed_payload(
    apply: bool,
    config: &DriveOrganizerConfig,
    proposals: &[OrganizerProposal],
) -> Value {
    json!({
        "apply": apply,
        "tier": config.tier,
        "runId": proposals.first().map(|proposal| proposal.source_run_id),
        "proposalCount": proposals.len(),
        "proposals": proposals.iter().map(organizer_proposal_event_payload).collect::<Vec<_>>(),
        "resourceRefs": proposals.iter().flat_map(organizer_resource_refs).collect::<Vec<_>>(),
    })
}

fn organizer_failed_payload(apply: bool, config: &DriveOrganizerConfig, error: &str) -> Value {
    organizer_failed_payload_with_proposals(apply, config, error, &[])
}

fn organizer_failed_payload_with_proposals(
    apply: bool,
    config: &DriveOrganizerConfig,
    error: &str,
    proposals: &[OrganizerProposal],
) -> Value {
    json!({
        "apply": apply,
        "tier": config.tier,
        "error": error,
        "runId": proposals.first().map(|proposal| proposal.source_run_id),
        "proposalCount": proposals.len(),
        "proposals": proposals.iter().map(organizer_proposal_event_payload).collect::<Vec<_>>(),
        "resourceRefs": proposals.iter().flat_map(organizer_resource_refs).collect::<Vec<_>>(),
    })
}

fn drive_write_proposal_payload(proposal: &OrganizerProposal) -> Value {
    let mut payload = organizer_proposal_event_payload(proposal);
    if let Value::Object(map) = &mut payload {
        map.insert("kind".to_string(), json!("drive"));
        map.insert("proposalId".to_string(), json!(proposal.id));
        map.insert("driveProposal".to_string(), json!(proposal));
    }
    payload
}

fn drive_entry_event_payload(action: &str, entry: &DriveEntry) -> Value {
    let title = match action {
        "put" => "Filed drive document",
        "tag" => "Tagged drive document",
        _ => "Updated drive document",
    };
    drive_entry_payload(action, title, &entry.path, entry)
}

fn drive_moved_payload(from_path: &str, entry: &DriveEntry) -> Value {
    let mut payload = drive_entry_payload(
        "move",
        "Moved drive document",
        &format!("{from_path} -> {}", entry.path),
        entry,
    );
    if let Value::Object(map) = &mut payload {
        map.insert("fromPath".to_string(), json!(from_path));
        map.insert(
            "fromUri".to_string(),
            json!(DriveEntry::drive_uri(from_path)),
        );
        map.insert("toPath".to_string(), json!(&entry.path));
        map.insert("toUri".to_string(), json!(&entry.uri));
        map.insert(
            "resourceRefs".to_string(),
            json!([
                {
                    "role": "previous",
                    "uri": DriveEntry::drive_uri(from_path),
                    "kind": "drive_document",
                    "title": drive_path_title(from_path),
                },
                drive_entry_resource_ref("current", entry),
            ]),
        );
    }
    payload
}

fn drive_entry_payload(
    action: &str,
    preview_title: &str,
    preview_subtitle: &str,
    entry: &DriveEntry,
) -> Value {
    json!({
        "action": action,
        "entryId": entry.id,
        "path": &entry.path,
        "uri": &entry.uri,
        "title": &entry.title,
        "docKind": &entry.doc_kind,
        "project": &entry.project,
        "tags": &entry.tags,
        "mime": &entry.mime,
        "sizeBytes": entry.size_bytes,
        "contentHash": &entry.content_hash,
        "sourceUri": &entry.source_uri,
        "status": entry.status,
        "preview": {
            "title": preview_title,
            "subtitle": compact_preview_text(preview_subtitle, 160),
            "snippet": drive_entry_snippet(entry),
        },
        "resourceRefs": [drive_entry_resource_ref("document", entry)],
    })
}

fn drive_entry_resource_ref(role: &str, entry: &DriveEntry) -> Value {
    json!({
        "role": role,
        "uri": &entry.uri,
        "kind": "drive_document",
        "title": drive_entry_title(entry),
        "path": &entry.path,
    })
}

fn drive_entry_title(entry: &DriveEntry) -> String {
    entry
        .title
        .clone()
        .unwrap_or_else(|| drive_path_title(&entry.path))
}

fn drive_path_title(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn drive_entry_snippet(entry: &DriveEntry) -> Option<String> {
    entry
        .summary
        .as_deref()
        .or_else(|| {
            entry.attributes.iter().find_map(|attribute| {
                attribute
                    .evidence
                    .as_ref()
                    .map(|evidence| evidence.snippet.as_str())
            })
        })
        .map(|snippet| compact_preview_text(snippet, 180))
}

fn drive_linked_payload(plan: &DriveLinkPlan) -> Value {
    json!({
        "action": "link",
        "alias": &plan.alias,
        "linkedUri": &plan.linked_uri,
        "mode": &plan.mode,
        "project": &plan.project,
        "memoryScope": &plan.memory_scope,
        "requiresApproval": plan.requires_approval,
        "preview": {
            "title": "Linked project folder",
            "subtitle": compact_preview_text(&format!("{} -> {}", plan.project, plan.linked_uri), 160),
            "snippet": compact_preview_text(&plan.canonical_root, 180),
        },
        "resourceRefs": [{
            "role": "linked",
            "uri": &plan.linked_uri,
            "kind": "linked_folder",
            "title": &plan.project,
        }],
    })
}

fn drive_unlinked_payload(result: &DriveUnlinkResult) -> Value {
    json!({
        "action": "unlink",
        "alias": &result.alias,
        "linkedUri": &result.linked_uri,
        "memoryScope": &result.memory_scope,
        "revokedAt": result.revoked_at,
        "preview": {
            "title": "Unlinked project folder",
            "subtitle": compact_preview_text(&result.linked_uri, 160),
            "snippet": compact_preview_text(&result.canonical_root, 180),
        },
        "resourceRefs": [{
            "role": "revoked",
            "uri": &result.linked_uri,
            "kind": "linked_folder",
            "title": &result.alias,
        }],
    })
}

fn organizer_proposal_event_payload(proposal: &OrganizerProposal) -> Value {
    json!({
        "proposalId": proposal.id,
        "runId": proposal.source_run_id,
        "action": proposal.action,
        "status": proposal.status,
        "policyDecision": proposal.policy_decision,
        "sourcePath": proposal.source_path,
        "sourceUri": DriveEntry::drive_uri(&proposal.source_path),
        "proposedPath": proposal.proposed_path,
        "proposedUri": proposal.proposed_path.as_deref().map(DriveEntry::drive_uri),
        "proposedTags": proposal.proposed_tags,
        "proposedDocKind": proposal.proposed_doc_kind,
        "proposedProject": proposal.proposed_project,
        "confidence": proposal.confidence,
        "preview": organizer_preview(proposal),
        "resourceRefs": organizer_resource_refs(proposal),
    })
}

fn organizer_resource_refs(proposal: &OrganizerProposal) -> Vec<Value> {
    let mut refs = vec![json!({
        "role": "source",
        "uri": DriveEntry::drive_uri(&proposal.source_path),
        "kind": "drive_document",
        "title": proposal.source_path.rsplit('/').next().unwrap_or(&proposal.source_path),
    })];
    if let Some(path) = proposal.proposed_path.as_deref() {
        refs.push(json!({
            "role": "proposed",
            "uri": DriveEntry::drive_uri(path),
            "kind": "drive_document",
            "title": path.rsplit('/').next().unwrap_or(path),
        }));
    }
    refs
}

fn organizer_preview(proposal: &OrganizerProposal) -> Value {
    let title = match proposal.action {
        OrganizerActionKind::Move => "Move drive document",
        OrganizerActionKind::Tag => "Tag drive document",
        OrganizerActionKind::Dedupe => "Deduplicate drive document",
        OrganizerActionKind::Archive => "Archive drive document",
        OrganizerActionKind::SetDocKind => "Set document kind",
        OrganizerActionKind::SetProject => "Set project",
    };
    let subtitle = proposal
        .proposed_path
        .as_ref()
        .map(|path| format!("{} -> {path}", proposal.source_path))
        .unwrap_or_else(|| proposal.source_path.clone());
    json!({
        "title": title,
        "subtitle": compact_preview_text(&subtitle, 160),
        "snippet": proposal.evidence.first().map(|evidence| compact_preview_text(&evidence.snippet, 180)),
    })
}

fn compact_preview_text(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut out = compact.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
}

fn content_to_bytes(
    store: &Option<InMemoryDriveStore>,
    content: &Value,
) -> tm_host::Result<Vec<u8>> {
    if let Some(text) = content.as_str() {
        if text.starts_with("blob:sha256:") {
            return store
                .as_ref()
                .expect("drive store")
                .artifacts
                .read_blob(text)
                .map_err(|err| HostError::NotFound(err.to_string()));
        }
        return Ok(text.as_bytes().to_vec());
    }
    if let Some(text) = content.get("text").and_then(Value::as_str) {
        return Ok(text.as_bytes().to_vec());
    }
    if let Some(uri) = content.get("uri").and_then(Value::as_str)
        && uri.starts_with("blob:sha256:")
    {
        return store
            .as_ref()
            .expect("drive store")
            .artifacts
            .read_blob(uri)
            .map_err(|err| HostError::NotFound(err.to_string()));
    }
    if let Some(uri) = content.get("uri").and_then(Value::as_str) {
        return Err(HostError::InvalidArgs(format!(
            "drive.put content.uri only supports blob:sha256: refs in P5 v1, got {uri}"
        )));
    }
    serde_json::to_vec(content).map_err(|err| HostError::InvalidArgs(err.to_string()))
}

fn drive_docs(name: &str, summary: &str, approval: &str, sensitive: bool) -> ToolDocs {
    ToolDocs {
        name: name.to_string(),
        namespace: "drive".to_string(),
        summary: summary.to_string(),
        description: Some(format!(
            "{summary}. Drive is local-first and exposes user documents through drive:// resources."
        )),
        signature: drive_signature(name),
        args_schema: drive_args_schema(name),
        result_schema: Some(drive_result_schema(name)),
        examples: drive_examples(name),
        errors: vec![
            tool_error(
                "CapabilityDeniedError",
                "The session lacks the drive capability grant.",
                false,
            ),
            tool_error(
                "ApprovalDeniedError",
                "The user denies a required drive mutation.",
                false,
            ),
            tool_error(
                "ApprovalTimeoutError",
                "A required drive mutation times out and defaults to deny.",
                true,
            ),
            tool_error(
                "InvalidPathError",
                "A drive path contains traversal, a raw host path, or a resource URI from another scheme.",
                false,
            ),
            tool_error(
                "NotFoundError",
                "The requested drive document does not exist.",
                false,
            ),
            tool_error(
                "HostCallError",
                "The drive store or blob integrity check fails.",
                false,
            ),
        ],
        grants: vec![GrantDoc {
            kind: "capability".to_string(),
            description: format!("Requires the {name} capability grant."),
        }],
        sensitive,
        approval: approval.to_string(),
        since: "P5".to_string(),
        stability: "experimental".to_string(),
    }
}

fn drive_signature(name: &str) -> String {
    match name {
        "drive.put" => "drive.put(content: DriveContent, opts?: DrivePutOptions): Promise<DrivePutResult>",
        "drive.get" => "drive.get(pathOrUri: string, opts?: { selector?: ResourceSelector }): Promise<ResourceContent>",
        "drive.ls" => "drive.ls(pathOrQuery?: string, opts?: DriveListOptions): Promise<DriveEntry[]>",
        "drive.move" => "drive.move(from: string, to: string, opts?: DriveMoveOptions): Promise<DriveEntry>",
        "drive.search" => "drive.search(query?: string, opts?: DriveSearchOptions): Promise<DriveSearchResult[]>",
        "drive.tag" => "drive.tag(path: string, tags: string[]): Promise<DriveEntry>",
        "drive.link" => "drive.link(hostPath: string, mode?: 'ro' | 'rw', opts?: { project?: string }): Promise<DriveLinkPlan>",
        "drive.unlink" => "drive.unlink(aliasOrUri: string): Promise<DriveUnlinkResult>",
        "drive.organize" => "drive.organize(opts?: DriveOrganizeOptions): Promise<OrganizerProposal[]>",
        _ => "drive.unknown(args): Promise<unknown>",
    }
    .to_string()
}

fn drive_args_schema(name: &str) -> Value {
    match name {
        "drive.put" => json!({
            "type": "object",
            "required": ["content"],
            "properties": {
                "content": {},
                "options": { "type": "object" }
            }
        }),
        "drive.get" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "uri": { "type": "string" },
                "selector": { "type": "string" }
            }
        }),
        "drive.move" => json!({
            "type": "object",
            "required": ["from", "to"],
            "properties": {
                "from": { "type": "string" },
                "to": { "type": "string" },
                "collision": { "enum": ["keep-both", "reject", "overwrite"] },
                "overwrite": { "type": "boolean" }
            }
        }),
        "drive.tag" => json!({
            "type": "object",
            "required": ["path", "tags"],
            "properties": {
                "path": { "type": "string" },
                "tags": { "type": "array", "items": { "type": "string" } }
            }
        }),
        "drive.link" => json!({
            "type": "object",
            "required": ["hostPath"],
            "properties": {
                "hostPath": { "type": "string" },
                "mode": { "enum": ["ro", "rw"], "default": "ro" },
                "project": { "type": "string" }
            }
        }),
        "drive.unlink" => json!({
            "type": "object",
            "required": ["alias"],
            "properties": {
                "alias": { "type": "string", "description": "Linked folder alias or linked:// URI." }
            }
        }),
        "drive.organize" => json!({
            "type": "object",
            "properties": {
                "apply": { "type": "boolean" },
                "config": {
                    "type": "object",
                    "additionalProperties": false,
                    "description": "Host SDK calls generate conservative proposals only; auto-apply rules are trusted server policy.",
                    "properties": {
                        "tier": { "enum": ["conservative"], "default": "conservative" }
                    }
                }
            }
        }),
        _ => json!({ "type": "object" }),
    }
}

fn drive_result_schema(name: &str) -> Value {
    match name {
        "drive.get" => json!({ "type": "object", "description": "ResourceContent" }),
        "drive.search" | "drive.organize" | "drive.ls" => json!({ "type": "array" }),
        _ => json!({ "type": "object" }),
    }
}

fn drive_examples(name: &str) -> Vec<ToolExample> {
    let code = match name {
        "drive.put" => {
            "const filed = await drive.put('# Note\\nhello', { auto: true, project: 'TempestMiku' });"
        }
        "drive.get" => {
            "const doc = await drive.get('projects/tempestmiku/docs/note.md', { selector: '1-20' });"
        }
        "drive.ls" => "const invoices = await drive.ls('/by-type/invoice');",
        "drive.move" => {
            "await drive.move('inbox/today/note.md', 'projects/tempestmiku/notes/note.md');"
        }
        "drive.search" => {
            "const hits = await drive.search('approval policy', { project: 'TempestMiku', returnSnippets: true });"
        }
        "drive.tag" => "await drive.tag('projects/tempestmiku/notes/note.md', ['planning']);",
        "drive.link" => {
            "const plan = await drive.link('/path/to/project', 'ro', { project: 'TempestMiku' });"
        }
        "drive.unlink" => "const revoked = await drive.unlink('tempestmiku');",
        "drive.organize" => "const proposals = await drive.organize();",
        _ => "await tools.call('drive.unknown', {});",
    };
    vec![ToolExample {
        title: None,
        code: code.to_string(),
        notes: None,
    }]
}

fn tool_error(name: &str, when: &str, retryable: bool) -> ToolErrorDoc {
    ToolErrorDoc {
        name: name.to_string(),
        when: when.to_string(),
        retryable,
    }
}

#[cfg(test)]
mod tests {
    use tm_host::{
        ApprovalDecision, ApprovalPolicy, CapabilityGrants, DefaultDenyApprovalPolicy,
        HostEventSink,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingHostEventSink {
        events: parking_lot::Mutex<Vec<(String, Value)>>,
    }

    #[async_trait]
    impl HostEventSink for RecordingHostEventSink {
        async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
            self.events
                .lock()
                .push((event_type.to_string(), payload_json));
            Ok(())
        }
    }

    impl RecordingHostEventSink {
        fn events(&self) -> Vec<(String, Value)> {
            self.events.lock().clone()
        }
    }

    struct StaticApproval(ApprovalDecision);

    #[async_trait]
    impl ApprovalPolicy for StaticApproval {
        async fn request(
            &self,
            _action: &str,
            _timeout: std::time::Duration,
        ) -> tm_host::Result<ApprovalDecision> {
            Ok(self.0)
        }
    }

    fn store() -> (tempfile::TempDir, InMemoryDriveStore) {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::open(dir.path(), "drive").unwrap();
        let store = InMemoryDriveStore::new(artifacts);
        (dir, store)
    }

    #[test]
    fn rejects_raw_host_and_traversal_paths() {
        assert!(normalize_canonical_path("/Users/brian/file.txt").is_err());
        assert!(normalize_canonical_path("C:/Users/brian/file.txt").is_err());
        assert!(normalize_canonical_path("notes/../secret.txt").is_err());
        assert_eq!(
            normalize_canonical_path("drive://notes/./a.txt").unwrap(),
            "notes/a.txt"
        );
    }

    #[test]
    fn put_get_search_and_virtual_dirs_work_offline() {
        let (_dir, store) = store();
        let result = store
            .put_bytes(
                b"# Invoice 42\nDate 2026-07-08\nAmount due $42.00",
                DrivePutOptions {
                    auto: true,
                    tags: vec!["tax".to_string()],
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        assert_eq!(result.entry.path, "finance/2026/invoice/invoice-42.txt");
        store
            .put_bytes(
                b"# Meeting Notes\nRecent unrelated project chatter",
                DrivePutOptions {
                    suggested_path: Some("notes/recent.md".to_string()),
                    doc_kind: Some("note".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();

        let content = store.resource_content(&result.uri, Some("1-1")).unwrap();
        assert_eq!(content.content, "# Invoice 42");
        assert!(content.has_more);

        let hits = store
            .search(DriveSearchOptions {
                query: Some("invoice".to_string()),
                return_snippets: true,
                ..DriveSearchOptions::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].uri, result.uri);
        assert!(hits[0].snippet.as_ref().unwrap().contains("Invoice"));

        let by_type = store
            .list(DriveListOptions {
                path: Some("/by-type/invoice".to_string()),
                recursive: true,
                ..DriveListOptions::default()
            })
            .unwrap();
        assert_eq!(by_type.len(), 1);
    }

    #[test]
    fn drive_put_classifies_representative_documents_without_llm() {
        let (_dir, store) = store();
        let cases = [
            (
                b"Meeting note\nFollow up with Miku about drive approvals.".as_slice(),
                "note",
                "notes/meeting.txt",
            ),
            (
                b"Receipt\nTotal paid: $18.25\nDate: 2026-07-08".as_slice(),
                "receipt",
                "receipts/lunch.txt",
            ),
            (
                b"Abstract\nA small agent runtime study.\nDOI: 10.1000/example\nReferences"
                    .as_slice(),
                "paper",
                "papers/runtime.txt",
            ),
            (
                b"Roadmap\nMilestone P5 adds drive and research workspace.".as_slice(),
                "project_doc",
                "projects/tempestmiku/roadmap.txt",
            ),
        ];

        for (content, expected_kind, path) in cases {
            let filed = store
                .put_bytes(
                    content,
                    DrivePutOptions {
                        suggested_path: Some(path.to_string()),
                        ..DrivePutOptions::default()
                    },
                )
                .unwrap();
            assert_eq!(filed.entry.doc_kind.as_deref(), Some(expected_kind));
            assert!(
                filed
                    .entry
                    .attributes
                    .iter()
                    .any(|attr| attr.key == "doc_kind" && attr.value == expected_kind)
            );
        }
    }

    #[test]
    fn duplicate_content_reuses_blob_but_allows_distinct_paths() {
        let (_dir, store) = store();
        let one = store
            .put_bytes(
                b"same",
                DrivePutOptions {
                    suggested_path: Some("notes/a.txt".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let two = store
            .put_bytes(
                b"same",
                DrivePutOptions {
                    suggested_path: Some("notes/b.txt".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        assert_eq!(one.entry.blob_uri, two.entry.blob_uri);
        assert_ne!(one.entry.id, two.entry.id);
    }

    #[test]
    fn missing_get_returns_nearby_drive_paths_without_host_paths() {
        let (dir, store) = store();
        store
            .put_bytes(
                b"alpha",
                DrivePutOptions {
                    suggested_path: Some("notes/a.md".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        store
            .put_bytes(
                b"beta",
                DrivePutOptions {
                    suggested_path: Some("notes/b.md".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();

        let err = store.get("notes/missing.md").unwrap_err().to_string();
        assert!(err.contains("drive entry not found: notes/missing.md"));
        assert!(err.contains("nearby paths: notes/a.md, notes/b.md"));
        assert!(!err.contains(&dir.path().display().to_string()));
    }

    #[test]
    fn organizer_run_claim_heartbeat_complete_and_stale_reclaim() {
        let (_dir, store) = store();
        let now = Utc::now();
        let queued = store.enqueue_organizer_run("manual", now - Duration::seconds(1));

        let claimed = store
            .claim_ready_organizer_run(now, Duration::seconds(30))
            .unwrap()
            .expect("ready organizer run");
        assert_eq!(claimed.id, queued.id);
        assert_eq!(claimed.status, OrganizerRunStatus::Running);
        assert_eq!(claimed.attempts, 1);
        assert_eq!(claimed.locked_at, Some(now));
        assert!(
            store
                .claim_ready_organizer_run(now + Duration::seconds(10), Duration::seconds(30))
                .unwrap()
                .is_none()
        );

        let reclaimed = store
            .claim_ready_organizer_run(now + Duration::seconds(31), Duration::seconds(30))
            .unwrap()
            .expect("stale organizer run");
        assert_eq!(reclaimed.id, queued.id);
        assert_eq!(reclaimed.status, OrganizerRunStatus::Running);
        assert_eq!(reclaimed.attempts, 2);

        let heartbeat_at = now + Duration::seconds(35);
        let heartbeated = store
            .heartbeat_organizer_run(queued.id, heartbeat_at)
            .unwrap();
        assert_eq!(heartbeated.locked_at, Some(heartbeat_at));

        let completed_at = now + Duration::seconds(40);
        let completed = store
            .complete_organizer_run(queued.id, vec![Uuid::nil()], completed_at)
            .unwrap();
        assert_eq!(completed.status, OrganizerRunStatus::Completed);
        assert_eq!(completed.locked_at, None);
        assert_eq!(completed.completed_at, Some(completed_at));
        assert_eq!(completed.proposal_ids, vec![Uuid::nil()]);
        assert!(
            store
                .claim_ready_organizer_run(now + Duration::seconds(90), Duration::seconds(30))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn organizer_run_failure_records_error_and_bounded_retry() {
        let (_dir, store) = store();
        let now = Utc::now();
        let queued = store.enqueue_organizer_run("manual", now);
        let claimed = store
            .claim_ready_organizer_run(now, Duration::seconds(30))
            .unwrap()
            .expect("ready organizer run");
        assert_eq!(claimed.id, queued.id);

        let retry_at = now + Duration::seconds(60);
        let retryable = store
            .fail_organizer_run(queued.id, "transient store error".to_string(), retry_at, 2)
            .unwrap();
        assert_eq!(retryable.status, OrganizerRunStatus::Queued);
        assert_eq!(retryable.locked_at, None);
        assert_eq!(retryable.available_at, retry_at);
        assert_eq!(
            retryable.last_error.as_deref(),
            Some("transient store error")
        );
        assert!(
            store
                .claim_ready_organizer_run(now + Duration::seconds(30), Duration::seconds(30))
                .unwrap()
                .is_none()
        );

        let second = store
            .claim_ready_organizer_run(retry_at, Duration::seconds(30))
            .unwrap()
            .expect("retryable organizer run");
        assert_eq!(second.attempts, 2);
        let terminal = store
            .fail_organizer_run(
                queued.id,
                "terminal policy error".to_string(),
                retry_at + Duration::seconds(60),
                2,
            )
            .unwrap();
        assert_eq!(terminal.status, OrganizerRunStatus::Failed);
        assert_eq!(terminal.locked_at, None);
        assert_eq!(
            terminal.last_error.as_deref(),
            Some("terminal policy error")
        );
        assert!(
            store
                .claim_ready_organizer_run(retry_at + Duration::seconds(90), Duration::seconds(30))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn duplicate_organizer_workers_cannot_claim_same_run() {
        let (_dir, store) = store();
        let now = Utc::now();
        let queued = store.enqueue_organizer_run("scheduled", now);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

        let handles = (0..2)
            .map(|_| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store
                        .claim_ready_organizer_run(now, Duration::seconds(30))
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        let claims = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        let claimed = claims
            .iter()
            .filter(|claim| claim.as_ref().is_some_and(|run| run.id == queued.id))
            .count();

        assert_eq!(claimed, 1);
        assert_eq!(
            store.organizer_runs()[0].status,
            OrganizerRunStatus::Running
        );
    }

    #[test]
    fn drive_organize_records_completed_run_and_proposal_refs() {
        let (_dir, store) = store();
        store
            .put_bytes(
                b"# Raw\norganizer should move this into project notes",
                DrivePutOptions {
                    suggested_path: Some("inbox/raw.md".to_string()),
                    project: Some("TempestMiku".to_string()),
                    doc_kind: Some("note".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();

        let proposals = store.organize().unwrap();
        assert_eq!(proposals.len(), 1);
        let runs = store.organizer_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, OrganizerRunStatus::Completed);
        assert_eq!(runs[0].proposal_ids, vec![proposals[0].id]);
        assert_eq!(proposals[0].source_run_id, runs[0].id);
    }

    #[test]
    fn same_path_collision_keeps_both_by_default() {
        let (_dir, store) = store();
        let one = store
            .put_bytes(
                b"one",
                DrivePutOptions {
                    suggested_path: Some("notes/a.txt".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let two = store
            .put_bytes(
                b"two",
                DrivePutOptions {
                    suggested_path: Some("notes/a.txt".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        assert_eq!(one.entry.path, "notes/a.txt");
        assert_eq!(two.entry.path, "notes/a-2.txt");
    }

    #[tokio::test]
    async fn host_missing_grant_fails_closed() {
        let (_dir, store) = store();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store, None);
        let ctx = InvocationCtx::new(CapabilityGrants::default());
        let err = host
            .invoke("drive.put", json!({ "content": "hello" }), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err, HostError::CapabilityDenied("drive.put".to_string()));
    }

    #[tokio::test]
    async fn auto_put_requires_policy_before_write() {
        let (_dir, store) = store();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store.clone(), None);

        let timeout_ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow("drive.put"),
            Arc::new(DefaultDenyApprovalPolicy),
            std::time::Duration::from_millis(1),
        );
        let err = host
            .invoke(
                "drive.put",
                json!({
                    "content": "hello",
                    "options": {
                        "auto": true,
                        "suggestedPath": "notes/a.txt"
                    }
                }),
                &timeout_ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::ApprovalTimeout(_)));
        assert!(store.get("notes/a.txt").is_err());
        assert!(store.proposals().is_empty());

        let denied_ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow("drive.put"),
            Arc::new(StaticApproval(ApprovalDecision::Denied)),
            std::time::Duration::from_secs(1),
        );
        let err = host
            .invoke(
                "drive.put",
                json!({
                    "content": "hello",
                    "options": {
                        "auto": true,
                        "suggestedPath": "notes/a.txt"
                    }
                }),
                &denied_ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::ApprovalDenied(_)));
        assert!(store.get("notes/a.txt").is_err());

        let approved_ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow("drive.put"),
            Arc::new(StaticApproval(ApprovalDecision::Approved)),
            std::time::Duration::from_secs(1),
        );
        let approved = host
            .invoke(
                "drive.put",
                json!({
                    "content": "hello",
                    "options": {
                        "auto": true,
                        "suggestedPath": "notes/a.txt"
                    }
                }),
                &approved_ctx,
            )
            .await
            .unwrap();
        assert_eq!(approved["filed"], json!(true));
        assert!(store.get("notes/a.txt").is_ok());

        let err = host
            .invoke(
                "drive.put",
                json!({
                    "content": "low risk",
                    "options": {
                        "auto": true,
                        "approvalMode": "auto",
                        "suggestedPath": "notes/b.txt"
                    }
                }),
                &timeout_ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::ApprovalTimeout(_)));
        assert!(store.get("notes/b.txt").is_err());
    }

    #[tokio::test]
    async fn host_drive_put_accepts_blob_refs_and_rejects_other_uri_refs() {
        let (_dir, store) = store();
        let blob_uri = store.artifacts.put_blob(b"from blob ref").unwrap();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store.clone(), None);
        let ctx = InvocationCtx::new(CapabilityGrants::default().allow("drive.put"));

        let by_object = host
            .invoke(
                "drive.put",
                json!({
                    "content": { "uri": blob_uri.clone() },
                    "options": { "suggestedPath": "notes/blob-object.txt" }
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(by_object["filed"], json!(true));
        assert_eq!(
            String::from_utf8(store.read("notes/blob-object.txt").unwrap().bytes).unwrap(),
            "from blob ref"
        );

        let by_string = host
            .invoke(
                "drive.put",
                json!({
                    "content": blob_uri,
                    "options": { "suggestedPath": "notes/blob-string.txt" }
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(by_string["filed"], json!(true));
        assert_eq!(
            String::from_utf8(store.read("notes/blob-string.txt").unwrap().bytes).unwrap(),
            "from blob ref"
        );

        let err = host
            .invoke(
                "drive.put",
                json!({
                    "content": { "uri": "artifact://0" },
                    "options": { "suggestedPath": "notes/artifact-pointer.txt" }
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        let HostError::InvalidArgs(message) = err else {
            panic!("expected invalid args for unsupported uri ref, got {err:?}");
        };
        assert!(message.contains("content.uri only supports blob:sha256"));
        assert!(store.get("notes/artifact-pointer.txt").is_err());
    }

    #[test]
    fn trusted_direct_auto_put_can_file_without_host_approval() {
        let (_dir, store) = store();
        let result = store
            .put_bytes(
                b"trusted server import",
                DrivePutOptions {
                    auto: true,
                    approval_mode: crate::DriveApprovalMode::Auto,
                    suggested_path: Some("imports/trusted.txt".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        assert_eq!(result.entry.path, "imports/trusted.txt");
        assert!(store.get("imports/trusted.txt").is_ok());
    }

    #[tokio::test]
    async fn dropped_file_auto_put_records_approval_provenance_and_replay_event() {
        let (_dir, store) = store();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store.clone(), None);
        let events = Arc::new(RecordingHostEventSink::default());
        let ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow("drive.put"),
            Arc::new(StaticApproval(ApprovalDecision::Approved)),
            std::time::Duration::from_secs(1),
        )
        .with_session_id("session-drop")
        .with_event_sink(events.clone());

        let result = host
            .invoke(
                "drive.put",
                json!({
                    "content": "# Dropped Brief\nfile this approved drop",
                    "options": {
                        "auto": true,
                        "suggestedPath": "inbox/drop.md",
                        "project": "TempestMiku",
                        "docKind": "note",
                        "sourceUri": "drop://browser/drop.md",
                        "eventSeq": 17
                    }
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result["filed"], json!(true));
        assert_eq!(result["entry"]["uri"], json!("drive://inbox/drop.md"));
        assert_eq!(
            result["entry"]["sourceUri"],
            json!("drop://browser/drop.md")
        );
        assert_eq!(
            result["entry"]["provenance"][0]["sourceUri"],
            json!("drop://browser/drop.md")
        );
        assert_eq!(
            result["entry"]["provenance"][0]["sessionId"],
            json!("session-drop")
        );
        assert_eq!(result["entry"]["provenance"][0]["eventSeq"], json!(17));
        assert_eq!(
            result["entry"]["provenance"][0]["contentHash"],
            result["entry"]["contentHash"]
        );

        let events = events.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "drive_put");
        assert_eq!(events[0].1["sourceUri"], json!("drop://browser/drop.md"));
        assert_eq!(events[0].1["uri"], json!("drive://inbox/drop.md"));
        assert_eq!(
            events[0].1["preview"]["title"],
            json!("Filed drive document")
        );
        assert_eq!(
            events[0].1["resourceRefs"][0]["uri"],
            json!("drive://inbox/drop.md")
        );
    }

    #[tokio::test]
    async fn drive_link_registers_shared_policy_only_after_approval() {
        let (_dir, store) = store();
        let project = tempfile::tempdir().unwrap();
        let linked = LinkedFolders::default();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(
            &mut host,
            &mut resources,
            store.clone(),
            Some(linked.clone()),
        );

        let timeout_ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow_many(["drive.link", "drive.unlink"]),
            Arc::new(DefaultDenyApprovalPolicy),
            std::time::Duration::from_millis(1),
        );
        let err = host
            .invoke(
                "drive.link",
                json!({
                    "hostPath": project.path(),
                    "mode": "ro",
                    "project": "Tempest Miku"
                }),
                &timeout_ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::ApprovalTimeout(_)));
        assert!(linked.policy("tempest-miku").is_err());

        let approved_ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow_many(["drive.link", "drive.unlink"]),
            Arc::new(StaticApproval(ApprovalDecision::Approved)),
            std::time::Duration::from_secs(1),
        );
        let linked_value = host
            .invoke(
                "drive.link",
                json!({
                    "hostPath": project.path(),
                    "mode": "rw",
                    "project": "Tempest Miku"
                }),
                &approved_ctx,
            )
            .await
            .unwrap();
        assert_eq!(linked_value["linkedUri"], json!("linked://tempest-miku/"));
        let policy = linked.policy("tempest-miku").unwrap();
        assert_eq!(policy.root, project.path().canonicalize().unwrap());
        assert_eq!(policy.mode, tm_host::FsMode::Rw);

        host.invoke(
            "drive.link",
            json!({
                "hostPath": project.path(),
                "mode": "ro",
                "project": "Tempest Miku"
            }),
            &approved_ctx,
        )
        .await
        .unwrap();
        let policy = linked.policy("tempest-miku").unwrap();
        assert_eq!(policy.mode, tm_host::FsMode::Ro);

        let denied_ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow_many(["drive.link", "drive.unlink"]),
            Arc::new(StaticApproval(ApprovalDecision::Denied)),
            std::time::Duration::from_secs(1),
        );
        let other = tempfile::tempdir().unwrap();
        let err = host
            .invoke(
                "drive.link",
                json!({
                    "hostPath": other.path(),
                    "mode": "ro",
                    "project": "Blocked Project"
                }),
                &denied_ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::ApprovalDenied(_)));
        assert!(linked.policy("blocked-project").is_err());

        let err = host
            .invoke(
                "drive.unlink",
                json!({ "alias": "linked://tempest-miku/" }),
                &denied_ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::ApprovalDenied(_)));
        assert!(linked.policy("tempest-miku").is_ok());

        let revoked = host
            .invoke(
                "drive.unlink",
                json!({ "alias": "linked://tempest-miku/" }),
                &approved_ctx,
            )
            .await
            .unwrap();
        assert_eq!(revoked["linkedUri"], json!("linked://tempest-miku/"));
        assert_eq!(revoked["memoryScope"], json!("project:tempest-miku"));
        assert!(linked.policy("tempest-miku").is_err());
    }

    #[tokio::test]
    async fn drive_mutations_emit_mobile_friendly_replay_events() {
        let (_dir, store) = store();
        let project = tempfile::tempdir().unwrap();
        let linked = LinkedFolders::default();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store, Some(linked.clone()));
        let events = Arc::new(RecordingHostEventSink::default());
        let ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow_many([
                "drive.put",
                "drive.move",
                "drive.tag",
                "drive.link",
                "drive.unlink",
            ]),
            Arc::new(StaticApproval(ApprovalDecision::Approved)),
            std::time::Duration::from_secs(1),
        )
        .with_event_sink(events.clone());

        host.invoke(
            "drive.put",
            json!({
                "content": "# Raw\nship the mobile event payloads",
                "options": {
                    "suggestedPath": "inbox/raw.md",
                    "project": "TempestMiku",
                    "docKind": "note",
                    "tags": ["planning"],
                    "sourceUri": "drop://raw.md"
                }
            }),
            &ctx,
        )
        .await
        .unwrap();
        host.invoke(
            "drive.move",
            json!({
                "from": "inbox/raw.md",
                "to": "projects/tempestmiku/note/raw.md"
            }),
            &ctx,
        )
        .await
        .unwrap();
        host.invoke(
            "drive.tag",
            json!({
                "path": "projects/tempestmiku/note/raw.md",
                "tags": ["review"]
            }),
            &ctx,
        )
        .await
        .unwrap();
        host.invoke(
            "drive.link",
            json!({
                "hostPath": project.path(),
                "mode": "ro",
                "project": "Tempest Miku"
            }),
            &ctx,
        )
        .await
        .unwrap();
        host.invoke("drive.unlink", json!({ "alias": "tempest-miku" }), &ctx)
            .await
            .unwrap();

        let events = events.events();
        let event_types = events
            .iter()
            .map(|(event_type, _)| event_type.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            event_types,
            vec![
                "drive_put",
                "drive_moved",
                "drive_tagged",
                "drive_linked",
                "drive_unlinked"
            ]
        );
        assert_eq!(events[0].1["uri"], json!("drive://inbox/raw.md"));
        assert_eq!(
            events[0].1["preview"]["title"],
            json!("Filed drive document")
        );
        assert_eq!(
            events[0].1["resourceRefs"][0]["uri"],
            json!("drive://inbox/raw.md")
        );
        assert_eq!(events[1].1["fromUri"], json!("drive://inbox/raw.md"));
        assert_eq!(
            events[1].1["toUri"],
            json!("drive://projects/tempestmiku/note/raw.md")
        );
        assert_eq!(events[1].1["resourceRefs"][1]["role"], json!("current"));
        assert_eq!(events[2].1["tags"], json!(["note", "planning", "review"]));
        assert_eq!(
            events[2].1["preview"]["subtitle"],
            json!("projects/tempestmiku/note/raw.md")
        );
        assert_eq!(events[3].1["linkedUri"], json!("linked://tempest-miku/"));
        assert_eq!(
            events[3].1["resourceRefs"][0]["kind"],
            json!("linked_folder")
        );
        assert_eq!(events[4].1["linkedUri"], json!("linked://tempest-miku/"));
        assert_eq!(
            events[4].1["preview"]["title"],
            json!("Unlinked project folder")
        );
    }

    #[tokio::test]
    async fn drive_organize_apply_is_approval_gated_and_updates_status() {
        let (_dir, store) = store();
        store
            .put_bytes(
                b"# Raw\norganizer should move this into project notes",
                DrivePutOptions {
                    suggested_path: Some("inbox/raw.md".to_string()),
                    project: Some("TempestMiku".to_string()),
                    doc_kind: Some("note".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store.clone(), None);

        let denied_ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow("drive.organize"),
            Arc::new(StaticApproval(ApprovalDecision::Denied)),
            std::time::Duration::from_secs(1),
        );
        let err = host
            .invoke("drive.organize", json!({ "apply": true }), &denied_ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::ApprovalDenied(_)));
        let proposals = store.proposals();
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].status, ProposalStatus::Denied);
        assert!(store.get("inbox/raw.md").is_ok());
        assert!(
            store
                .get(proposals[0].proposed_path.as_deref().unwrap())
                .is_err()
        );

        let approved_ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow("drive.organize"),
            Arc::new(StaticApproval(ApprovalDecision::Approved)),
            std::time::Duration::from_secs(1),
        );
        let applied = host
            .invoke("drive.organize", json!({ "apply": true }), &approved_ctx)
            .await
            .unwrap();
        assert_eq!(applied.as_array().unwrap().len(), 1);
        assert_eq!(applied[0]["status"], json!("applied"));
        let proposed_path = applied[0]["proposedPath"].as_str().unwrap();
        assert!(store.get("inbox/raw.md").is_err());
        assert_eq!(store.get(proposed_path).unwrap().path, proposed_path);
    }

    #[tokio::test]
    async fn host_drive_organize_rejects_auto_apply_config() {
        let (_dir, store) = store();
        store
            .put_bytes(
                b"# Raw\norganizer should move this into project notes",
                DrivePutOptions {
                    suggested_path: Some("inbox/raw.md".to_string()),
                    project: Some("TempestMiku".to_string()),
                    doc_kind: Some("note".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store.clone(), None);
        let ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow("drive.organize"),
            Arc::new(DefaultDenyApprovalPolicy),
            std::time::Duration::from_millis(1),
        );

        let err = host
            .invoke(
                "drive.organize",
                json!({
                    "config": {
                        "tier": "conservative",
                        "autoApply": [{
                            "actions": ["move"],
                            "docKinds": ["note"],
                            "projects": ["TempestMiku"],
                            "minConfidence": 0.7
                        }]
                    }
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        let HostError::InvalidArgs(message) = err else {
            panic!("expected invalid args for host autoApply config, got {err:?}");
        };
        assert!(message.contains("autoApply"));
        assert!(store.get("inbox/raw.md").is_ok());
        assert!(store.proposals().is_empty());

        let err = host
            .invoke(
                "drive.organize",
                json!({ "config": { "tier": "moderate" } }),
                &ctx,
            )
            .await
            .unwrap_err();
        let HostError::InvalidArgs(message) = err else {
            panic!("expected invalid args for host non-conservative tier, got {err:?}");
        };
        assert!(message.contains("conservative tier"));
        assert!(store.get("inbox/raw.md").is_ok());
        assert!(store.proposals().is_empty());
    }

    #[test]
    fn trusted_store_drive_organize_auto_apply_is_tier_and_rule_gated() {
        let (_dir, conservative_store) = store();
        conservative_store
            .put_bytes(
                b"# Raw\norganizer should move this into project notes",
                DrivePutOptions {
                    suggested_path: Some("inbox/raw.md".to_string()),
                    project: Some("TempestMiku".to_string()),
                    doc_kind: Some("note".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let conservative = conservative_store
            .organize_with_config(DriveOrganizerConfig {
                tier: DriveAutomationTier::Conservative,
                auto_apply: vec![crate::DriveOrganizerAutoApplyRule {
                    actions: vec![OrganizerActionKind::Move],
                    doc_kinds: vec!["note".to_string()],
                    projects: vec!["TempestMiku".to_string()],
                    min_confidence: 0.7,
                }],
            })
            .unwrap();
        assert_eq!(conservative[0].status, ProposalStatus::Pending);
        assert_eq!(
            conservative[0].policy_decision,
            PolicyDecision::ApprovalRequired
        );
        assert!(conservative_store.get("inbox/raw.md").is_ok());
        assert!(
            conservative_store
                .get(conservative[0].proposed_path.as_deref().unwrap())
                .is_err()
        );

        let (_dir, moderate_store) = store();
        moderate_store
            .put_bytes(
                b"# Raw\norganizer should move this into project notes",
                DrivePutOptions {
                    suggested_path: Some("inbox/raw.md".to_string()),
                    project: Some("TempestMiku".to_string()),
                    doc_kind: Some("note".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let applied = moderate_store
            .organize_with_config(DriveOrganizerConfig {
                tier: DriveAutomationTier::Moderate,
                auto_apply: vec![crate::DriveOrganizerAutoApplyRule {
                    actions: vec![OrganizerActionKind::Move],
                    doc_kinds: vec!["note".to_string()],
                    projects: vec!["TempestMiku".to_string()],
                    min_confidence: 0.7,
                }],
            })
            .unwrap();
        assert_eq!(applied[0].status, ProposalStatus::Applied);
        assert_eq!(applied[0].policy_decision, PolicyDecision::AutoApply);
        let proposed_path = applied[0].proposed_path.as_deref().unwrap();
        assert!(moderate_store.get("inbox/raw.md").is_err());
        assert_eq!(
            moderate_store.get(proposed_path).unwrap().path,
            proposed_path
        );

        let (_dir, strict_store) = store();
        strict_store
            .put_bytes(
                b"# Raw\norganizer should move this into project notes",
                DrivePutOptions {
                    suggested_path: Some("inbox/raw.md".to_string()),
                    project: Some("TempestMiku".to_string()),
                    doc_kind: Some("note".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let strict = strict_store
            .organize_with_config(DriveOrganizerConfig {
                tier: DriveAutomationTier::Moderate,
                auto_apply: vec![crate::DriveOrganizerAutoApplyRule {
                    actions: vec![OrganizerActionKind::Move],
                    doc_kinds: vec!["note".to_string()],
                    projects: vec!["TempestMiku".to_string()],
                    min_confidence: 0.95,
                }],
            })
            .unwrap();
        assert_eq!(strict[0].status, ProposalStatus::Pending);
        assert_eq!(strict[0].policy_decision, PolicyDecision::ApprovalRequired);
        assert!(strict_store.get("inbox/raw.md").is_ok());
    }

    #[tokio::test]
    async fn drive_organize_emits_replayable_events_with_resource_refs() {
        let (_dir, store) = store();
        store
            .put_bytes(
                b"# Raw\norganizer should move this into project notes",
                DrivePutOptions {
                    suggested_path: Some("inbox/raw.md".to_string()),
                    project: Some("TempestMiku".to_string()),
                    doc_kind: Some("note".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store.clone(), None);
        let events = Arc::new(RecordingHostEventSink::default());
        let ctx = InvocationCtx::new(CapabilityGrants::default().allow("drive.organize"))
            .with_event_sink(events.clone());

        let proposals = host
            .invoke("drive.organize", json!({}), &ctx)
            .await
            .unwrap();
        assert_eq!(proposals.as_array().unwrap().len(), 1);

        let events = events.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].0, "drive_organizer_started");
        assert_eq!(events[0].1["apply"], json!(false));
        assert_eq!(events[0].1["tier"], json!("conservative"));
        assert_eq!(events[1].0, "write_proposal");
        assert_eq!(events[1].1["kind"], json!("drive"));
        assert_eq!(events[1].1["status"], json!("pending"));
        assert_eq!(events[1].1["sourceUri"], json!("drive://inbox/raw.md"));
        assert_eq!(
            events[1].1["proposedUri"],
            json!("drive://projects/tempestmiku/note/raw.md")
        );
        assert_eq!(events[2].0, "drive_organizer_completed");
        assert_eq!(events[2].1["proposalCount"], json!(1));
        assert_eq!(events[2].1["proposals"][0]["status"], json!("pending"));
        assert_eq!(
            events[2].1["proposals"][0]["sourceUri"],
            json!("drive://inbox/raw.md")
        );
        assert_eq!(
            events[2].1["proposals"][0]["proposedUri"],
            json!("drive://projects/tempestmiku/note/raw.md")
        );
        assert_eq!(
            events[2].1["proposals"][0]["resourceRefs"][0]["role"],
            json!("source")
        );
        assert_eq!(
            events[2].1["proposals"][0]["resourceRefs"][1]["role"],
            json!("proposed")
        );
        assert!(
            events[2].1["proposals"][0]["preview"]["subtitle"]
                .as_str()
                .unwrap()
                .contains("inbox/raw.md -> projects/tempestmiku/note/raw.md")
        );
        assert_eq!(
            events[2].1["resourceRefs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value["uri"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "drive://inbox/raw.md",
                "drive://projects/tempestmiku/note/raw.md"
            ]
        );
    }

    #[tokio::test]
    async fn drive_organize_apply_marks_stale_sources_without_mutation() {
        let (_dir, store) = store();
        store
            .put_bytes(
                b"# Raw\norganizer should move this into project notes",
                DrivePutOptions {
                    suggested_path: Some("inbox/raw.md".to_string()),
                    project: Some("TempestMiku".to_string()),
                    doc_kind: Some("note".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let proposal = store.organize().unwrap().remove(0);
        let proposed_path = proposal.proposed_path.clone().unwrap();
        store
            .move_entry(
                "inbox/raw.md",
                "manual/raw.md",
                DriveCollisionStrategy::Reject,
            )
            .unwrap();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store.clone(), None);
        let approved_ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow("drive.organize"),
            Arc::new(StaticApproval(ApprovalDecision::Approved)),
            std::time::Duration::from_secs(1),
        );

        let applied = host
            .invoke("drive.organize", json!({ "apply": true }), &approved_ctx)
            .await
            .unwrap();
        assert_eq!(applied.as_array().unwrap().len(), 1);
        assert_eq!(applied[0]["id"], json!(proposal.id));
        assert_eq!(applied[0]["status"], json!("stale"));
        assert!(store.get("manual/raw.md").is_ok());
        assert!(store.get(&proposed_path).is_err());
    }

    #[tokio::test]
    async fn drive_organize_apply_timeout_marks_failed_without_mutation() {
        let (_dir, store) = store();
        store
            .put_bytes(
                b"# Raw Note\norganizer should move this into project notes",
                DrivePutOptions {
                    suggested_path: Some("inbox/raw.md".to_string()),
                    project: Some("TempestMiku".to_string()),
                    doc_kind: Some("note".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store.clone(), None);
        let timeout_ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow("drive.organize"),
            Arc::new(DefaultDenyApprovalPolicy),
            std::time::Duration::from_millis(1),
        );

        let err = host
            .invoke("drive.organize", json!({ "apply": true }), &timeout_ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::ApprovalTimeout(_)));
        let proposals = store.proposals();
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].status, ProposalStatus::Failed);
        assert!(store.get("inbox/raw.md").is_ok());
        assert!(
            store
                .get(proposals[0].proposed_path.as_deref().unwrap())
                .is_err()
        );
    }

    #[tokio::test]
    async fn drive_organize_apply_collision_fails_without_partial_metadata_write() {
        let (_dir, store) = store();
        store
            .put_bytes(
                b"# Raw\norganizer should move this into project notes",
                DrivePutOptions {
                    suggested_path: Some("inbox/raw.md".to_string()),
                    project: Some("TempestMiku".to_string()),
                    doc_kind: Some("note".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        store
            .put_bytes(
                b"# Existing Note\nalready owns the proposed path",
                DrivePutOptions {
                    suggested_path: Some("projects/tempestmiku/note/raw.md".to_string()),
                    project: Some("TempestMiku".to_string()),
                    doc_kind: Some("note".to_string()),
                    title: Some("Raw".to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store.clone(), None);
        let approved_ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow("drive.organize"),
            Arc::new(StaticApproval(ApprovalDecision::Approved)),
            std::time::Duration::from_secs(1),
        );

        let applied = host
            .invoke("drive.organize", json!({ "apply": true }), &approved_ctx)
            .await
            .unwrap();
        assert_eq!(applied.as_array().unwrap().len(), 1);
        assert_eq!(applied[0]["status"], json!("failed"));
        assert_eq!(store.get("inbox/raw.md").unwrap().path, "inbox/raw.md");
        let existing = store.read("projects/tempestmiku/note/raw.md").unwrap();
        assert_eq!(
            String::from_utf8(existing.bytes).unwrap(),
            "# Existing Note\nalready owns the proposed path"
        );
    }

    #[tokio::test]
    async fn mutating_approval_timeout_writes_nothing() {
        let (_dir, store) = store();
        let mut host = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_drive_functions(&mut host, &mut resources, store.clone(), None);
        let ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow_many(["drive.put", "drive.move"]),
            Arc::new(DefaultDenyApprovalPolicy),
            std::time::Duration::from_millis(1),
        );
        host.invoke(
            "drive.put",
            json!({
                "content": "hello",
                "options": { "suggestedPath": "notes/a.txt" }
            }),
            &ctx,
        )
        .await
        .unwrap();
        let err = host
            .invoke(
                "drive.move",
                json!({ "from": "notes/a.txt", "to": "notes/b.txt" }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HostError::ApprovalTimeout(_)));
        assert!(store.get("notes/a.txt").is_ok());
        assert!(store.get("notes/b.txt").is_err());
    }
}

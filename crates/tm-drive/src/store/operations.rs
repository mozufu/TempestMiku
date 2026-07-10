use std::{collections::BTreeMap, fmt::Debug, sync::Arc};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use tm_artifacts::ResourceContent;
use tm_host::ResourceEntry;
use uuid::Uuid;

use super::{
    core::{
        normalize_canonical_path_path_or_uri, provenance, sanitize_drive_bytes,
        sanitize_drive_put_options, unique_path, validate_drive_identifier, write_proposal,
    },
    metadata::{
        DriveEntryUpdate, DriveMetadataStore, DriveMoveCommit, DriveOverwriteTarget,
        OrganizerProposalCommit,
    },
    types::{DriveService, InMemoryDriveMetadataStore},
};
use crate::{
    DriveCollisionStrategy, DriveCorrectionRecord, DriveDedupeMode, DriveEntry, DriveEntryStatus,
    DriveLinkPlan, DriveLinkRecord, DriveLinkStatus, DriveListOptions, DriveOrganizerConfig,
    DrivePutOptions, DrivePutResult, DriveSearchOptions, DriveSearchResult, OrganizerActionKind,
    OrganizerProposal, OrganizerRun, OrganizerRunStatus, PolicyDecision, ProposalStatus,
    generate_organizer_proposals_for_run, initial_record_version,
};

#[async_trait]
pub trait DriveOperations: Debug + Send + Sync {
    async fn plan_put_path(&self, bytes: &[u8], options: &DrivePutOptions)
    -> crate::Result<String>;
    async fn put_bytes(
        &self,
        bytes: &[u8],
        options: DrivePutOptions,
    ) -> crate::Result<DrivePutResult>;
    async fn read_blob(&self, uri: &str) -> crate::Result<Vec<u8>>;
    async fn resource_content(
        &self,
        uri: &str,
        selector: Option<&str>,
    ) -> crate::Result<ResourceContent>;
    async fn list(&self, options: DriveListOptions) -> crate::Result<Vec<DriveEntry>>;
    async fn search(&self, options: DriveSearchOptions) -> crate::Result<Vec<DriveSearchResult>>;
    async fn move_entry(
        &self,
        from: &str,
        to: &str,
        collision: DriveCollisionStrategy,
    ) -> crate::Result<DriveEntry>;
    async fn tag_entry(&self, path_or_uri: &str, tags: Vec<String>) -> crate::Result<DriveEntry>;
    async fn organize_with_config(
        &self,
        config: DriveOrganizerConfig,
    ) -> crate::Result<Vec<OrganizerProposal>>;
    async fn organize_scoped_with_config(
        &self,
        project: Option<&str>,
        config: DriveOrganizerConfig,
    ) -> crate::Result<Vec<OrganizerProposal>>;
    async fn pending_proposal_ids(&self) -> crate::Result<Vec<Uuid>>;
    async fn mark_proposals_status(
        &self,
        ids: &[Uuid],
        status: ProposalStatus,
    ) -> crate::Result<()>;
    async fn apply_organizer_proposals(
        &self,
        ids: &[Uuid],
    ) -> crate::Result<Vec<OrganizerProposal>>;
    async fn proposals(&self) -> crate::Result<Vec<OrganizerProposal>>;
    async fn resource_entries(&self, uri: Option<&str>) -> crate::Result<Vec<ResourceEntry>>;
    async fn record_link(&self, plan: &DriveLinkPlan) -> crate::Result<DriveLinkRecord>;
    async fn revoke_link(&self, alias: &str) -> crate::Result<Option<DriveLinkRecord>>;
    async fn invalidate_link(&self, alias: &str, reason: &str) -> crate::Result<DriveLinkRecord>;
    async fn links(&self) -> crate::Result<Vec<DriveLinkRecord>>;
}

pub type SharedDriveStore = Arc<dyn DriveOperations>;

pub trait IntoSharedDriveStore {
    fn into_shared_drive_store(self) -> SharedDriveStore;
}

impl<M> IntoSharedDriveStore for DriveService<M>
where
    M: DriveMetadataStore + 'static,
{
    fn into_shared_drive_store(self) -> SharedDriveStore {
        Arc::new(self)
    }
}

impl IntoSharedDriveStore for SharedDriveStore {
    fn into_shared_drive_store(self) -> SharedDriveStore {
        self
    }
}

#[async_trait]
impl<M> DriveOperations for DriveService<M>
where
    M: DriveMetadataStore + 'static,
{
    async fn plan_put_path(
        &self,
        bytes: &[u8],
        options: &DrivePutOptions,
    ) -> crate::Result<String> {
        let options = sanitize_drive_put_options(options.clone())?;
        let bytes = sanitize_drive_bytes(bytes)?;
        Ok(self
            .mirror()
            .await?
            .plan_put_bytes(bytes.as_ref(), &options)?
            .path)
    }

    async fn put_bytes(
        &self,
        bytes: &[u8],
        options: DrivePutOptions,
    ) -> crate::Result<DrivePutResult> {
        let options = sanitize_drive_put_options(options)?;
        let bytes = sanitize_drive_bytes(bytes)?;
        let bytes = bytes.as_ref();
        let plan = self.mirror().await?.plan_put_bytes(bytes, &options)?;
        let blob_uri = self
            .artifacts
            .put_blob(bytes)
            .map_err(|err| crate::DriveError::Store(err.to_string()))?;
        let transduction = plan.transduction;
        let proposed_path = plan.proposed_path;
        let mut path = plan.path;
        let now = plan.now;

        if let Some(existing) = self.metadata.entry_by_path(&path).await? {
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
                let mut replacement = existing.clone();
                replacement.blob_uri = blob_uri;
                replacement.content_hash = transduction.content_hash.clone();
                replacement.mime = transduction.mime.clone();
                replacement.size_bytes = bytes.len();
                replacement.title = transduction.title.clone();
                replacement.doc_kind = transduction.doc_kind.clone();
                replacement.project = transduction.project.clone();
                replacement.entities = transduction.entities.clone();
                replacement.dates = transduction.dates.clone();
                replacement.amounts = transduction.amounts.clone();
                replacement.tags = transduction.tags.clone();
                replacement.source_uri = options.source_uri.clone();
                replacement.attributes = transduction.attributes.clone();
                replacement.summary = transduction.summary.clone();
                replacement.updated_at = now;
                replacement.provenance.push(provenance(
                    &options,
                    &transduction.content_hash,
                    &transduction.extractor,
                    now,
                ));
                let entry = self
                    .metadata
                    .compare_and_swap_entry(existing.id, existing.version, replacement)
                    .await?;
                return Ok(DrivePutResult {
                    uri: entry.uri.clone(),
                    proposed_path,
                    filed: true,
                    proposal: None,
                    entry,
                });
            }
            if options.collision == DriveCollisionStrategy::Reject {
                return Err(crate::DriveError::Collision(path));
            }
            path = unique_path_for_entries(&self.metadata.entries().await?, &path);
        }

        loop {
            let entry = DriveEntry {
                id: Uuid::new_v4(),
                version: initial_record_version(),
                path: path.clone(),
                uri: DriveEntry::drive_uri(&path),
                blob_uri: blob_uri.clone(),
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
                attributes: transduction.attributes.clone(),
                summary: transduction.summary.clone(),
            };
            match self.metadata.insert_entry(entry).await {
                Ok(entry) => {
                    let proposal = if options.auto
                        && options.approval_mode == crate::DriveApprovalMode::Propose
                    {
                        Some(
                            self.metadata
                                .insert_proposal(write_proposal(&entry, proposed_path.clone(), now))
                                .await?,
                        )
                    } else {
                        None
                    };
                    return Ok(DrivePutResult {
                        uri: entry.uri.clone(),
                        proposed_path,
                        filed: true,
                        proposal,
                        entry,
                    });
                }
                Err(crate::DriveError::Collision(_))
                    if options.collision == DriveCollisionStrategy::KeepBoth =>
                {
                    path = unique_path_for_entries(&self.metadata.entries().await?, &path);
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn read_blob(&self, uri: &str) -> crate::Result<Vec<u8>> {
        self.artifacts
            .read_blob(uri)
            .map_err(|err| crate::DriveError::NotFound(err.to_string()))
    }

    async fn resource_content(
        &self,
        uri: &str,
        selector: Option<&str>,
    ) -> crate::Result<ResourceContent> {
        self.mirror().await?.resource_content(uri, selector)
    }

    async fn list(&self, options: DriveListOptions) -> crate::Result<Vec<DriveEntry>> {
        self.mirror().await?.list(options)
    }

    async fn search(&self, options: DriveSearchOptions) -> crate::Result<Vec<DriveSearchResult>> {
        self.mirror().await?.search(options)
    }

    async fn move_entry(
        &self,
        from: &str,
        to: &str,
        collision: DriveCollisionStrategy,
    ) -> crate::Result<DriveEntry> {
        validate_drive_identifier("move source", from)?;
        validate_drive_identifier("move destination", to)?;
        let from = normalize_canonical_path_path_or_uri(from)?;
        let mut to = normalize_canonical_path_path_or_uri(to)?;
        let current = self
            .metadata
            .entry_by_path(&from)
            .await?
            .ok_or_else(|| crate::DriveError::NotFound(from.clone()))?;
        let mut overwrite = None;
        if let Some(target) = self.metadata.entry_by_path(&to).await?
            && target.id != current.id
        {
            match collision {
                DriveCollisionStrategy::Reject => {
                    return Err(crate::DriveError::Collision(to));
                }
                DriveCollisionStrategy::Overwrite => {
                    overwrite = Some(DriveOverwriteTarget {
                        id: target.id,
                        expected_version: target.version,
                    });
                }
                DriveCollisionStrategy::KeepBoth => {
                    to = unique_path_for_entries(&self.metadata.entries().await?, &to);
                }
            }
        }
        let now = Utc::now();
        let mut replacement = current.clone();
        replacement.path = to.clone();
        replacement.uri = DriveEntry::drive_uri(&to);
        replacement.updated_at = now;
        self.metadata
            .commit_move(DriveMoveCommit {
                source: DriveEntryUpdate {
                    expected_version: current.version,
                    replacement,
                },
                overwrite,
                correction: DriveCorrectionRecord {
                    id: Uuid::new_v4(),
                    version: initial_record_version(),
                    from,
                    to,
                    created_at: now,
                },
            })
            .await
    }

    async fn tag_entry(&self, path_or_uri: &str, tags: Vec<String>) -> crate::Result<DriveEntry> {
        validate_drive_identifier("tag path", path_or_uri)?;
        for tag in &tags {
            validate_drive_identifier("tag", tag)?;
        }
        let path = normalize_canonical_path_path_or_uri(path_or_uri)?;
        let current = self
            .metadata
            .entry_by_path(&path)
            .await?
            .ok_or_else(|| crate::DriveError::NotFound(path.clone()))?;
        let mut replacement = current.clone();
        replacement.tags = crate::apply_tags(&current.tags, &tags);
        replacement.updated_at = Utc::now();
        self.metadata
            .compare_and_swap_entry(current.id, current.version, replacement)
            .await
    }

    async fn organize_with_config(
        &self,
        config: DriveOrganizerConfig,
    ) -> crate::Result<Vec<OrganizerProposal>> {
        let now = Utc::now();
        let active = self
            .metadata
            .organizer_runs()
            .await?
            .into_iter()
            .find(|run| {
                matches!(
                    run.status,
                    OrganizerRunStatus::Queued | OrganizerRunStatus::Running
                )
            });
        if active.as_ref().is_some_and(|run| {
            run.status == OrganizerRunStatus::Running
                && run
                    .locked_at
                    .is_none_or(|locked_at| locked_at > now - Duration::seconds(30))
        }) {
            return Err(crate::DriveError::Store(
                "organizer worker already running".to_string(),
            ));
        }
        let queued = if let Some(run) = active {
            run
        } else {
            self.metadata
                .insert_organizer_run(OrganizerRun {
                    id: Uuid::new_v4(),
                    version: initial_record_version(),
                    trigger: "manual".to_string(),
                    status: OrganizerRunStatus::Queued,
                    attempts: 0,
                    proposal_ids: Vec::new(),
                    created_at: now,
                    available_at: now,
                    locked_at: None,
                    completed_at: None,
                    last_error: None,
                })
                .await?
        };
        if queued.status == OrganizerRunStatus::Queued && queued.available_at > now {
            return Err(crate::DriveError::Store(
                "organizer worker is not ready".to_string(),
            ));
        }
        let mut running = queued.clone();
        running.status = OrganizerRunStatus::Running;
        running.attempts = running.attempts.saturating_add(1);
        running.locked_at = Some(now);
        running.completed_at = None;
        running.last_error = None;
        let running = self
            .metadata
            .compare_and_swap_organizer_run(queued.id, queued.version, running)
            .await?;

        let generated =
            generate_organizer_proposals_for_run(&self.metadata.entries().await?, running.id);
        let mut proposals = Vec::with_capacity(generated.len());
        for proposal in generated {
            proposals.push(self.metadata.insert_proposal(proposal).await?);
        }

        let mut auto_apply_ids = Vec::new();
        for proposal in &mut proposals {
            if !super::core::organizer_auto_apply_allowed(proposal, &config) {
                continue;
            }
            let mut replacement = proposal.clone();
            replacement.policy_decision = PolicyDecision::AutoApply;
            *proposal = self
                .metadata
                .compare_and_swap_proposal(proposal.id, proposal.version, replacement)
                .await?;
            auto_apply_ids.push(proposal.id);
        }
        if !auto_apply_ids.is_empty() {
            for applied in self.apply_proposals(&auto_apply_ids).await? {
                if let Some(proposal) = proposals
                    .iter_mut()
                    .find(|proposal| proposal.id == applied.id)
                {
                    *proposal = applied;
                }
            }
        }

        let completed_at = Utc::now();
        let mut completed = running.clone();
        completed.status = OrganizerRunStatus::Completed;
        completed.proposal_ids = proposals.iter().map(|proposal| proposal.id).collect();
        completed.available_at = completed_at;
        completed.locked_at = None;
        completed.completed_at = Some(completed_at);
        completed.last_error = None;
        self.metadata
            .compare_and_swap_organizer_run(running.id, running.version, completed)
            .await?;
        Ok(proposals)
    }

    async fn organize_scoped_with_config(
        &self,
        project: Option<&str>,
        config: DriveOrganizerConfig,
    ) -> crate::Result<Vec<OrganizerProposal>> {
        let now = Utc::now();
        let active = self
            .metadata
            .organizer_runs()
            .await?
            .into_iter()
            .find(|run| {
                matches!(
                    run.status,
                    OrganizerRunStatus::Queued | OrganizerRunStatus::Running
                )
            });
        if active.as_ref().is_some_and(|run| {
            run.status == OrganizerRunStatus::Running
                && run
                    .locked_at
                    .is_none_or(|locked_at| locked_at > now - Duration::seconds(30))
        }) {
            return Err(crate::DriveError::Store(
                "organizer worker already running".to_string(),
            ));
        }
        let queued = if let Some(run) = active {
            run
        } else {
            self.metadata
                .insert_organizer_run(OrganizerRun {
                    id: Uuid::new_v4(),
                    version: initial_record_version(),
                    trigger: project.map_or_else(
                        || "manual:global".to_string(),
                        |project| format!("manual:project:{}", crate::slug(project)),
                    ),
                    status: OrganizerRunStatus::Queued,
                    attempts: 0,
                    proposal_ids: Vec::new(),
                    created_at: now,
                    available_at: now,
                    locked_at: None,
                    completed_at: None,
                    last_error: None,
                })
                .await?
        };
        if queued.status == OrganizerRunStatus::Queued && queued.available_at > now {
            return Err(crate::DriveError::Store(
                "organizer worker is not ready".to_string(),
            ));
        }
        let mut running = queued.clone();
        running.status = OrganizerRunStatus::Running;
        running.attempts = running.attempts.saturating_add(1);
        running.locked_at = Some(now);
        running.completed_at = None;
        running.last_error = None;
        let running = self
            .metadata
            .compare_and_swap_organizer_run(queued.id, queued.version, running)
            .await?;
        let project_slug = project.map(crate::slug);
        let entries = self
            .metadata
            .entries()
            .await?
            .into_iter()
            .filter(|entry| match project_slug.as_deref() {
                Some(project_slug) => entry
                    .project
                    .as_deref()
                    .is_some_and(|entry_project| crate::slug(entry_project) == project_slug),
                None => entry.project.is_none(),
            })
            .collect::<Vec<_>>();
        let generated = generate_organizer_proposals_for_run(&entries, running.id);
        let mut proposals = Vec::with_capacity(generated.len());
        for proposal in generated {
            proposals.push(self.metadata.insert_proposal(proposal).await?);
        }
        let mut auto_apply_ids = Vec::new();
        for proposal in &mut proposals {
            if !super::core::organizer_auto_apply_allowed(proposal, &config) {
                continue;
            }
            let mut replacement = proposal.clone();
            replacement.policy_decision = PolicyDecision::AutoApply;
            *proposal = self
                .metadata
                .compare_and_swap_proposal(proposal.id, proposal.version, replacement)
                .await?;
            auto_apply_ids.push(proposal.id);
        }
        if !auto_apply_ids.is_empty() {
            for applied in self.apply_proposals(&auto_apply_ids).await? {
                if let Some(proposal) = proposals
                    .iter_mut()
                    .find(|proposal| proposal.id == applied.id)
                {
                    *proposal = applied;
                }
            }
        }
        let completed_at = Utc::now();
        let mut completed = running.clone();
        completed.status = OrganizerRunStatus::Completed;
        completed.proposal_ids = proposals.iter().map(|proposal| proposal.id).collect();
        completed.available_at = completed_at;
        completed.locked_at = None;
        completed.completed_at = Some(completed_at);
        completed.last_error = None;
        self.metadata
            .compare_and_swap_organizer_run(running.id, running.version, completed)
            .await?;
        Ok(proposals)
    }

    async fn pending_proposal_ids(&self) -> crate::Result<Vec<Uuid>> {
        Ok(self
            .metadata
            .proposals()
            .await?
            .into_iter()
            .filter(|proposal| {
                matches!(
                    proposal.status,
                    ProposalStatus::Pending | ProposalStatus::Approved
                )
            })
            .map(|proposal| proposal.id)
            .collect())
    }

    async fn mark_proposals_status(
        &self,
        ids: &[Uuid],
        status: ProposalStatus,
    ) -> crate::Result<()> {
        let proposals = self
            .metadata
            .proposals()
            .await?
            .into_iter()
            .map(|proposal| (proposal.id, proposal))
            .collect::<BTreeMap<_, _>>();
        let now = Utc::now();
        for id in ids {
            let Some(current) = proposals.get(id) else {
                continue;
            };
            let mut replacement = current.clone();
            replacement.status = status.clone();
            replacement.updated_at = now;
            self.metadata
                .compare_and_swap_proposal(*id, current.version, replacement)
                .await?;
        }
        Ok(())
    }

    async fn apply_organizer_proposals(
        &self,
        ids: &[Uuid],
    ) -> crate::Result<Vec<OrganizerProposal>> {
        self.apply_proposals(ids).await
    }

    async fn proposals(&self) -> crate::Result<Vec<OrganizerProposal>> {
        self.metadata.proposals().await
    }

    async fn resource_entries(&self, uri: Option<&str>) -> crate::Result<Vec<ResourceEntry>> {
        self.mirror().await?.resource_entries(uri)
    }

    async fn record_link(&self, plan: &DriveLinkPlan) -> crate::Result<DriveLinkRecord> {
        for (field, value) in [
            ("link alias", plan.alias.as_str()),
            ("link canonical root", plan.canonical_root.as_str()),
            ("link URI", plan.linked_uri.as_str()),
            ("link memory scope", plan.memory_scope.as_str()),
            ("link project", plan.project.as_str()),
        ] {
            validate_drive_identifier(field, value)?;
        }
        let now = Utc::now();
        let mut replacement = DriveLinkRecord::from_plan(plan, now);
        if let Some(current) = self.metadata.link(&plan.alias).await? {
            replacement.created_at = current.created_at;
            self.metadata
                .compare_and_swap_link(&plan.alias, current.version, replacement)
                .await
        } else {
            self.metadata.insert_link(replacement).await
        }
    }

    async fn revoke_link(&self, alias: &str) -> crate::Result<Option<DriveLinkRecord>> {
        let Some(current) = self.metadata.link(alias).await? else {
            return Ok(None);
        };
        if current.status == DriveLinkStatus::Revoked {
            return Ok(Some(current));
        }
        let now = Utc::now();
        let mut replacement = current.clone();
        replacement.status = DriveLinkStatus::Revoked;
        replacement.updated_at = now;
        replacement.revoked_at = Some(now);
        self.metadata
            .compare_and_swap_link(alias, current.version, replacement)
            .await
            .map(Some)
    }

    async fn invalidate_link(&self, alias: &str, reason: &str) -> crate::Result<DriveLinkRecord> {
        validate_drive_identifier("link alias", alias)?;
        let reason = tm_memory::redact_dream_text(reason).text;
        let current = self
            .metadata
            .link(alias)
            .await?
            .ok_or_else(|| crate::DriveError::NotFound(format!("drive link {alias}")))?;
        if current.status != DriveLinkStatus::Active {
            return Ok(current);
        }
        let mut replacement = current.clone();
        replacement.status = DriveLinkStatus::Invalid;
        replacement.updated_at = Utc::now();
        replacement
            .metadata
            .insert("invalidReason".to_string(), reason.into());
        self.metadata
            .compare_and_swap_link(alias, current.version, replacement)
            .await
    }

    async fn links(&self) -> crate::Result<Vec<DriveLinkRecord>> {
        self.metadata.links().await
    }
}

impl<M> DriveService<M>
where
    M: DriveMetadataStore,
{
    async fn mirror(&self) -> crate::Result<DriveService<InMemoryDriveMetadataStore>> {
        DriveService::from_snapshot(self.artifacts.clone(), self.metadata.snapshot().await?)
    }

    async fn apply_proposals(&self, ids: &[Uuid]) -> crate::Result<Vec<OrganizerProposal>> {
        let proposals = self
            .metadata
            .proposals()
            .await?
            .into_iter()
            .map(|proposal| (proposal.id, proposal))
            .collect::<BTreeMap<_, _>>();
        let mut applied = Vec::new();
        for id in ids {
            let Some(current) = proposals.get(id).cloned() else {
                continue;
            };
            if !matches!(
                current.status,
                ProposalStatus::Pending | ProposalStatus::Approved
            ) {
                applied.push(current);
                continue;
            }
            let now = Utc::now();
            let entry = self.metadata.entry(current.entry_id).await?;
            let mut status = ProposalStatus::Stale;
            let mut entry_update = None;
            let mut correction = None;
            if let Some(entry) = entry
                && entry.path == current.source_path
                && current
                    .replay_metadata
                    .get("contentHash")
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(|expected| expected == entry.content_hash)
            {
                let mut replacement = entry.clone();
                status = match current.action.clone() {
                    OrganizerActionKind::Move => {
                        let target = current
                            .proposed_path
                            .as_deref()
                            .and_then(|target| normalize_canonical_path_path_or_uri(target).ok());
                        if let Some(target) = target {
                            if self
                                .metadata
                                .entry_by_path(&target)
                                .await?
                                .is_some_and(|other| other.id != entry.id)
                            {
                                ProposalStatus::Failed
                            } else {
                                let from = replacement.path.clone();
                                replacement.path = target.clone();
                                replacement.uri = DriveEntry::drive_uri(&target);
                                replacement.updated_at = now;
                                entry_update = Some(DriveEntryUpdate {
                                    expected_version: entry.version,
                                    replacement,
                                });
                                correction = Some(DriveCorrectionRecord {
                                    id: Uuid::new_v4(),
                                    version: initial_record_version(),
                                    from,
                                    to: target,
                                    created_at: now,
                                });
                                ProposalStatus::Applied
                            }
                        } else {
                            ProposalStatus::Failed
                        }
                    }
                    OrganizerActionKind::Tag => {
                        replacement.tags =
                            crate::apply_tags(&replacement.tags, &current.proposed_tags);
                        replacement.updated_at = now;
                        entry_update = Some(DriveEntryUpdate {
                            expected_version: entry.version,
                            replacement,
                        });
                        ProposalStatus::Applied
                    }
                    OrganizerActionKind::Archive => {
                        replacement.status = DriveEntryStatus::Archived;
                        replacement.updated_at = now;
                        entry_update = Some(DriveEntryUpdate {
                            expected_version: entry.version,
                            replacement,
                        });
                        ProposalStatus::Applied
                    }
                    OrganizerActionKind::SetDocKind => {
                        if let Some(kind) = current.proposed_doc_kind.clone() {
                            replacement.doc_kind = Some(kind);
                            replacement.updated_at = now;
                            entry_update = Some(DriveEntryUpdate {
                                expected_version: entry.version,
                                replacement,
                            });
                            ProposalStatus::Applied
                        } else {
                            ProposalStatus::Failed
                        }
                    }
                    OrganizerActionKind::SetProject => {
                        if let Some(project) = current.proposed_project.clone() {
                            replacement.project = Some(project);
                            replacement.updated_at = now;
                            entry_update = Some(DriveEntryUpdate {
                                expected_version: entry.version,
                                replacement,
                            });
                            ProposalStatus::Applied
                        } else {
                            ProposalStatus::Failed
                        }
                    }
                    OrganizerActionKind::Dedupe => ProposalStatus::Failed,
                };
            }
            applied.push(
                self.finish_proposal(current, status, now, entry_update, correction)
                    .await?,
            );
        }
        Ok(applied)
    }

    async fn finish_proposal(
        &self,
        current: OrganizerProposal,
        status: ProposalStatus,
        now: chrono::DateTime<Utc>,
        entry_update: Option<DriveEntryUpdate>,
        correction: Option<DriveCorrectionRecord>,
    ) -> crate::Result<OrganizerProposal> {
        let mut replacement = current.clone();
        replacement.status = status;
        replacement.updated_at = now;
        self.metadata
            .commit_organizer_proposal(OrganizerProposalCommit {
                expected_proposal_version: current.version,
                replacement,
                entry_update,
                correction,
            })
            .await
    }
}

fn unique_path_for_entries(entries: &[DriveEntry], path: &str) -> String {
    let paths = entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.id))
        .collect::<BTreeMap<_, _>>();
    unique_path(&paths, path)
}

use super::*;

impl DriveService<InMemoryDriveMetadataStore> {
    pub fn organize(&self) -> crate::Result<Vec<OrganizerProposal>> {
        let now = Utc::now();
        self.enqueue_organizer_run("manual", now);
        let run = self
            .claim_ready_organizer_run(now, Duration::seconds(30))?
            .ok_or_else(|| DriveError::Store("organizer worker already running".to_string()))?;
        let proposals = self.generate_organizer_proposals_for_run(run.id)?;
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

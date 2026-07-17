use super::*;

#[async_trait]
impl DriveMetadataStore for InMemoryDriveMetadataStore {
    async fn entry(&self, id: DriveEntryId) -> crate::Result<Option<DriveEntry>> {
        Ok(self.inner.lock().entries.get(&id).cloned())
    }

    async fn entry_by_path(&self, path: &str) -> crate::Result<Option<DriveEntry>> {
        let inner = self.inner.lock();
        Ok(inner
            .path_to_id
            .get(path)
            .and_then(|id| inner.entries.get(id))
            .cloned())
    }

    async fn entries(&self) -> crate::Result<Vec<DriveEntry>> {
        Ok(self.inner.lock().entries.values().cloned().collect())
    }

    async fn insert_entry(&self, mut entry: DriveEntry) -> crate::Result<DriveEntry> {
        entry.version = initial_record_version();
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.entries.get(&entry.id) {
            return Err(version_conflict("entry", entry.id, 0, existing.version));
        }
        if inner.path_to_id.contains_key(&entry.path) {
            return Err(DriveError::Collision(entry.path));
        }
        inner.path_to_id.insert(entry.path.clone(), entry.id);
        inner.entries.insert(entry.id, entry.clone());
        Ok(entry)
    }

    async fn compare_and_swap_entry(
        &self,
        id: DriveEntryId,
        expected_version: u64,
        mut replacement: DriveEntry,
    ) -> crate::Result<DriveEntry> {
        let mut inner = self.inner.lock();
        let current = inner
            .entries
            .get(&id)
            .cloned()
            .ok_or_else(|| DriveError::NotFound(format!("drive entry {id}")))?;
        require_version("entry", id, expected_version, current.version)?;
        if replacement.id != id {
            return Err(DriveError::InvalidArgs(format!(
                "replacement entry id {} does not match {id}",
                replacement.id
            )));
        }
        if inner
            .path_to_id
            .get(&replacement.path)
            .is_some_and(|other| *other != id)
        {
            return Err(DriveError::Collision(replacement.path));
        }
        replacement.version = next_version("entry", id, expected_version)?;
        if current.path != replacement.path {
            inner.path_to_id.remove(&current.path);
            inner.path_to_id.insert(replacement.path.clone(), id);
        }
        inner.entries.insert(id, replacement.clone());
        Ok(replacement)
    }

    async fn remove_entry(
        &self,
        id: DriveEntryId,
        expected_version: u64,
    ) -> crate::Result<DriveEntry> {
        let mut inner = self.inner.lock();
        let current = inner
            .entries
            .get(&id)
            .cloned()
            .ok_or_else(|| DriveError::NotFound(format!("drive entry {id}")))?;
        require_version("entry", id, expected_version, current.version)?;
        inner.entries.remove(&id);
        inner.path_to_id.remove(&current.path);
        Ok(current)
    }

    async fn commit_move(&self, commit: DriveMoveCommit) -> crate::Result<DriveEntry> {
        let mut inner = self.inner.lock();
        let source_id = commit.source.replacement.id;
        let current = inner
            .entries
            .get(&source_id)
            .cloned()
            .ok_or_else(|| DriveError::NotFound(format!("drive entry {source_id}")))?;
        require_version(
            "entry",
            source_id,
            commit.source.expected_version,
            current.version,
        )?;
        validate_move_correction(&current, &commit.source.replacement, &commit.correction)?;

        let overwritten = if let Some(target) = commit.overwrite {
            if target.id == source_id {
                return Err(DriveError::InvalidArgs(
                    "move overwrite target must differ from source".to_string(),
                ));
            }
            let overwritten = inner
                .entries
                .get(&target.id)
                .cloned()
                .ok_or_else(|| DriveError::NotFound(format!("drive entry {}", target.id)))?;
            require_version(
                "entry",
                target.id,
                target.expected_version,
                overwritten.version,
            )?;
            if overwritten.path != commit.source.replacement.path {
                return Err(DriveError::InvalidArgs(
                    "overwrite target no longer occupies the move destination".to_string(),
                ));
            }
            Some(overwritten)
        } else {
            None
        };
        if let Some(occupant) = inner.path_to_id.get(&commit.source.replacement.path)
            && *occupant != source_id
            && overwritten
                .as_ref()
                .is_none_or(|entry| entry.id != *occupant)
        {
            return Err(DriveError::Collision(
                commit.source.replacement.path.clone(),
            ));
        }
        reject_duplicate_correction(&inner, commit.correction.id)?;

        let mut replacement = commit.source.replacement;
        replacement.version = next_version("entry", source_id, commit.source.expected_version)?;
        let mut correction = commit.correction;
        correction.version = initial_record_version();

        if let Some(overwritten) = overwritten {
            inner.entries.remove(&overwritten.id);
            inner.path_to_id.remove(&overwritten.path);
        }
        if current.path != replacement.path {
            inner.path_to_id.remove(&current.path);
            inner.path_to_id.insert(replacement.path.clone(), source_id);
        }
        inner.entries.insert(source_id, replacement.clone());
        inner.corrections.push(correction);
        Ok(replacement)
    }

    async fn proposals(&self) -> crate::Result<Vec<OrganizerProposal>> {
        Ok(self.inner.lock().proposals.values().cloned().collect())
    }

    async fn insert_proposal(
        &self,
        mut proposal: OrganizerProposal,
    ) -> crate::Result<OrganizerProposal> {
        proposal.version = initial_record_version();
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.proposals.get(&proposal.id) {
            return Err(version_conflict(
                "proposal",
                proposal.id,
                0,
                existing.version,
            ));
        }
        inner.proposals.insert(proposal.id, proposal.clone());
        Ok(proposal)
    }

    async fn compare_and_swap_proposal(
        &self,
        id: Uuid,
        expected_version: u64,
        mut replacement: OrganizerProposal,
    ) -> crate::Result<OrganizerProposal> {
        let mut inner = self.inner.lock();
        let current = inner
            .proposals
            .get(&id)
            .ok_or_else(|| DriveError::NotFound(format!("organizer proposal {id}")))?;
        require_version("proposal", id, expected_version, current.version)?;
        if replacement.id != id {
            return Err(DriveError::InvalidArgs(format!(
                "replacement proposal id {} does not match {id}",
                replacement.id
            )));
        }
        replacement.version = next_version("proposal", id, expected_version)?;
        inner.proposals.insert(id, replacement.clone());
        Ok(replacement)
    }

    async fn commit_organizer_proposal(
        &self,
        commit: OrganizerProposalCommit,
    ) -> crate::Result<OrganizerProposal> {
        if matches!(
            &commit.replacement.status,
            ProposalStatus::Pending | ProposalStatus::Approved
        ) {
            return Err(DriveError::InvalidArgs(
                "organizer proposal commit requires a terminal status".to_string(),
            ));
        }
        if commit.entry_update.is_none() && commit.correction.is_some() {
            return Err(DriveError::InvalidArgs(
                "organizer correction requires an entry update".to_string(),
            ));
        }

        let mut inner = self.inner.lock();
        let proposal_id = commit.replacement.id;
        let current_proposal = inner
            .proposals
            .get(&proposal_id)
            .cloned()
            .ok_or_else(|| DriveError::NotFound(format!("organizer proposal {proposal_id}")))?;
        require_version(
            "proposal",
            proposal_id,
            commit.expected_proposal_version,
            current_proposal.version,
        )?;
        if !matches!(
            &current_proposal.status,
            ProposalStatus::Pending | ProposalStatus::Approved
        ) {
            return Ok(current_proposal);
        }
        if current_proposal.entry_id != commit.replacement.entry_id {
            return Err(DriveError::InvalidArgs(
                "organizer proposal replacement changed entry id".to_string(),
            ));
        }

        let entry_change = if let Some(update) = commit.entry_update {
            let entry_id = update.replacement.id;
            if entry_id != current_proposal.entry_id {
                return Err(DriveError::InvalidArgs(format!(
                    "organizer entry {entry_id} does not match proposal entry {}",
                    current_proposal.entry_id
                )));
            }
            let current_entry = inner
                .entries
                .get(&entry_id)
                .cloned()
                .ok_or_else(|| DriveError::NotFound(format!("drive entry {entry_id}")))?;
            require_version(
                "entry",
                entry_id,
                update.expected_version,
                current_entry.version,
            )?;
            if let Some(occupant) = inner.path_to_id.get(&update.replacement.path)
                && *occupant != entry_id
            {
                return Err(DriveError::Collision(update.replacement.path));
            }
            if let Some(correction) = commit.correction.as_ref() {
                validate_move_correction(&current_entry, &update.replacement, correction)?;
                reject_duplicate_correction(&inner, correction.id)?;
            }
            let mut replacement = update.replacement;
            replacement.version = next_version("entry", entry_id, update.expected_version)?;
            Some((current_entry, replacement))
        } else {
            None
        };

        let mut replacement_proposal = commit.replacement;
        replacement_proposal.version =
            next_version("proposal", proposal_id, commit.expected_proposal_version)?;
        let correction = commit.correction.map(|mut correction| {
            correction.version = initial_record_version();
            correction
        });

        if let Some((current_entry, replacement_entry)) = entry_change {
            if current_entry.path != replacement_entry.path {
                inner.path_to_id.remove(&current_entry.path);
                inner
                    .path_to_id
                    .insert(replacement_entry.path.clone(), replacement_entry.id);
            }
            inner
                .entries
                .insert(replacement_entry.id, replacement_entry);
        }
        if let Some(correction) = correction {
            inner.corrections.push(correction);
        }
        inner
            .proposals
            .insert(proposal_id, replacement_proposal.clone());
        Ok(replacement_proposal)
    }

    async fn organizer_runs(&self) -> crate::Result<Vec<OrganizerRun>> {
        Ok(self.inner.lock().organizer_runs.values().cloned().collect())
    }

    async fn insert_organizer_run(&self, mut run: OrganizerRun) -> crate::Result<OrganizerRun> {
        run.version = initial_record_version();
        let mut inner = self.inner.lock();
        if matches!(
            run.status,
            crate::OrganizerRunStatus::Queued | crate::OrganizerRunStatus::Running
        ) && let Some(existing) = inner.organizer_runs.values().find(|existing| {
            matches!(
                existing.status,
                crate::OrganizerRunStatus::Queued | crate::OrganizerRunStatus::Running
            )
        }) {
            return Err(version_conflict(
                "organizer run",
                "active",
                0,
                existing.version,
            ));
        }
        if let Some(existing) = inner.organizer_runs.get(&run.id) {
            return Err(version_conflict(
                "organizer run",
                run.id,
                0,
                existing.version,
            ));
        }
        inner.organizer_runs.insert(run.id, run.clone());
        Ok(run)
    }

    async fn compare_and_swap_organizer_run(
        &self,
        id: DriveOrganizerRunId,
        expected_version: u64,
        mut replacement: OrganizerRun,
    ) -> crate::Result<OrganizerRun> {
        let mut inner = self.inner.lock();
        let current = inner
            .organizer_runs
            .get(&id)
            .ok_or_else(|| DriveError::NotFound(format!("organizer run {id}")))?;
        require_version("organizer run", id, expected_version, current.version)?;
        if replacement.id != id {
            return Err(DriveError::InvalidArgs(format!(
                "replacement organizer run id {} does not match {id}",
                replacement.id
            )));
        }
        replacement.version = next_version("organizer run", id, expected_version)?;
        inner.organizer_runs.insert(id, replacement.clone());
        Ok(replacement)
    }

    async fn links(&self) -> crate::Result<Vec<DriveLinkRecord>> {
        Ok(self.inner.lock().links.values().cloned().collect())
    }

    async fn link(&self, alias: &str) -> crate::Result<Option<DriveLinkRecord>> {
        Ok(self.inner.lock().links.get(alias).cloned())
    }

    async fn insert_link(&self, mut link: DriveLinkRecord) -> crate::Result<DriveLinkRecord> {
        link.version = initial_record_version();
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.links.get(&link.alias) {
            return Err(version_conflict("link", &link.alias, 0, existing.version));
        }
        inner.links.insert(link.alias.clone(), link.clone());
        Ok(link)
    }

    async fn compare_and_swap_link(
        &self,
        alias: &str,
        expected_version: u64,
        mut replacement: DriveLinkRecord,
    ) -> crate::Result<DriveLinkRecord> {
        let mut inner = self.inner.lock();
        let current = inner
            .links
            .get(alias)
            .ok_or_else(|| DriveError::NotFound(format!("drive link {alias}")))?;
        require_version("link", alias, expected_version, current.version)?;
        if replacement.alias != alias {
            return Err(DriveError::InvalidArgs(format!(
                "replacement link alias {} does not match {alias}",
                replacement.alias
            )));
        }
        replacement.version = next_version("link", alias, expected_version)?;
        inner.links.insert(alias.to_string(), replacement.clone());
        Ok(replacement)
    }

    async fn append_correction(
        &self,
        mut correction: DriveCorrectionRecord,
    ) -> crate::Result<DriveCorrectionRecord> {
        correction.version = initial_record_version();
        self.inner.lock().corrections.push(correction.clone());
        Ok(correction)
    }

    async fn snapshot(&self) -> crate::Result<DriveMetadataSnapshot> {
        Ok(self.snapshot_now())
    }
}

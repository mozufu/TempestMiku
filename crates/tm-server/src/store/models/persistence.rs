use super::*;
pub(crate) fn turn_content_hash(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

pub(crate) fn redact_persisted_text(content: &str) -> String {
    tm_memory::redact_dream_text(content).text
}

pub(crate) fn redact_persisted_json(mut payload: Value) -> Value {
    tm_memory::redact_json_value(&mut payload);
    payload
}

pub(crate) fn sanitize_approval_request_persistence(
    mut request: NewApprovalRequest,
) -> Result<NewApprovalRequest> {
    validate_persistence_identifier("approval origin", &request.origin)?;
    validate_persistence_identifier("approval effect type", &request.effect_type)?;
    request.action = redact_persisted_text(&request.action);
    tm_memory::redact_json_value(&mut request.scope_json);
    tm_memory::redact_json_value(&mut request.options_json);
    tm_memory::redact_json_value(&mut request.effect_payload_json);
    if request.origin.trim().is_empty() || request.action.trim().is_empty() {
        return Err(ServerError::InvalidRequest(
            "approval origin and action must not be empty".to_string(),
        ));
    }
    if !request.options_json.is_array() {
        return Err(ServerError::InvalidRequest(
            "approval options must be an array".to_string(),
        ));
    }
    if request.effect_type.trim().is_empty() {
        return Err(ServerError::InvalidRequest(
            "approval effect type must not be empty".to_string(),
        ));
    }
    if request.expires_at <= request.created_at {
        return Err(ServerError::InvalidRequest(
            "approval expiry must be after creation".to_string(),
        ));
    }
    Ok(request)
}

pub(crate) fn validate_profile_fact_persistence(fact: &ProfileFactRecord) -> Result<()> {
    reject_sensitive_persistence_fields([
        ("profile fact subject", fact.subject.as_str()),
        ("profile fact predicate", fact.predicate.as_str()),
        ("profile fact object", fact.object.as_str()),
        ("profile fact provenance", fact.provenance.as_str()),
    ])
}

pub(crate) fn validate_recall_chunk_persistence(chunk: &RecallChunkRecord) -> Result<()> {
    reject_sensitive_persistence_fields([
        ("recall scope", chunk.scope.as_str()),
        ("recall text", chunk.text.as_str()),
        ("recall source", chunk.source.as_str()),
    ])
}

pub(crate) fn sanitize_durable_memory_record(
    mut record: StoredMemoryRecord,
) -> Result<StoredMemoryRecord> {
    match &mut record.resource {
        MemoryRecordResource::Episodic(episodic) => {
            reject_sensitive_persistence_fields([
                (
                    "memory record owner subject",
                    episodic.owner_subject.as_str(),
                ),
                ("memory record scope", episodic.memory_scope.as_str()),
            ])?;
            episodic.text = redact_persisted_text(&episodic.text);
            sanitize_durable_memory_evidence(&mut episodic.evidence)?;
        }
        MemoryRecordResource::Semantic(semantic) => {
            reject_sensitive_persistence_fields([
                (
                    "memory record owner subject",
                    semantic.owner_subject.as_str(),
                ),
                ("memory record scope", semantic.memory_scope.as_str()),
                (
                    "memory semantic subject",
                    semantic.semantic_subject.as_str(),
                ),
                ("memory semantic predicate", semantic.predicate.as_str()),
                ("memory semantic object", semantic.object.as_str()),
            ])?;
            sanitize_durable_memory_evidence(&mut semantic.evidence)?;
        }
    }
    StoredMemoryRecord::new(record.resource)
        .map_err(|error| ServerError::InvalidRequest(error.to_string()))
}

pub(crate) fn sanitize_new_memory_embedding_job(
    job: NewMemoryEmbeddingJob,
) -> Result<NewMemoryEmbeddingJob> {
    job.validate()
        .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
    reject_sensitive_persistence_fields([
        ("embedding job owner subject", job.owner_subject.as_str()),
        ("embedding job scope", job.memory_scope.as_str()),
        ("embedding job content key", job.content_key.as_str()),
        ("embedding model id", job.provenance.model_id.as_str()),
    ])?;
    Ok(job)
}

pub(crate) fn validate_persistence_identifier(field: &str, value: &str) -> Result<()> {
    reject_sensitive_persistence_fields([(field, value)])
}

pub(crate) fn sanitize_project_item_persistence(
    mut item: NewProjectItem,
) -> Result<NewProjectItem> {
    reject_sensitive_persistence_fields([
        ("project id", item.project_id.as_str()),
        ("project target URI", item.target_uri.as_str()),
        ("project dedupe key", item.dedupe_key.as_str()),
    ])?;
    if let Some(source_uri) = item.source_uri.as_deref() {
        reject_sensitive_persistence_fields([("project source URI", source_uri)])?;
    }
    item.text = redact_persisted_text(&item.text);
    item.provenance_json = redact_persisted_json(item.provenance_json);
    Ok(item)
}

pub(crate) fn sanitize_memory_summary_persistence(
    mut summary: NewMemorySummaryRecord,
) -> Result<NewMemorySummaryRecord> {
    reject_sensitive_persistence_fields([
        ("memory summary subject", summary.subject.as_str()),
        ("memory summary scope", summary.scope.as_str()),
        ("memory summary dedupe key", summary.dedupe_key.as_str()),
    ])?;
    summary.title = redact_persisted_text(&summary.title);
    summary.body = redact_persisted_text(&summary.body);
    sanitize_memory_evidence(&mut summary.evidence)?;
    Ok(summary)
}

pub(crate) fn sanitize_skill_proposal_persistence(
    mut proposal: NewSkillProposalRecord,
) -> Result<NewSkillProposalRecord> {
    reject_sensitive_persistence_fields([
        ("skill proposal name", proposal.name.as_str()),
        ("skill proposal dedupe key", proposal.dedupe_key.as_str()),
    ])?;
    proposal.description = redact_persisted_text(&proposal.description);
    proposal.body = redact_persisted_text(&proposal.body);
    proposal.trigger = redact_persisted_text(&proposal.trigger);
    proposal.use_criteria = redact_persisted_text(&proposal.use_criteria);
    proposal.self_critique = redact_persisted_text(&proposal.self_critique);
    proposal.verification.checks = std::mem::take(&mut proposal.verification.checks)
        .into_iter()
        .map(|check| redact_persisted_text(&check))
        .collect();
    sanitize_memory_evidence(&mut proposal.evidence)?;
    Ok(proposal)
}

pub(crate) fn sanitize_evolution_review_proposal_persistence(
    mut proposal: NewEvolutionReviewProposal,
) -> Result<NewEvolutionReviewProposal> {
    validate_persistence_identifier("review proposal target", proposal.target.id())?;
    validate_persistence_identifier("review proposal base digest", &proposal.base_digest)?;
    if let Some(digest) = &proposal.base_active_digest {
        validate_persistence_identifier("review proposal base active digest", digest)?;
    }
    validate_persistence_identifier("review proposal content digest", &proposal.content_digest)?;
    if proposal.base_version == 0 {
        return Err(ServerError::InvalidRequest(
            "review proposal base version must be positive".to_string(),
        ));
    }
    if proposal.changes.is_empty() || proposal.changes.len() > tm_modes::MAX_REVIEW_PROPOSAL_CHANGES
    {
        return Err(ServerError::InvalidRequest(format!(
            "review proposal must contain 1..={} changes",
            tm_modes::MAX_REVIEW_PROPOSAL_CHANGES
        )));
    }
    if serde_json::to_vec(&proposal.changes)?.len() > tm_modes::MAX_REVIEW_METADATA_BYTES {
        return Err(ServerError::InvalidRequest(format!(
            "review proposal metadata exceeds {} bytes",
            tm_modes::MAX_REVIEW_METADATA_BYTES
        )));
    }
    let target_kind = proposal.target.kind();
    for change in &mut proposal.changes {
        if !review_section_allowed(target_kind, change.section) {
            return Err(ServerError::InvalidRequest(format!(
                "review section {:?} is not valid for {target_kind}",
                change.section
            )));
        }
        change.after.label = redact_persisted_text(&change.after.label);
        change.after.summary = redact_persisted_text(&change.after.summary);
        if change.after.label.trim().is_empty() || change.after.summary.trim().is_empty() {
            return Err(ServerError::InvalidRequest(
                "review proposal labels and summaries must not be empty".to_string(),
            ));
        }
        if let Some(before) = &mut change.before {
            before.label = redact_persisted_text(&before.label);
            before.summary = redact_persisted_text(&before.summary);
        }
    }
    if let Some(candidate) = &mut proposal.auto_candidate {
        sanitize_persona_auto_candidate(
            candidate,
            &proposal.target,
            proposal.apply_contract,
            proposal.changes.len(),
        )?;
    }
    Ok(proposal)
}

fn sanitize_persona_auto_candidate(
    candidate: &mut PersonaAutoCandidate,
    target: &tm_modes::ReviewProposalTarget,
    apply_contract: tm_modes::ReviewApplyContract,
    change_count: usize,
) -> Result<()> {
    if candidate.schema_version != PERSONA_AUTO_CANDIDATE_SCHEMA_VERSION {
        return Err(ServerError::InvalidRequest(format!(
            "unsupported persona auto-candidate schema version {}",
            candidate.schema_version
        )));
    }
    if !matches!(
        target,
        tm_modes::ReviewProposalTarget::Persona { persona_id } if persona_id == "miku"
    ) || apply_contract != tm_modes::ReviewApplyContract::VersionedPersonaAddendum
        || change_count != 1
    {
        return Err(ServerError::InvalidRequest(
            "persona auto-candidates require one activatable miku persona change".to_string(),
        ));
    }
    validate_persistence_identifier("persona auto-candidate dedupe key", &candidate.dedupe_key)?;
    let digest = candidate.dedupe_key.strip_prefix("persona-auto:v1:sha256:");
    if digest.is_none_or(|digest| {
        digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) {
        return Err(ServerError::InvalidRequest(
            "persona auto-candidate dedupe key is invalid".to_string(),
        ));
    }
    let minimum_evidence = match candidate.trigger {
        PersonaAutoCandidateTrigger::RepeatedPreference => 2,
        PersonaAutoCandidateTrigger::PersonaMismatch => 1,
    };
    if candidate.evidence.len() < minimum_evidence
        || candidate.evidence.len() > MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE
    {
        return Err(ServerError::InvalidRequest(format!(
            "persona auto-candidate evidence must contain {minimum_evidence}..={} records",
            MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE
        )));
    }
    let mut record_ids = std::collections::BTreeSet::new();
    for evidence in &mut candidate.evidence {
        if !record_ids.insert(evidence.record_id) {
            return Err(ServerError::InvalidRequest(
                "persona auto-candidate evidence records must be distinct".to_string(),
            ));
        }
        let expected_uri = format!(
            "memory://records/{}/{}",
            evidence.kind.as_str(),
            evidence.record_id
        );
        if evidence.source_uri != expected_uri {
            return Err(ServerError::InvalidRequest(
                "persona auto-candidate evidence URI does not match its typed record".to_string(),
            ));
        }
        if evidence.evidence.is_empty()
            || evidence.evidence.len() > MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE_REFS
        {
            return Err(ServerError::InvalidRequest(format!(
                "persona auto-candidate record evidence must contain 1..={} references",
                MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE_REFS
            )));
        }
        for reference in &mut evidence.evidence {
            *reference = redact_persisted_text(reference);
            if reference.trim().is_empty()
                || reference.len() > MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE_REF_BYTES
            {
                return Err(ServerError::InvalidRequest(format!(
                    "persona auto-candidate evidence reference exceeds {} bytes",
                    MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE_REF_BYTES
                )));
            }
        }
    }
    Ok(())
}

fn review_section_allowed(target_kind: &str, section: tm_modes::ReviewAddendumSection) -> bool {
    use tm_modes::ReviewAddendumSection::{
        AddressGuidance, BehaviorGuidance, Description, InteractionPreference, RoutingGuidance,
        ToneGuidance, VoiceGuidance,
    };
    match target_kind {
        "persona" => matches!(
            section,
            BehaviorGuidance
                | VoiceGuidance
                | ToneGuidance
                | AddressGuidance
                | InteractionPreference
        ),
        "mode" => matches!(section, Description | RoutingGuidance),
        _ => false,
    }
}

pub(crate) fn sanitize_cron_job_persistence(mut job: NewCronJobRecord) -> Result<NewCronJobRecord> {
    reject_sensitive_persistence_fields([
        ("cron job id", job.id.as_str()),
        ("cron schedule", job.schedule.as_str()),
        ("cron mode", job.cron_mode.as_str()),
    ])?;
    job.name = redact_persisted_text(&job.name);
    Ok(job)
}

pub(crate) fn sanitize_cron_run_persistence(mut run: NewCronRunRecord) -> Result<NewCronRunRecord> {
    reject_sensitive_persistence_fields([
        ("cron run job id", run.job_id.as_str()),
        ("cron run status", run.status.as_str()),
    ])?;
    run.result_json = redact_persisted_json(run.result_json);
    Ok(run)
}

fn sanitize_memory_evidence(evidence: &mut [MemoryEvidenceRef]) -> Result<()> {
    for item in evidence {
        if let Some(uri) = item.uri.as_deref() {
            reject_sensitive_persistence_fields([("memory evidence URI", uri)])?;
        }
        item.label = redact_persisted_text(&item.label);
    }
    Ok(())
}

fn sanitize_durable_memory_evidence(
    evidence: &mut [tm_memory::MemoryRecordEvidence],
) -> Result<()> {
    for item in evidence {
        if let MemoryEvidenceSource::Resource { uri } = &item.source {
            reject_sensitive_persistence_fields([("memory record evidence URI", uri.as_str())])?;
        }
        item.label = redact_persisted_text(&item.label);
    }
    Ok(())
}

fn reject_sensitive_persistence_fields<'a>(
    fields: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<()> {
    for (field, value) in fields {
        if tm_memory::contains_sensitive_data(value) {
            return Err(ServerError::InvalidRequest(format!(
                "{field} contains sensitive data"
            )));
        }
    }
    Ok(())
}

use serde_json::Value;
use tm_host::EvolutionAuditRecord;
use tm_memory::{
    DreamQueueRecord, DreamReason, DreamStatus, EmbeddingNormalization, EmbeddingProvenance,
    EmbeddingProvider, EpisodicMemoryRecord, MemoryEmbeddingJobRecord, MemoryEmbeddingJobStatus,
    MemoryRecordKind, MemoryRecordLinks, MemoryRecordResource, MemoryRecordStatus,
    MemorySummaryKind, MemorySummaryRecord, ReembeddingState, SemanticMemoryRecord,
    SkillProposalRecord, SkillProposalStatus, SkillVerification, StoredMemoryRecord,
};

use crate::{Result, ServerError};

use super::{
    ApprovalEffectRecord, ApprovalRequestRecord, CronJobRecord, CronRunRecord,
    EvolutionReviewProposalRecord, MessageRecord, ModeState, ProjectItemRecord, SessionRecord,
    SessionTurnRecord,
};

pub(super) fn row_to_approval_request(row: tokio_postgres::Row) -> ApprovalRequestRecord {
    ApprovalRequestRecord {
        id: row.get("id"),
        session_id: row.get("session_id"),
        turn_id: row.get("turn_id"),
        requester_id: row.get("requester_id"),
        origin: row.get("origin"),
        action: row.get("action"),
        scope_json: row.get("scope_json"),
        options_json: row.get("options_json"),
        status: row.get("status"),
        resumable: row.get("resumable"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        heartbeat_at: row.get("heartbeat_at"),
        resolved_at: row.get("resolved_at"),
        selected_option_id: row.get("selected_option_id"),
        resolution_json: row.get("resolution_json"),
        request_event_seq: row.get("request_event_seq"),
        resolution_event_seq: row.get("resolution_event_seq"),
        resolution_version: row.get("resolution_version"),
    }
}

pub(super) fn row_to_approval_effect(row: tokio_postgres::Row) -> ApprovalEffectRecord {
    ApprovalEffectRecord {
        id: row.get("id"),
        approval_id: row.get("approval_id"),
        session_id: row.get("session_id"),
        effect_type: row.get("effect_type"),
        payload_json: row.get("payload_json"),
        status: row.get("status"),
        attempts: row.get("attempts"),
        available_at: row.get("available_at"),
        locked_at: row.get("locked_at"),
        lease_owner: row.get("lease_owner"),
        lease_epoch: row.get("lease_epoch"),
        applied_at: row.get("applied_at"),
        error_at: row.get("error_at"),
        last_error: row.get("last_error"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

pub(super) fn row_to_evolution_audit(row: tokio_postgres::Row) -> Result<EvolutionAuditRecord> {
    serde_json::from_value(row.get("record_json"))
        .map_err(|error| ServerError::Store(format!("invalid evolution audit record: {error}")))
}

pub(super) fn row_to_evolution_review_proposal(
    row: tokio_postgres::Row,
) -> Result<EvolutionReviewProposalRecord> {
    serde_json::from_value(row.get("record_json"))
        .map_err(|error| ServerError::Store(format!("invalid evolution review proposal: {error}")))
}

pub(super) fn row_to_cron_job(row: tokio_postgres::Row) -> CronJobRecord {
    CronJobRecord {
        id: row.get("id"),
        name: row.get("name"),
        schedule: row.get("schedule"),
        enabled: row.get("enabled"),
        cron_mode: row.get("cron_mode"),
        max_turns: row.get("max_turns"),
        script_timeout_seconds: row.get("script_timeout_seconds"),
        next_run_at: row.get("next_run_at"),
        updated_at: row.get("updated_at"),
    }
}

pub(super) fn row_to_cron_run(row: tokio_postgres::Row) -> CronRunRecord {
    CronRunRecord {
        id: row.get("id"),
        job_id: row.get("job_id"),
        scheduled_for: row.get("scheduled_for"),
        status: row.get("status"),
        session_id: row.get("session_id"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
        attempts: row.get("attempts"),
        available_at: row.get("available_at"),
        locked_at: row.get("locked_at"),
        lease_owner: row.get("lease_owner"),
        lease_epoch: row.get("lease_epoch"),
        last_error: row.get("last_error"),
        result_json: row.get("result_json"),
    }
}

pub(super) fn row_to_memory_summary(row: tokio_postgres::Row) -> Result<MemorySummaryRecord> {
    let kind: String = row.get("kind");
    let evidence_json: Value = row.get("evidence_json");
    Ok(MemorySummaryRecord {
        id: row.get("id"),
        kind: kind
            .parse::<MemorySummaryKind>()
            .map_err(|err| ServerError::Store(err.to_string()))?,
        subject: row.get("subject"),
        scope: row.get("scope"),
        title: row.get("title"),
        body: row.get("body"),
        evidence: serde_json::from_value(evidence_json)
            .map_err(|err| ServerError::Store(err.to_string()))?,
        source_dream_id: row.get("source_dream_id"),
        source_session_id: row.get("source_session_id"),
        dedupe_key: row.get("dedupe_key"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub(super) fn row_to_stored_memory_record(row: tokio_postgres::Row) -> Result<StoredMemoryRecord> {
    let record_kind: String = row.get("record_kind");
    let kind = MemoryRecordKind::parse(&record_kind).ok_or_else(|| {
        ServerError::Store(format!("unknown durable memory record kind {record_kind}"))
    })?;
    let status: String = row.get("status");
    let status = MemoryRecordStatus::parse(&status)
        .ok_or_else(|| ServerError::Store(format!("unknown durable memory status {status}")))?;
    let schema_version: i32 = row.get("schema_version");
    let schema_version = u16::try_from(schema_version).map_err(|_| {
        ServerError::Store(format!(
            "invalid durable memory schema version {schema_version}"
        ))
    })?;
    let evidence = serde_json::from_value(row.get("evidence_json"))
        .map_err(|error| ServerError::Store(format!("invalid durable memory evidence: {error}")))?;
    let links = MemoryRecordLinks {
        corrects_record_id: row.get("corrects_record_id"),
        corrected_by_record_id: row.get("corrected_by_record_id"),
        supersedes_record_id: row.get("supersedes_record_id"),
        superseded_by_record_id: row.get("superseded_by_record_id"),
    };
    let resource = match kind {
        MemoryRecordKind::Episodic => MemoryRecordResource::Episodic(EpisodicMemoryRecord {
            schema_version,
            id: row.get("id"),
            owner_subject: row.get("owner_subject"),
            memory_scope: row.get("memory_scope"),
            text: row
                .get::<_, Option<String>>("text")
                .ok_or_else(|| ServerError::Store("episodic record is missing text".to_string()))?,
            evidence,
            confidence: row.get("confidence"),
            importance: row.get("importance"),
            observed_at: row.get("observed_at"),
            effective_from: row.get("effective_from"),
            effective_to: row.get("effective_to"),
            status,
            links,
            created_at: row.get("created_at"),
        }),
        MemoryRecordKind::Semantic => MemoryRecordResource::Semantic(SemanticMemoryRecord {
            schema_version,
            id: row.get("id"),
            owner_subject: row.get("owner_subject"),
            memory_scope: row.get("memory_scope"),
            semantic_subject: row
                .get::<_, Option<String>>("semantic_subject")
                .ok_or_else(|| {
                    ServerError::Store("semantic record is missing semantic_subject".to_string())
                })?,
            predicate: row.get::<_, Option<String>>("predicate").ok_or_else(|| {
                ServerError::Store("semantic record is missing predicate".to_string())
            })?,
            object: row.get::<_, Option<String>>("object").ok_or_else(|| {
                ServerError::Store("semantic record is missing object".to_string())
            })?,
            evidence,
            confidence: row.get("confidence"),
            importance: row.get("importance"),
            observed_at: row.get("observed_at"),
            effective_from: row.get("effective_from"),
            effective_to: row.get("effective_to"),
            status,
            links,
            created_at: row.get("created_at"),
        }),
    };
    let stored = StoredMemoryRecord {
        resource,
        content_key: row.get("content_key"),
        version_key: row.get("version_key"),
    };
    stored
        .validate()
        .map_err(|error| ServerError::Store(format!("invalid durable memory record: {error}")))?;
    Ok(stored)
}

pub(super) fn row_to_memory_embedding_job(
    row: tokio_postgres::Row,
) -> Result<MemoryEmbeddingJobRecord> {
    let record_kind: String = row.get("record_kind");
    let record_kind = MemoryRecordKind::parse(&record_kind).ok_or_else(|| {
        ServerError::Store(format!("unknown durable memory record kind {record_kind}"))
    })?;
    let provider: String = row.get("provider");
    let provider = EmbeddingProvider::parse(&provider)
        .ok_or_else(|| ServerError::Store(format!("unknown embedding provider {provider}")))?;
    let normalization: String = row.get("normalization");
    let normalization = EmbeddingNormalization::parse(&normalization).ok_or_else(|| {
        ServerError::Store(format!("unknown embedding normalization {normalization}"))
    })?;
    let reembedding_state: String = row.get("reembedding_state");
    let reembedding_state = ReembeddingState::parse(&reembedding_state).ok_or_else(|| {
        ServerError::Store(format!("unknown reembedding state {reembedding_state}"))
    })?;
    let status: String = row.get("status");
    let status = MemoryEmbeddingJobStatus::parse(&status)
        .ok_or_else(|| ServerError::Store(format!("unknown embedding job status {status}")))?;
    let schema_version: i32 = row.get("schema_version");
    let schema_version = u16::try_from(schema_version).map_err(|_| {
        ServerError::Store(format!("invalid embedding schema version {schema_version}"))
    })?;
    let dimensions: i32 = row.get("dimensions");
    let dimensions = usize::try_from(dimensions)
        .map_err(|_| ServerError::Store(format!("invalid embedding dimensions {dimensions}")))?;
    let provenance = EmbeddingProvenance {
        schema_version,
        provider,
        model_id: row.get("model_id"),
        dimensions,
        normalization,
        content_hash: row.get("content_hash"),
        embedding_version: row.get("embedding_version"),
        created_at: row.get("provenance_created_at"),
        reembedding_state,
    };
    provenance
        .validate()
        .map_err(|error| ServerError::Store(format!("invalid embedding provenance: {error}")))?;
    Ok(MemoryEmbeddingJobRecord {
        id: row.get("id"),
        record_kind,
        record_id: row.get("record_id"),
        owner_subject: row.get("owner_subject"),
        memory_scope: row.get("memory_scope"),
        content_key: row.get("content_key"),
        provenance,
        reembedding_key: row.get("reembedding_key"),
        status,
        input_limit_bytes: usize::try_from(row.get::<_, i32>("input_limit_bytes"))
            .map_err(|_| ServerError::Store("invalid embedding job input limit".to_string()))?,
        failure_code: row.get("failure_code"),
        attempts: row.get("attempts"),
        available_at: row.get("available_at"),
        locked_at: row.get("locked_at"),
        lease_owner: row.get("lease_owner"),
        lease_epoch: row.get("lease_epoch"),
        cancelled_at: row.get("cancelled_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub(super) fn row_to_skill_proposal(row: tokio_postgres::Row) -> Result<SkillProposalRecord> {
    let evidence_json: Value = row.get("evidence_json");
    let verification_json: Value = row.get("verification_json");
    let status: String = row.get("status");
    Ok(SkillProposalRecord {
        id: row.get("id"),
        name: row.get("name"),
        description: row.get("description"),
        body: row.get("body"),
        trigger: row.get("trigger"),
        use_criteria: row.get("use_criteria"),
        evidence: serde_json::from_value(evidence_json)
            .map_err(|err| ServerError::Store(err.to_string()))?,
        self_critique: row.get("self_critique"),
        verification: serde_json::from_value::<SkillVerification>(verification_json)
            .map_err(|err| ServerError::Store(err.to_string()))?,
        status: status
            .parse::<SkillProposalStatus>()
            .map_err(|err| ServerError::Store(err.to_string()))?,
        dedupe_key: row.get("dedupe_key"),
        source_dream_id: row.get("source_dream_id"),
        source_session_id: row.get("source_session_id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub(super) fn row_to_dream_record(row: tokio_postgres::Row) -> Result<DreamQueueRecord> {
    let reason: String = row.get("reason");
    let status: String = row.get("status");
    Ok(DreamQueueRecord {
        id: row.get("id"),
        session_id: row.get("session_id"),
        subject: row.get("subject"),
        scope: row.get("scope"),
        reason: reason
            .parse::<DreamReason>()
            .map_err(|err| ServerError::Store(err.to_string()))?,
        status: status
            .parse::<DreamStatus>()
            .map_err(|err| ServerError::Store(err.to_string()))?,
        dedupe_key: row.get("dedupe_key"),
        source_event_seq: row.get("source_event_seq"),
        attempts: row.get("attempts"),
        enqueued_at: row.get("enqueued_at"),
        available_at: row.get("available_at"),
        locked_at: row.get("locked_at"),
        lease_owner: row.get("lease_owner"),
        lease_epoch: row.get("lease_epoch"),
        heartbeat_at: row.get("heartbeat_at"),
        completed_at: row.get("completed_at"),
        error_at: row.get("error_at"),
        last_error: row.get("last_error"),
    })
}

pub(super) fn row_to_project_item(row: tokio_postgres::Row) -> Result<ProjectItemRecord> {
    let kind: String = row.get("kind");
    Ok(ProjectItemRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        kind: kind.parse()?,
        text: row.get("text"),
        target_uri: row.get("target_uri"),
        source_session_id: row.get("source_session_id"),
        source_event_seq: row.get("source_event_seq"),
        source_uri: row.get("source_uri"),
        dedupe_key: row.get("dedupe_key"),
        provenance_json: row.get("provenance_json"),
        created_at: row.get("created_at"),
    })
}

pub(super) fn row_to_session_record(row: &tokio_postgres::Row) -> Result<SessionRecord> {
    let mode: Value = row.get("mode");
    let mode: tm_modes::ModeId =
        serde_json::from_value(mode).map_err(|err| ServerError::Store(err.to_string()))?;
    let mode_state: Option<Value> = row.get("mode_state_json");
    let mode_state = match mode_state {
        Some(value) => {
            serde_json::from_value(value).map_err(|err| ServerError::Store(err.to_string()))?
        }
        None => ModeState::new(mode.clone(), row.get("updated_at")),
    };
    let persona_status: Value = row.get("persona_status");
    Ok(SessionRecord {
        id: row.get("id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        status: row.get("status"),
        mode,
        mode_state,
        persona_status: serde_json::from_value(persona_status)
            .map_err(|err| ServerError::Store(err.to_string()))?,
        owner_subject: row.get("owner_subject"),
        memory_scope: row.get("memory_scope"),
    })
}

pub(super) fn row_to_message_record(row: tokio_postgres::Row) -> MessageRecord {
    MessageRecord {
        session_id: row.get("session_id"),
        seq: row.get("seq"),
        role: row.get("role"),
        content: row.get("content"),
        turn_id: row.get("turn_id"),
        created_at: row.get("created_at"),
    }
}

pub(super) fn row_to_session_turn(row: tokio_postgres::Row) -> SessionTurnRecord {
    SessionTurnRecord {
        id: row.get("id"),
        session_id: row.get("session_id"),
        client_message_id: row.get("client_message_id"),
        content: row.get("content"),
        content_hash: row.get("content_hash"),
        status: row.get("status"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
        worker_id: row.get("worker_id"),
        error: row.get("error"),
    }
}

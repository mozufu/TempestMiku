use serde_json::Value;
use tm_host::EvolutionAuditRecord;
use tm_memory::{
    DreamQueueRecord, DreamReason, DreamStatus, MemorySummaryKind, MemorySummaryRecord,
    SkillProposalRecord, SkillProposalStatus, SkillVerification,
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

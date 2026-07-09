use serde_json::Value;
use tm_memory::{
    DreamQueueRecord, DreamReason, DreamStatus, MemorySummaryKind, MemorySummaryRecord,
    SkillProposalRecord, SkillProposalStatus, SkillVerification,
};

use crate::{Result, ServerError};

use super::{
    CronJobRecord, CronRunRecord, MessageRecord, ModeState, ProjectItemKind, ProjectItemRecord,
    SessionRecord,
};

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
    })
}

pub(super) fn row_to_message_record(row: tokio_postgres::Row) -> MessageRecord {
    MessageRecord {
        session_id: row.get("session_id"),
        seq: row.get("seq"),
        role: row.get("role"),
        content: row.get("content"),
        created_at: row.get("created_at"),
    }
}

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tm_host::EvolutionAuditRecord;
use uuid::Uuid;

use crate::{Result, ServerError};

use super::{
    ApprovalEffectLease, ApprovalEffectRecord, ApprovalRequestRecord, EvolutionAuditEntry, Inner,
    NewApprovalResolution, SessionEvent,
};

pub(super) fn append_evolution_audit_in_memory(
    inner: &mut Inner,
    entry: EvolutionAuditEntry,
) -> Result<EvolutionAuditRecord> {
    if let Some(existing) = inner
        .evolution_audits
        .iter()
        .find(|existing| existing.idempotency_key == entry.idempotency_key)
    {
        return Ok(existing.record.clone());
    }
    let record = entry.record.clone();
    inner.evolution_audits.push(entry);
    Ok(record)
}

pub(super) fn stale_approval_effect_lease(lease: &ApprovalEffectLease) -> ServerError {
    ServerError::NotFound(format!(
        "active approval effect lease {} owner {} epoch {}",
        lease.effect.id, lease.owner_id, lease.epoch
    ))
}

pub(super) fn approval_effect_is_claimable(
    effect: &ApprovalEffectRecord,
    now: DateTime<Utc>,
    stale_before: DateTime<Utc>,
) -> bool {
    (effect.status == "pending" && effect.available_at <= now)
        || (effect.status == "claimed"
            && effect
                .locked_at
                .is_some_and(|locked| locked <= stale_before))
}

pub(super) fn claim_approval_effect_at(
    effect: &mut ApprovalEffectRecord,
    owner_id: Uuid,
    now: DateTime<Utc>,
) -> ApprovalEffectLease {
    effect.status = "claimed".to_string();
    effect.attempts += 1;
    effect.locked_at = Some(now);
    effect.lease_owner = Some(owner_id);
    effect.lease_epoch += 1;
    effect.updated_at = now;
    effect.error_at = None;
    effect.last_error = None;
    ApprovalEffectLease {
        effect: effect.clone(),
        owner_id,
        epoch: effect.lease_epoch,
    }
}

pub(super) fn resolve_approval_in_memory(
    inner: &mut Inner,
    session_id: Uuid,
    approval_id: Uuid,
    mut resolution: NewApprovalResolution,
) -> Result<ApprovalRequestRecord> {
    tm_memory::redact_json_value(&mut resolution.resolution_json);
    if !matches!(
        resolution.status.as_str(),
        "approved" | "denied" | "timed_out" | "cancelled"
    ) {
        return Err(ServerError::InvalidRequest(format!(
            "invalid terminal approval status {}",
            resolution.status
        )));
    }
    let index = inner
        .approval_requests
        .iter()
        .position(|approval| approval.id == approval_id && approval.session_id == session_id)
        .ok_or_else(|| ServerError::NotFound(format!("approval {approval_id}")))?;
    if inner.approval_requests[index].status != "pending" {
        return Err(ServerError::Conflict(format!(
            "approval {approval_id} is already resolved"
        )));
    }
    let effect_payload = inner
        .approval_effects
        .iter()
        .find(|effect| effect.approval_id == approval_id)
        .map(|effect| (effect.id, effect.payload_json.clone()));
    let audit = if let Some((effect_id, payload)) = effect_payload {
        crate::evolution::evolution_audit_record(
            &payload,
            approval_id,
            effect_id,
            crate::evolution::audit_status_from_approval(&resolution.status)?,
            resolution.resolved_at,
            None,
        )?
        .map(|record| EvolutionAuditEntry {
            idempotency_key: format!("approval:{approval_id}:resolution:{}", resolution.status),
            record,
        })
    } else {
        None
    };
    {
        let approval = &mut inner.approval_requests[index];
        approval.status = resolution.status;
        approval.resolved_at = Some(resolution.resolved_at);
        approval.selected_option_id = resolution.selected_option_id;
        approval.resolution_json = Some(resolution.resolution_json.clone());
        approval.resolution_version += 1;
    }
    if let Some(effect) = inner
        .approval_effects
        .iter_mut()
        .find(|effect| effect.approval_id == approval_id)
    {
        effect.status = "pending".to_string();
        effect.available_at = resolution.resolved_at;
        effect.updated_at = resolution.resolved_at;
        if let Value::Object(payload) = &mut effect.payload_json {
            payload.insert("resolution".to_string(), resolution.resolution_json);
        } else {
            effect.payload_json = json!({
                "effect": effect.payload_json.clone(),
                "resolution": resolution.resolution_json,
            });
        }
    } else {
        inner.approval_effects.push(ApprovalEffectRecord {
            id: approval_id,
            approval_id,
            session_id,
            effect_type: "approval_resolution".to_string(),
            payload_json: json!({ "resolution": resolution.resolution_json }),
            status: "pending".to_string(),
            attempts: 0,
            available_at: resolution.resolved_at,
            locked_at: None,
            lease_owner: None,
            lease_epoch: 0,
            applied_at: None,
            error_at: None,
            last_error: None,
            created_at: resolution.resolved_at,
            updated_at: resolution.resolved_at,
        });
    }
    if let Some(audit) = audit {
        append_evolution_audit_in_memory(inner, audit)?;
    }
    Ok(inner.approval_requests[index].clone())
}

pub(super) fn resolve_approval_with_event_in_memory(
    inner: &mut Inner,
    session_id: Uuid,
    approval_id: Uuid,
    mut resolution: NewApprovalResolution,
) -> Result<(ApprovalRequestRecord, SessionEvent)> {
    tm_memory::redact_json_value(&mut resolution.resolution_json);
    let approval_id_text = approval_id.to_string();
    if resolution
        .resolution_json
        .get("approvalId")
        .and_then(Value::as_str)
        != Some(approval_id_text.as_str())
    {
        return Err(ServerError::InvalidRequest(
            "approval resolution event payload has a different approvalId".to_string(),
        ));
    }
    let event_payload = resolution.resolution_json.clone();
    let event_at = resolution.resolved_at;
    let mut approval = resolve_approval_in_memory(inner, session_id, approval_id, resolution)?;
    let events = inner.events.entry(session_id).or_default();
    let mut event = SessionEvent::new(
        session_id,
        events.len() as i64 + 1,
        "approval_resolved",
        event_payload,
        event_at,
    );
    event.turn_id = approval.turn_id;
    events.push(event.clone());
    let stored = inner
        .approval_requests
        .iter_mut()
        .find(|record| record.id == approval_id && record.session_id == session_id)
        .expect("resolved approval remains present");
    stored.resolution_event_seq = Some(event.seq);
    approval.resolution_event_seq = Some(event.seq);
    if let Some(session) = inner.sessions.get_mut(&session_id) {
        session.updated_at = event_at;
    }
    Ok((approval, event))
}

use serde_json::{Value, json};
use uuid::Uuid;

use crate::{ApprovalRequestRecord, Result, ServerError};

use super::{ApprovalOutcome, ApprovalPrompt, ApprovalStatus, DetailedApprovalOutcome};

pub(super) fn resolution_payload(
    approval_id: Uuid,
    origin: &str,
    detailed: &DetailedApprovalOutcome,
) -> Value {
    match &detailed.outcome {
        ApprovalOutcome::Selected { option_id } => json!({
            "approvalId": approval_id,
            "backend": origin,
            "status": detailed.status.as_str(),
            "outcome": "selected",
            "optionId": option_id,
        }),
        ApprovalOutcome::Cancelled => json!({
            "approvalId": approval_id,
            "backend": origin,
            "status": detailed.status.as_str(),
            "outcome": "cancelled",
        }),
    }
}

pub(super) fn selected_option_id(outcome: &ApprovalOutcome) -> Option<String> {
    match outcome {
        ApprovalOutcome::Selected { option_id } => Some(option_id.clone()),
        ApprovalOutcome::Cancelled => None,
    }
}

pub(super) fn prompt_from_record(record: &ApprovalRequestRecord) -> Result<ApprovalPrompt> {
    Ok(ApprovalPrompt {
        action: record.action.clone(),
        scope: record.scope_json.clone(),
        options: serde_json::from_value(record.options_json.clone())?,
    })
}

pub(super) fn detailed_from_record(
    record: &ApprovalRequestRecord,
) -> Result<DetailedApprovalOutcome> {
    let status = match record.status.as_str() {
        "approved" => ApprovalStatus::Approved,
        "denied" => ApprovalStatus::Denied,
        "timed_out" => ApprovalStatus::TimedOut,
        "cancelled" => ApprovalStatus::Cancelled,
        other => {
            return Err(ServerError::Store(format!(
                "approval {} has non-terminal status {other}",
                record.id
            )));
        }
    };
    let outcome = record
        .selected_option_id
        .clone()
        .map_or(ApprovalOutcome::Cancelled, |option_id| {
            ApprovalOutcome::Selected { option_id }
        });
    Ok(DetailedApprovalOutcome { outcome, status })
}

pub(super) fn select_option(
    prompt: &ApprovalPrompt,
    predicate: impl Fn(&str) -> bool,
) -> Option<String> {
    prompt
        .options
        .iter()
        .find(|option| predicate(&option.kind))
        .map(|option| option.option_id.clone())
}

pub(super) fn status_for_outcome(
    prompt: &ApprovalPrompt,
    outcome: &ApprovalOutcome,
) -> ApprovalStatus {
    match outcome {
        ApprovalOutcome::Selected { option_id } => prompt
            .options
            .iter()
            .find(|option| option.option_id == *option_id)
            .map(|option| {
                if option.kind.starts_with("allow_") {
                    ApprovalStatus::Approved
                } else if option.kind.starts_with("reject_") {
                    ApprovalStatus::Denied
                } else {
                    ApprovalStatus::Cancelled
                }
            })
            .unwrap_or(ApprovalStatus::Cancelled),
        ApprovalOutcome::Cancelled => ApprovalStatus::Cancelled,
    }
}

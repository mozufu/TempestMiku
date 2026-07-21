use serde::Deserialize;
use serde_json::Value;
use tm_host::{HostError, InvocationCtx};

use super::{
    DriveOrganizeFn,
    authority::{DriveAuthority, authorized_proposal_ids, drive_authority},
};
use crate::{
    OrganizerProposal, ProposalStatus,
    store::{
        core::drive_error_to_host,
        payloads::{
            drive_write_proposal_payload, organizer_completed_payload, organizer_failed_payload,
            organizer_failed_payload_with_proposals, organizer_started_payload,
        },
    },
};

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DriveOrganizeArgs {
    #[serde(default)]
    apply: bool,
}

impl DriveOrganizeFn {
    pub(super) async fn call_drive(
        &self,
        args: Value,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Value> {
        let args: DriveOrganizeArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let store = &self.store;
        let authority = drive_authority(ctx)?;
        ctx.emit_event(
            "drive_organizer_started",
            organizer_started_payload(args.apply),
        )
        .await?;
        if !args.apply {
            let proposals = match match &authority {
                DriveAuthority::Trusted => store.organize().await,
                DriveAuthority::Global => store.organize_scoped(None).await,
                DriveAuthority::Project(project) => store.organize_scoped(Some(project)).await,
            } {
                Ok(proposals) => proposals,
                Err(err) => {
                    ctx.emit_event(
                        "drive_organizer_failed",
                        organizer_failed_payload(args.apply, &err.to_string()),
                    )
                    .await?;
                    return Err(drive_error_to_host(err));
                }
            };
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &proposals),
            )
            .await?;
            return serde_json::to_value(proposals)
                .map_err(|err| HostError::HostCall(err.to_string()));
        }

        let mut ids = store
            .pending_proposal_ids()
            .await
            .map_err(drive_error_to_host)?;
        if !matches!(&authority, DriveAuthority::Trusted) {
            ids = authorized_proposal_ids(store, ids, &authority).await?;
        }
        let mut generated = Vec::<OrganizerProposal>::new();
        if ids.is_empty() {
            generated = match match &authority {
                DriveAuthority::Trusted => store.organize().await,
                DriveAuthority::Global => store.organize_scoped(None).await,
                DriveAuthority::Project(project) => store.organize_scoped(Some(project)).await,
            } {
                Ok(proposals) => proposals,
                Err(err) => {
                    ctx.emit_event(
                        "drive_organizer_failed",
                        organizer_failed_payload(args.apply, &err.to_string()),
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
                .map(|proposal| proposal.id)
                .collect();
        }
        if ids.is_empty() {
            emit_drive_write_proposals(ctx, &generated).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &generated),
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
            store
                .mark_proposals_status(&ids, status)
                .await
                .map_err(drive_error_to_host)?;
            let proposals = store
                .proposals()
                .await
                .map_err(drive_error_to_host)?
                .into_iter()
                .filter(|proposal| ids.contains(&proposal.id))
                .collect::<Vec<_>>();
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_failed",
                organizer_failed_payload_with_proposals(args.apply, &err.to_string(), &proposals),
            )
            .await?;
            return Err(err);
        }
        let proposals = match store.apply_organizer_proposals(&ids).await {
            Ok(proposals) => proposals,
            Err(err) => {
                ctx.emit_event(
                    "drive_organizer_failed",
                    organizer_failed_payload(args.apply, &err.to_string()),
                )
                .await?;
                return Err(drive_error_to_host(err));
            }
        };
        if generated.is_empty() {
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &proposals),
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
            organizer_completed_payload(args.apply, &generated),
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

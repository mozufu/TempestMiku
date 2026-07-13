use super::*;

use tm_host::EvolutionTargetClass;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposeSkillRollbackRequest {
    pub expected_active_digest: String,
    pub target_digest: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRollbackResponse {
    pub approval_id: Uuid,
    pub name: String,
    pub expected_active_digest: String,
    pub target_digest: String,
    pub status: String,
}

pub(crate) async fn propose_skill_rollback<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, name)): Path<(Uuid, String)>,
    Json(payload): Json<ProposeSkillRollbackRequest>,
) -> Result<Json<SkillRollbackResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.store.get_session(session_id).await?;
    let managed = state
        .persona
        .managed_skill(&name)
        .map_err(managed_skill_error)?;
    if managed.active.content_digest != payload.expected_active_digest {
        return Err(crate::evolution::policy_error(
            tm_host::EvolutionPolicyReason::StaleApproval,
            format!(
                "managed skill {name} active version changed from {} to {}",
                payload.expected_active_digest, managed.active.content_digest
            ),
        ));
    }
    if payload.expected_active_digest == payload.target_digest {
        return Err(ServerError::InvalidRequest(format!(
            "managed skill {name} is already at {}",
            payload.target_digest
        )));
    }
    let target = managed
        .versions
        .iter()
        .find(|version| version.content_digest == payload.target_digest)
        .ok_or_else(|| {
            ServerError::NotFound(format!(
                "managed skill {name} version {}",
                payload.target_digest
            ))
        })?;
    let target_proposal_id = Uuid::parse_str(&target.source_proposal_id).map_err(|_| {
        ServerError::Store(format!(
            "managed skill {name} target version has invalid source proposal id"
        ))
    })?;
    let rollback = json!({
        "name": name,
        "expectedActiveDigest": payload.expected_active_digest,
        "targetDigest": payload.target_digest,
        "targetProposalId": target_proposal_id,
    });
    let evolution = crate::evolution::evolution_effect_metadata(
        state.self_evolution_tier,
        EvolutionTargetClass::SkillProposal,
        target_proposal_id.to_string(),
        "owner",
        session_id,
        None,
        &rollback,
    )?;
    let timeout = Duration::from_millis(payload.timeout_ms.unwrap_or(60_000).clamp(1, 300_000));
    let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session_id,
        Arc::clone(&state.store),
        state.sender(session_id),
    ));
    sink.emit(
        "write_proposal",
        skill_rollback_payload(&rollback, "pending", None),
    )
    .await?;
    let approval_id = state
        .approval_broker
        .enqueue_permission_for_backend(DurableApprovalSpec {
            session_id,
            origin: "skill-rollback".to_string(),
            prompt: skill_rollback_prompt(&rollback, timeout),
            timeout,
            effect_type: "skill_rollback".to_string(),
            effect_payload_json: json!({
                "evolution": evolution,
                "rollback": rollback,
            }),
            resumable: true,
            sink,
        })
        .await?;
    Ok(Json(SkillRollbackResponse {
        approval_id,
        name,
        expected_active_digest: payload.expected_active_digest,
        target_digest: payload.target_digest,
        status: "pending".to_string(),
    }))
}

pub(crate) fn skill_rollback_payload(
    rollback: &Value,
    status: &str,
    activation: Option<&tm_modes::ManagedSkillActivation>,
) -> Value {
    let mut payload = json!({
        "kind": "skill_rollback",
        "status": status,
        "name": rollback["name"],
        "expectedActiveDigest": rollback["expectedActiveDigest"],
        "targetDigest": rollback["targetDigest"],
        "targetProposalId": rollback["targetProposalId"],
    });
    if let Some(activation) = activation {
        payload["activation"] = serde_json::to_value(activation).unwrap_or(Value::Null);
    }
    payload
}

fn skill_rollback_prompt(rollback: &Value, timeout: Duration) -> ApprovalPrompt {
    ApprovalPrompt {
        action: format!(
            "skill.rollback {}",
            rollback["name"].as_str().unwrap_or_default()
        ),
        scope: json!({
            "kind": "skill_rollback",
            "name": rollback["name"],
            "expectedActiveDigest": rollback["expectedActiveDigest"],
            "targetDigest": rollback["targetDigest"],
            "targetProposalId": rollback["targetProposalId"],
            "timeoutMs": timeout.as_millis(),
        }),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Roll back skill".to_string(),
                kind: "allow_once".to_string(),
            },
            ApprovalOption {
                option_id: "reject".to_string(),
                name: "Keep current version".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}

fn managed_skill_error(error: tm_modes::ManagedSkillError) -> ServerError {
    ServerError::InvalidRequest(error.to_string())
}

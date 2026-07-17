use super::*;

pub async fn run_record_evolution_policy(options: RecordOptions) -> Result<EvidenceManifest> {
    crate::load_dotenv();
    let root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| default_run_dir("evolution-policy"));
    let command = env::args().collect::<Vec<_>>().join(" ");
    let recorder = EvidenceRecorder::create(&root, command)?;
    let result = run_evolution_policy_scenario(&recorder).await;
    let manifest = recorder.finish(result.is_ok())?;
    if let Err(error) = result {
        bail!(
            "tm-e2e record evolution-policy failed: {error}; evidence: {}",
            manifest.run_dir
        );
    }
    Ok(manifest)
}

async fn run_evolution_policy_scenario(recorder: &EvidenceRecorder) -> Result<()> {
    let started_at = timestamp();
    let result = async {
        let conservative = RecordingServer::start_with_tier(
            &recorder.root().join("conservative-server"),
            tm_host::SelfEvolutionTier::Conservative,
        )
        .await?;
        let conservative_client = MikuClient::new(E2eConfig {
            base_url: conservative.base_url.clone(),
            bearer_token: None,
            timeout: Duration::from_secs(30),
        })?
        .with_recorder(recorder.clone());
        let conservative_session = conservative_client.create_session(None).await?;

        let allow_client = conservative_client.clone();
        let allow_session = conservative_session.id.clone();
        let allow = tokio::spawn(async move {
            allow_client
                .propose_profile_fact(
                    &allow_session,
                    "prefers",
                    "bounded evolution evidence",
                    5_000,
                )
                .await
        });
        let (_, approval) = conservative_client
            .wait_for_event(&conservative_session.id, Some(0), |event| {
                event.event_type == "approval" && event.data["backend"] == "memory"
            })
            .await?;
        let approval_id = approval.data["approvalId"]
            .as_str()
            .context("conservative approval id")?;
        conservative_client
            .resolve_approval(&conservative_session.id, approval_id, "approve")
            .await?;
        let allowed = allow
            .await
            .context("joining conservative allow proposal")??;
        ensure!(
            allowed["status"] == "approved",
            "conservative write was not approved"
        );

        let timed_out = conservative_client
            .propose_profile_fact(
                &conservative_session.id,
                "prefers",
                "default deny timeouts",
                25,
            )
            .await?;
        ensure!(
            timed_out["status"] == "timed_out",
            "timeout was not default-deny"
        );
        let denied = conservative_client
            .propose_evolution_review(
                &conservative_session.id,
                json!({
                    "target": { "kind": "mode", "modeId": "serious_engineer" },
                    "changes": [{
                        "section": "description",
                        "before": null,
                        "after": { "label": "Denied", "summary": "Conservative cannot reach this." }
                    }]
                }),
            )
            .await
            .expect_err("conservative moderate target must be denied");
        ensure!(denied.to_string().contains("evolution_insufficient_tier"));

        let (first_skill_session, skill_name, first_skill_digest) = install_dream_skill(
            &conservative,
            &conservative_client,
            "Workflow: when I ask for release notes, gather commits and draft concise notes.",
        )
        .await?;
        let (second_skill_session, second_skill_name, second_skill_digest) = install_dream_skill(
            &conservative,
            &conservative_client,
            "Workflow: when I ask for release notes, gather commits and include upgrade risks.",
        )
        .await?;
        ensure!(
            skill_name == second_skill_name,
            "skill upgrade did not preserve normalized identity"
        );
        ensure!(
            first_skill_digest != second_skill_digest,
            "skill upgrade did not create an immutable second version"
        );
        let rollback = conservative_client
            .propose_skill_rollback(
                &second_skill_session,
                &skill_name,
                &second_skill_digest,
                &first_skill_digest,
            )
            .await?;
        let rollback_approval_id = rollback["approvalId"]
            .as_str()
            .context("skill rollback approval id")?;
        let rollback_recovery = conservative_client
            .session_messages(&second_skill_session)
            .await?;
        let rollback_approval = rollback_recovery["pendingEvents"]
            .as_array()
            .context("skill rollback pending events")?
            .iter()
            .find(|event| {
                event["type"] == "approval" && event["data"]["backend"] == "skill-rollback"
            })
            .context("skill rollback recovery approval")?;
        ensure!(
            rollback_approval["data"]["approvalId"] == rollback_approval_id,
            "skill rollback approval event did not match response"
        );
        conservative_client
            .resolve_approval(&second_skill_session, rollback_approval_id, "approve")
            .await?;
        let rollback_applied = conservative
            .store
            .events_after(Uuid::parse_str(&second_skill_session)?, None)
            .await?
            .into_iter()
            .find(|event| {
                event.event_type == "write_proposal"
                    && event.payload_json["kind"] == "skill_rollback"
                    && event.payload_json["status"] == "approved"
            })
            .context("durable approved skill rollback event")?;
        ensure!(
            rollback_applied.payload_json["activation"]["active"]["contentDigest"]
                == first_skill_digest,
            "skill rollback did not activate the requested immutable version"
        );
        let skill_uri = format!("skill://{skill_name}");
        capture_resource(
            recorder,
            &conservative_client,
            &second_skill_session,
            &skill_uri,
        )
        .await?;
        ensure!(
            conservative
                .persona
                .managed_skill(&skill_name)
                .context("reloading managed skill state")?
                .active
                .content_digest
                == first_skill_digest,
            "managed skill active pointer did not survive catalog reload"
        );

        capture_resource(
            recorder,
            &conservative_client,
            &conservative_session.id,
            "memory://evolution-audits",
        )
        .await?;
        drop(conservative);

        let moderate = RecordingServer::start_with_tier(
            &recorder.root().join("moderate-server"),
            tm_host::SelfEvolutionTier::Moderate,
        )
        .await?;
        recorder.set_server(ServerEvidence {
            base_url: moderate.base_url.clone(),
            artifact_root: moderate.artifact_root.display().to_string(),
            store: "in-memory".to_string(),
            coding_backend: "tm-e2e-evolution-policy-fixture".to_string(),
        });
        let moderate_client = MikuClient::new(E2eConfig {
            base_url: moderate.base_url.clone(),
            bearer_token: None,
            timeout: Duration::from_secs(30),
        })?
        .with_recorder(recorder.clone());
        let session = moderate_client.create_session(None).await?;
        let serious_mode = ModeId::new("serious_engineer");
        let base_profile = moderate
            .persona
            .load_assets()
            .profile_or_unknown(&serious_mode);
        let large_summary = format!("bounded review body {} tail-marker", "x".repeat(3_000));
        let proposal = moderate_client
            .propose_evolution_review(
                &session.id,
                json!({
                    "target": { "kind": "mode", "modeId": "serious_engineer" },
                    "changes": [{
                        "section": "description",
                        "before": {
                            "label": "Current",
                            "summary": "Serious engineering mode."
                        },
                        "after": {
                            "label": "Evidence addendum",
                            "summary": large_summary
                        }
                    }],
                    "timeoutMs": 5_000
                }),
            )
            .await?;
        ensure!(
            proposal["applyEnabled"] == true,
            "moderate mode addendum apply was not enabled"
        );
        let proposal_id = proposal["proposalId"].as_str().context("proposal id")?;
        let approval_id = proposal["approvalId"].as_str().context("approval id")?;
        let resource_uri = proposal["resourceUri"].as_str().context("resource URI")?;
        let (pending_batch, _) = moderate_client
            .wait_for_event(&session.id, Some(0), |event| {
                event.event_type == "write_proposal"
                    && event.data["proposalId"] == proposal_id
                    && event.data["status"] == "pending"
            })
            .await?;
        let pending = pending_batch
            .iter()
            .find(|event| {
                event.event_type == "write_proposal" && event.data["proposalId"] == proposal_id
            })
            .context("pending review event")?;
        ensure!(
            pending.data["preview"].as_str().unwrap_or_default().len() <= 512,
            "review event preview exceeded bound"
        );
        ensure!(
            pending.data.get("changes").is_none(),
            "full diff leaked into event"
        );
        capture_resource(recorder, &moderate_client, &session.id, resource_uri).await?;
        moderate_client
            .resolve_approval(&session.id, approval_id, "approve")
            .await?;
        let duplicate_resolution = moderate_client
            .resolve_approval(&session.id, approval_id, "approve")
            .await
            .expect_err("duplicate resolution must conflict");
        ensure!(
            duplicate_resolution
                .to_string()
                .contains("already resolved")
        );
        let (replay, _) = moderate_client
            .wait_for_event(&session.id, Some(0), |event| {
                event.event_type == "write_proposal"
                    && event.data["proposalId"] == proposal_id
                    && event.data["status"] == "approved"
            })
            .await?;
        let statuses = replay
            .iter()
            .filter(|event| {
                event.event_type == "write_proposal" && event.data["proposalId"] == proposal_id
            })
            .filter_map(|event| event.data["status"].as_str())
            .collect::<Vec<_>>();
        ensure!(
            statuses == ["pending", "approved"],
            "replay lifecycle was not contiguous: {statuses:?}"
        );
        ensure!(
            replay
                .iter()
                .any(|event| event.event_type == "approval_resolved")
        );
        let applied = replay
            .iter()
            .find(|event| {
                event.event_type == "write_proposal"
                    && event.data["proposalId"] == proposal_id
                    && event.data["status"] == "approved"
            })
            .context("approved mode addendum event")?;
        let active_digest = applied.data["activation"]["active"]["contentDigest"]
            .as_str()
            .context("active mode addendum digest")?;
        ensure!(
            moderate
                .persona
                .build_system_prompt(&serious_mode, "base", "", "close this gate")
                .system_prompt
                .contains("tail-marker"),
            "approved mode addendum did not compose on the next prompt"
        );
        ensure!(
            moderate
                .persona
                .load_assets()
                .profile_or_unknown(&serious_mode)
                == base_profile,
            "mode addendum changed the base capability profile"
        );
        let mode_rollback = moderate_client
            .propose_mode_addendum_rollback(&session.id, serious_mode.as_str(), active_digest, None)
            .await?;
        let mode_rollback_approval_id = mode_rollback["approvalId"]
            .as_str()
            .context("mode rollback approval id")?;
        moderate_client
            .resolve_approval(&session.id, mode_rollback_approval_id, "approve")
            .await?;
        let mode_rollback_applied = moderate
            .store
            .events_after(Uuid::parse_str(&session.id)?, None)
            .await?
            .into_iter()
            .find(|event| {
                event.event_type == "write_proposal"
                    && event.payload_json["kind"] == "mode_addendum_rollback"
                    && event.payload_json["status"] == "approved"
            })
            .context("durable approved mode addendum rollback event")?;
        ensure!(
            mode_rollback_applied.payload_json["activation"]["active"].is_null(),
            "mode addendum rollback did not restore the base catalog"
        );
        ensure!(
            !moderate
                .persona
                .build_system_prompt(&serious_mode, "base", "", "close this gate")
                .system_prompt
                .contains("tail-marker"),
            "mode addendum remained composed after rollback"
        );
        capture_resource(
            recorder,
            &moderate_client,
            &session.id,
            "memory://evolution-audits",
        )
        .await?;

        let forged = moderate_client
            .propose_evolution_review(
                &session.id,
                json!({
                    "target": { "kind": "mode", "modeId": "serious_engineer", "path": "SOUL.md" },
                    "changes": []
                }),
            )
            .await
            .expect_err("raw path target must fail deserialization");
        ensure!(forged.to_string().contains("422 Unprocessable Entity"));
        let downgrade = tm_host::decide_evolution_target(
            tm_host::SelfEvolutionTier::Conservative,
            tm_host::EvolutionTargetClass::ModeProposal,
        );
        ensure!(
            downgrade.outcome == tm_host::EvolutionPolicyOutcome::Denied,
            "tier downgrade did not revoke moderate target"
        );
        Ok::<Value, anyhow::Error>(json!({
            "conservativeSessionId": conservative_session.id,
            "moderateSessionId": session.id,
            "proposalId": proposal_id,
            "conservativeAllowed": allowed["status"],
            "skillName": skill_name,
            "skillInstallSessions": [first_skill_session, second_skill_session],
            "skillVersionDigests": [first_skill_digest, second_skill_digest],
            "skillRollback": rollback_applied.payload_json,
            "skillResource": skill_uri,
            "timeoutStatus": timed_out["status"],
            "moderateApplyEnabled": proposal["applyEnabled"],
            "modeAddendumDigest": active_digest,
            "modeAddendumRollback": mode_rollback_applied.payload_json,
            "replayStatuses": statuses,
            "duplicateResolution": "conflict",
            "forgedTarget": "rejected",
            "tierDowngradeDecision": downgrade,
            "largeCandidateResource": resource_uri,
        }))
    }
    .await;
    record_scenario_result(recorder, "evolution-policy", started_at, &result);
    result.map(|_| ())
}

async fn install_dream_skill(
    server: &RecordingServer,
    client: &MikuClient,
    message: &str,
) -> Result<(String, String, String)> {
    let session = client.create_session(None).await?;
    let session_id = Uuid::parse_str(&session.id).context("parsing skill dream session id")?;
    server.run_skill_dream(session_id, message).await?;
    let recovery = client.session_messages(&session.id).await?;
    let pending_events = recovery["pendingEvents"]
        .as_array()
        .context("skill dream pending events")?;
    let approval = pending_events
        .iter()
        .find(|event| event["type"] == "approval" && event["data"]["backend"] == "skill")
        .context("managed skill recovery approval")?;
    let pending = pending_events
        .iter()
        .find(|event| {
            event["type"] == "write_proposal"
                && event["data"]["kind"] == "skill"
                && event["data"]["status"] == "pending"
        })
        .context("pending managed skill proposal")?;
    let name = pending["data"]["name"]
        .as_str()
        .context("managed skill proposal name")?
        .to_string();
    ensure!(
        pending["data"]["installEnabled"] == true,
        "reviewable skill proposal did not expose install authority"
    );
    let approval_id = approval["data"]["approvalId"]
        .as_str()
        .context("managed skill approval id")?;
    client
        .resolve_approval(&session.id, approval_id, "approve")
        .await?;
    let installed = server
        .store
        .events_after(session_id, None)
        .await?
        .into_iter()
        .find(|event| {
            event.event_type == "write_proposal"
                && event.payload_json["kind"] == "skill"
                && event.payload_json["status"] == "approved"
                && event.payload_json.get("installation").is_some()
        })
        .context("durable installed skill event")?;
    let digest = installed.payload_json["installation"]["active"]["contentDigest"]
        .as_str()
        .context("installed managed skill digest")?
        .to_string();
    Ok((session.id, name, digest))
}

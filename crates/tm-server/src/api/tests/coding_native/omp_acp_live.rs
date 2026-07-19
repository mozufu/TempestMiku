use std::{collections::HashSet, path::PathBuf, process::Command, time::Duration};

use super::super::*;
use crate::{OmpAcpBackend, OmpAcpConfig};

/// Opt-in acceptance for the replaceable OMP ACP bridge. This deliberately uses a disposable
/// repository and the public TempestMiku approval route; normal `cargo test` stays network-free.
#[serial_test::serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gated_live_omp_acp_patches_tests_approves_and_replays() {
    if std::env::var("TM_OMP_ACP_LIVE").ok().as_deref() != Some("1") {
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let artifact_root = temp.path().join("artifacts");
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf();
    let clone_status = Command::new("git")
        .args(["clone", "--quiet", "--local", "--no-hardlinks"])
        .arg(&workspace_root)
        .arg(&repo)
        .status()
        .unwrap();
    assert!(clone_status.success(), "could not clone TempestMiku");
    let source_path = repo.join("crates/tm-core/src/shape.rs");
    let baseline_source = std::fs::read_to_string(&source_path).unwrap();
    let broken_source = baseline_source.replacen(
        "pub const DEFAULT_CAP: usize = 8 * 1024;",
        "pub const DEFAULT_CAP: usize = 1;",
        1,
    );
    assert_ne!(broken_source, baseline_source, "fixture source drifted");
    std::fs::write(&source_path, broken_source).unwrap();
    let red = Command::new("cargo")
        .args([
            "test",
            "-p",
            "tm-core",
            "shape::tests::shapes_result_and_stdout",
            "--",
            "--exact",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        !red.status.success(),
        "the disposable TempestMiku regression was not red before OMP"
    );

    let command = std::env::var_os("TM_OMP_ACP_LIVE_COMMAND")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("omp"));
    let version_output = Command::new(&command).arg("--version").output().unwrap();
    assert!(version_output.status.success(), "omp --version failed");
    let stdout = String::from_utf8_lossy(&version_output.stdout)
        .trim()
        .to_string();
    let stderr = String::from_utf8_lossy(&version_output.stderr)
        .trim()
        .to_string();
    let actual_version = if stdout.is_empty() { stderr } else { stdout };
    assert!(!actual_version.is_empty(), "omp --version was empty");
    let expected_version = std::env::var("TM_OMP_ACP_LIVE_EXPECTED_VERSION")
        .expect("TM_OMP_ACP_LIVE_EXPECTED_VERSION must pin the live OMP version");
    assert_eq!(
        actual_version, expected_version,
        "the live OMP binary does not match the acceptance pin"
    );

    let mode_assets = temp.path().join("mode-assets");
    let bundled_mode_assets = workspace_root.join("crates/tm-modes/assets");
    std::fs::create_dir_all(mode_assets.join("skills")).unwrap();
    for file in ["SOUL.md", "modes.json"] {
        std::fs::copy(bundled_mode_assets.join(file), mode_assets.join(file)).unwrap();
    }
    for skill in tm_modes::KNOWN_SKILLS {
        let target_dir = mode_assets.join("skills").join(skill);
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::copy(
            bundled_mode_assets
                .join("skills")
                .join(skill)
                .join("SKILL.md"),
            target_dir.join("SKILL.md"),
        )
        .unwrap();
    }
    use std::io::Write;
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(
                mode_assets
                    .join("skills/serious-engineer-ops/SKILL.md")
            )
            .unwrap(),
        "\n## Live bridge acceptance\nWhen the requested work is complete, include the exact token P0A_SYSTEM_PROMPT_OK in the final response."
    )
    .unwrap();

    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::from_path(mode_assets),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone());
    let backend = OmpAcpBackend::new(
        OmpAcpConfig {
            command,
            expected_version: expected_version.clone(),
            cwd: repo.clone(),
            approval_mode: "always-ask".to_string(),
            profile: std::env::var("TM_OMP_ACP_LIVE_PROFILE").ok(),
            artifact_root,
            approval_timeout: Duration::from_secs(180),
        },
        Arc::clone(&state.approval_broker),
    )
    .unwrap();
    state = state.with_coding_backend(Arc::new(backend));
    let (app, store) = test_app_with_state(state);
    let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    assert_eq!(session.voice_cap, "off");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(message_body(
                    "This is a disposable clone of the real TempestMiku repository. Fix the \
                     regression only in crates/tm-core/src/shape.rs by restoring DEFAULT_CAP from \
                     1 to `8 * 1024`, without changing any test. \
                     Run exactly `cargo test -p tm-core \
                     shape::tests::shapes_result_and_stdout -- --exact` and report its result. \
                     Do not modify any other file or any path outside this repository.",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let turn_id = accepted_turn_id(response).await;

    let mut approvals = HashSet::new();
    let mut final_turn = None;
    for _ in 0..1_800 {
        let events = store.events_after(session.id, None).await.unwrap();
        for approval in events.iter().filter(|event| event.event_type == "approval") {
            let Some(approval_id) = approval.payload_json["approvalId"]
                .as_str()
                .and_then(|value| value.parse::<Uuid>().ok())
            else {
                continue;
            };
            if approvals.insert(approval_id) {
                let allow_option = approval.payload_json["options"]
                    .as_array()
                    .and_then(|options| {
                        options.iter().find(|option| {
                            option["kind"]
                                .as_str()
                                .is_some_and(|kind| kind.starts_with("allow_"))
                        })
                    })
                    .and_then(|option| option["optionId"].as_str())
                    .unwrap_or_else(|| {
                        panic!(
                            "OMP approval has no normalized allow option: {}",
                            approval.payload_json
                        )
                    });
                let resolved = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(Method::POST)
                            .uri(format!("/sessions/{}/approvals/{approval_id}", session.id))
                            .header("content-type", "application/json")
                            .body(Body::from(
                                json!({
                                    "decision": "approve",
                                    "optionId": allow_option,
                                })
                                .to_string(),
                            ))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    resolved.status(),
                    StatusCode::OK,
                    "failed to resolve OMP approval {}",
                    approval.payload_json
                );
            }
        }

        let status = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sessions/{}/turns/{turn_id}", session.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        let turn = response_json(status).await;
        if matches!(turn["status"].as_str(), Some("completed") | Some("failed")) {
            final_turn = Some(turn);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let final_turn = final_turn.expect("live OMP ACP turn did not finish within six minutes");
    assert_eq!(final_turn["status"], json!("completed"), "{final_turn}");
    let precheck_events = store.events_after(session.id, None).await.unwrap();
    let precheck_final = precheck_events
        .iter()
        .rev()
        .find(|event| event.event_type == "final")
        .and_then(|event| event.payload_json["text"].as_str())
        .unwrap_or("<missing final text>");
    assert!(
        !approvals.is_empty(),
        "OMP ACP did not exercise the TempestMiku approval route; final response: \
         {precheck_final}; event types: {:?}",
        precheck_events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>()
    );

    let source = std::fs::read_to_string(&source_path).unwrap();
    assert_eq!(
        source, baseline_source,
        "OMP did not restore the exact source; final response: {precheck_final}"
    );
    let diff_names = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(diff_names.status.success());
    assert_eq!(
        String::from_utf8_lossy(&diff_names.stdout).trim(),
        "",
        "OMP left changes outside the exact repair"
    );
    let targeted = Command::new("cargo")
        .args([
            "test",
            "-p",
            "tm-core",
            "shape::tests::shapes_result_and_stdout",
            "--",
            "--exact",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        targeted.status.success(),
        "targeted test failed after OMP patch:\n{}\n{}",
        String::from_utf8_lossy(&targeted.stdout),
        String::from_utf8_lossy(&targeted.stderr)
    );

    let events = store.events_after(session.id, None).await.unwrap();
    let event_types = events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect::<Vec<_>>();
    for required in ["approval", "approval_resolved", "diff", "artifact", "final"] {
        assert!(
            event_types.contains(&required),
            "missing {required}: {event_types:?}"
        );
    }
    for resolution in events
        .iter()
        .filter(|event| event.event_type == "approval_resolved")
    {
        assert_eq!(
            resolution.payload_json["status"],
            json!("approved"),
            "OMP permission did not resolve as approved: {}",
            resolution.payload_json
        );
        assert_eq!(resolution.payload_json["outcome"], json!("selected"));
    }
    let final_text = events
        .iter()
        .rev()
        .find(|event| event.event_type == "final")
        .and_then(|event| event.payload_json["text"].as_str())
        .expect("OMP final event is missing text");
    assert!(
        final_text.contains("P0A_SYSTEM_PROMPT_OK"),
        "the OMP final did not obey the TempestMiku system prompt: {final_text}"
    );
    assert!(
        final_text.contains("Evidence: artifact://"),
        "the OMP final did not retain transcript provenance: {final_text}"
    );
    let first_seq = events.first().unwrap().seq;

    let artifacts = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/resources/artifacts", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(artifacts.status(), StatusCode::OK);
    let artifacts = response_json(artifacts).await;
    let artifact_count = artifacts.as_array().map_or(0, Vec::len);
    assert!(artifact_count > 0);

    let ended = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/end", session.id))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ended.status(), StatusCode::OK);
    let replay = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/events", session.id))
                .header("last-event-id", first_seq.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay = tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(replay.into_body(), 8 * 1024 * 1024),
    )
    .await
    .expect("Last-Event-ID replay did not close after session end")
    .unwrap();
    let replay = String::from_utf8(replay.to_vec()).unwrap();
    for required in [
        "approval_resolved",
        "diff",
        "artifact",
        "final",
        "session_end",
    ] {
        assert!(
            replay.contains(&format!(r#""type":"{required}""#)),
            "Last-Event-ID replay is missing {required}: {replay}"
        );
    }

    let source_revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(source_revision.status.success());
    let source_revision = String::from_utf8_lossy(&source_revision.stdout)
        .trim()
        .to_string();

    let evidence = json!({
        "schema": 1,
        "gate": "p0a-omp-acp-live",
        "ompVersion": expected_version,
        "tempestMikuRevision": source_revision,
        "approvalsResolvedThroughPublicApi": approvals.len(),
        "eventTypes": event_types,
        "artifactCount": artifact_count,
        "lastEventIdReplayPassed": true,
        "systemPromptMarkerObserved": true,
        "targetedTest": "cargo test -p tm-core shape::tests::shapes_result_and_stdout -- --exact",
        "targetedTestPassed": true,
        "exactSourceRestored": true,
        "changedFilesAfterRepair": [],
    });
    let evidence_path = std::env::var_os("TM_OMP_ACP_LIVE_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("target/omp-acp-live-evidence.json"));
    if let Some(parent) = evidence_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
}

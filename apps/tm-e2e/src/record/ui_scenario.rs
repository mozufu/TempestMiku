use super::*;

pub(super) async fn run_ui_scenario(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    base_url: &str,
    options: &RecordOptions,
) -> Result<()> {
    let started_at = timestamp();
    let result = async {
        if !options.skip_flutter_build
            && env::var("TM_E2E_SKIP_FLUTTER_BUILD").ok().as_deref() != Some("1")
        {
            run_command_capture(
                "flutter",
                &["build", "web"],
                Path::new("clients/miku_flutter"),
                &recorder.root().join("ui/flutter-build.stdout.log"),
                &recorder.root().join("ui/flutter-build.stderr.log"),
            )
            .await
            .context("building Flutter web client")?;
            recorder.add_artifact(
                "flutter build stdout",
                recorder.root().join("ui/flutter-build.stdout.log"),
            )?;
            recorder.add_artifact(
                "flutter build stderr",
                recorder.root().join("ui/flutter-build.stderr.log"),
            )?;
        }

        let ui = run_playwright(recorder, base_url, options.headed).await?;
        let session_id = read_ui_session_id(&recorder.root().join("ui/ui-result.json"))
            .ok()
            .flatten();
        recorder.record_ui(ui.clone());
        if let Some(session_id) = session_id {
            let _ = capture_resource(recorder, client, &session_id, "artifact://0").await;
            Ok::<Value, anyhow::Error>(json!({
                "sessionId": session_id,
                "ui": ui,
            }))
        } else {
            Ok::<Value, anyhow::Error>(json!({ "ui": ui }))
        }
    }
    .await;
    record_scenario_result(recorder, "ui-remote-control", started_at, &result);
    result.map(|_| ())
}

pub(super) async fn capture_resource(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    session_id: &str,
    uri: &str,
) -> Result<()> {
    let preview = client
        .preview_resource(session_id, uri)
        .await
        .with_context(|| format!("previewing resource {uri}"))?;
    let resolved = client
        .resolve_resource(session_id, uri)
        .await
        .with_context(|| format!("resolving resource {uri}"))?;
    recorder.record_resource(session_id, uri, &preview, &resolved)?;
    Ok(())
}

pub(super) fn record_scenario_result(
    recorder: &EvidenceRecorder,
    name: &str,
    started_at: String,
    result: &Result<Value>,
) {
    recorder.record_scenario(crate::RecordedScenario {
        name: name.to_string(),
        ok: result.is_ok(),
        started_at,
        finished_at: timestamp(),
        details: result
            .as_ref()
            .cloned()
            .unwrap_or_else(|_| json!({ "status": "failed" })),
        error: result.as_ref().err().map(|err| err.to_string()),
    });
}

async fn run_playwright(
    recorder: &EvidenceRecorder,
    base_url: &str,
    headed: bool,
) -> Result<UiEvidence> {
    let root = recorder.root();
    fs::create_dir_all(root.join("ui")).context("creating UI evidence dir")?;
    let stdout = root.join("ui/playwright.stdout.log");
    let stderr = root.join("ui/playwright.stderr.log");
    let mut args = vec![
        "exec",
        "playwright",
        "--",
        "test",
        "-c",
        "evidence.config.ts",
    ];
    if headed {
        args.push("--headed");
    }
    let mut command = Command::new("npm");
    command
        .args(&args)
        .current_dir("clients/miku_web")
        .env("TM_E2E_BASE_URL", base_url)
        .env("TM_E2E_RUN_DIR", &root);
    if headed {
        command.env("TM_E2E_HEADED", "1");
    }
    let output = command
        .output()
        .await
        .context("running Playwright evidence test through npm")?;
    fs::write(&stdout, &output.stdout).context("writing Playwright stdout")?;
    fs::write(&stderr, &output.stderr).context("writing Playwright stderr")?;

    let result_path = root.join("ui/ui-result.json");
    let playwright_json = root.join("ui/playwright-report.json");
    let mut ui = read_ui_evidence(&root).unwrap_or_default();
    ui.ok = output.status.success() && ui.ok;
    ui.result_path = existing_relative(&root, &result_path);
    ui.playwright_json_path = existing_relative(&root, &playwright_json);
    ui.stdout_path = existing_relative(&root, &stdout);
    ui.stderr_path = existing_relative(&root, &stderr);
    ui.console_path = existing_relative(&root, &root.join("ui/console.ndjson"));
    ui.network_path = existing_relative(&root, &root.join("ui/network.ndjson"));
    ui.artifacts = collect_ui_artifacts(&root)?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        ui.error = Some(if err.is_empty() {
            "Playwright evidence test failed".to_string()
        } else {
            err
        });
        recorder.record_ui(ui.clone());
        bail!("Playwright evidence test failed");
    }
    if !ui.ok {
        ui.error
            .get_or_insert_with(|| "Playwright evidence result did not report ok=true".to_string());
        recorder.record_ui(ui.clone());
        bail!("Playwright evidence result did not report ok=true");
    }
    Ok(ui)
}

async fn run_command_capture(
    program: &str,
    args: &[&str],
    cwd: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| format!("running {program} {}", args.join(" ")))?;
    fs::write(stdout_path, &output.stdout)
        .with_context(|| format!("writing {}", stdout_path.display()))?;
    fs::write(stderr_path, &output.stderr)
        .with_context(|| format!("writing {}", stderr_path.display()))?;
    ensure!(
        output.status.success(),
        "{program} {} failed; see {} and {}",
        args.join(" "),
        stdout_path.display(),
        stderr_path.display()
    );
    Ok(())
}

fn existing_relative(root: &Path, path: &Path) -> Option<String> {
    path.exists().then(|| {
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    })
}

fn collect_ui_artifacts(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_files(&root.join("ui"), root, &mut files)?;
    files.sort();
    Ok(files
        .into_iter()
        .filter(|path| {
            !matches!(
                path.as_str(),
                "ui/ui-result.json"
                    | "ui/playwright-report.json"
                    | "ui/console.ndjson"
                    | "ui/network.ndjson"
                    | "ui/playwright.stdout.log"
                    | "ui/playwright.stderr.log"
            ) && !path.starts_with("ui/playwright-html/trace/")
        })
        .collect())
}

fn collect_files(dir: &Path, root: &Path, out: &mut Vec<String>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, root, out)?;
        } else {
            out.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiResultFile {
    ok: bool,
    session_id: Option<String>,
    screenshot_path: Option<String>,
    error: Option<String>,
}

fn read_ui_evidence(root: &Path) -> Result<UiEvidence> {
    let result_path = root.join("ui/ui-result.json");
    let result = read_ui_result(&result_path)?;
    Ok(UiEvidence {
        ok: result.ok,
        result_path: existing_relative(root, &result_path),
        screenshot_path: result
            .screenshot_path
            .as_deref()
            .map(Path::new)
            .and_then(|path| existing_relative(root, path)),
        error: result.error,
        ..UiEvidence::default()
    })
}

fn read_ui_session_id(path: &Path) -> Result<Option<String>> {
    Ok(read_ui_result(path)?.session_id)
}

fn read_ui_result(path: &Path) -> Result<UiResultFile> {
    let bytes = fs::read(path).with_context(|| format!("reading UI result {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decoding UI result {}", path.display()))
}

use std::{
    fs,
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::model::{EvidenceManifest, GitEvidence};

pub(super) fn git_evidence() -> GitEvidence {
    let revision = command_stdout("git", &["rev-parse", "HEAD"]);
    let status_short = command_stdout("git", &["status", "--short"])
        .map(|text| text.lines().map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default();
    GitEvidence {
        revision,
        dirty: !status_short.is_empty(),
        status_short,
    }
}

fn command_stdout(cmd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(cmd).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|text| !text.is_empty())
}

pub(super) fn ndjson_file(path: impl AsRef<Path>) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path.as_ref())
        .with_context(|| format!("opening evidence ndjson {}", path.as_ref().display()))
}

pub(super) fn write_ndjson<T: Serialize>(file: &mut File, value: &T) -> Result<()> {
    serde_json::to_writer(&mut *file, value).context("encoding ndjson record")?;
    writeln!(file).context("terminating ndjson record")?;
    Ok(())
}

pub(super) fn write_json_file(path: impl AsRef<Path>, value: &impl Serialize) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating evidence directory {}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(value).context("encoding evidence JSON")?;
    fs::write(path, json).with_context(|| format!("writing evidence JSON {}", path.display()))
}

pub(super) fn write_transcript(root: &Path, transcript: &[String]) -> Result<()> {
    let mut text = transcript.join("\n");
    text.push('\n');
    fs::write(root.join("transcript.md"), text)
        .with_context(|| format!("writing transcript {}", root.display()))
}

pub(super) fn write_report(root: &Path, manifest: &EvidenceManifest) -> Result<()> {
    let mut lines = Vec::new();
    lines.push("# TempestMiku E2E Evidence Report".to_string());
    lines.push(String::new());
    lines.push(format!(
        "- Status: **{}**",
        if manifest.ok { "PASS" } else { "FAIL" }
    ));
    lines.push(format!("- Started: {}", manifest.started_at));
    lines.push(format!("- Finished: {}", manifest.finished_at));
    lines.push(format!("- Command: `{}`", manifest.command));
    lines.push(String::new());
    lines.push("## Scenarios".to_string());
    for scenario in &manifest.scenarios {
        lines.push(format!(
            "- `{}`: {}",
            scenario.name,
            if scenario.ok { "PASS" } else { "FAIL" }
        ));
    }
    lines.push(String::new());
    lines.push("## Evidence Files".to_string());
    for path in [
        "manifest.json",
        "events.ndjson",
        "http.ndjson",
        "transcript.md",
    ] {
        lines.push(format!("- [{}]({})", path, path));
    }
    for resource in &manifest.resources {
        lines.push(format!(
            "- `{}`: [{}]({}), [{}]({})",
            resource.uri,
            resource.preview_path,
            resource.preview_path,
            resource.resolve_path,
            resource.resolve_path
        ));
    }
    if let Some(ui) = &manifest.ui {
        lines.push(String::new());
        lines.push("## UI".to_string());
        for path in [
            ui.result_path.as_deref(),
            ui.playwright_json_path.as_deref(),
            ui.screenshot_path.as_deref(),
            ui.console_path.as_deref(),
            ui.network_path.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            lines.push(format!("- [{}]({})", path, path));
        }
        for path in &ui.artifacts {
            lines.push(format!("- [{}]({})", path, path));
        }
    }
    lines.push(String::new());
    fs::write(root.join("report.md"), lines.join("\n"))
        .with_context(|| format!("writing report {}", root.display()))
}

pub(super) fn write_index(root: &Path, manifest: &EvidenceManifest) -> Result<()> {
    let scenario_rows = manifest
        .scenarios
        .iter()
        .map(|scenario| {
            format!(
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                escape_html(&scenario.name),
                if scenario.ok { "PASS" } else { "FAIL" },
                escape_html(scenario.error.as_deref().unwrap_or(""))
            )
        })
        .collect::<String>();
    let html = format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>TempestMiku E2E Evidence</title>
  <style>
    body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; margin: 32px; line-height: 1.5; }}
    code {{ background: #f2f4f7; padding: 2px 5px; border-radius: 4px; }}
    table {{ border-collapse: collapse; width: 100%; max-width: 920px; }}
    td, th {{ border-bottom: 1px solid #d0d7de; padding: 8px; text-align: left; }}
  </style>
</head>
<body>
  <h1>TempestMiku E2E Evidence</h1>
  <p>Status: <strong>{}</strong></p>
  <p>Started: <code>{}</code><br>Finished: <code>{}</code></p>
  <p><a href="report.md">Report</a> · <a href="manifest.json">Manifest</a> · <a href="events.ndjson">SSE events</a> · <a href="http.ndjson">HTTP log</a> · <a href="transcript.md">Transcript</a></p>
  <h2>Scenarios</h2>
  <table><thead><tr><th>Name</th><th>Status</th><th>Error</th></tr></thead><tbody>{}</tbody></table>
</body>
</html>
"#,
        if manifest.ok { "PASS" } else { "FAIL" },
        escape_html(&manifest.started_at),
        escape_html(&manifest.finished_at),
        scenario_rows
    );
    fs::write(root.join("index.html"), html)
        .with_context(|| format!("writing evidence index {}", root.display()))
}

pub(super) fn validate_manifest_paths(root: &Path, manifest: &EvidenceManifest) -> Result<()> {
    for path in [
        "manifest.json",
        "events.ndjson",
        "http.ndjson",
        "transcript.md",
        "report.md",
        "index.html",
    ] {
        ensure!(
            root.join(path).exists(),
            "evidence file {path} was not written"
        );
    }
    for path in manifest.artifacts.values() {
        ensure!(
            root.join(path).exists(),
            "declared artifact {path} is missing"
        );
    }
    for resource in &manifest.resources {
        ensure!(
            root.join(&resource.preview_path).exists(),
            "declared resource preview {} is missing",
            resource.preview_path
        );
        ensure!(
            root.join(&resource.resolve_path).exists(),
            "declared resource resolve {} is missing",
            resource.resolve_path
        );
    }
    if let Some(ui) = &manifest.ui {
        for path in [
            ui.result_path.as_deref(),
            ui.playwright_json_path.as_deref(),
            ui.screenshot_path.as_deref(),
            ui.console_path.as_deref(),
            ui.network_path.as_deref(),
            ui.stdout_path.as_deref(),
            ui.stderr_path.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            ensure!(
                root.join(path).exists(),
                "declared UI file {path} is missing"
            );
        }
        for path in &ui.artifacts {
            ensure!(
                root.join(path).exists(),
                "declared UI artifact {path} is missing"
            );
        }
    }
    Ok(())
}

pub(super) fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub(super) fn resource_slug(session_id: &str, uri: &str, index: usize) -> String {
    let mut slug = format!("{index:03}-{session_id}-{}", uri.replace("://", "-"));
    slug = slug
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    slug.trim_matches('-').to_string()
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

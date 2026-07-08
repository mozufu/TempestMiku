use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::{
    DriveEntry, DriveEntryStatus, DriveEvidence, DrivePutOptions, OrganizerActionKind,
    OrganizerProposal, PolicyDecision, ProposalStatus, Transduction,
};

const DEFAULT_PROJECT_CONVENTION: &str = "projects/{project}/{docKind}/{filename}";
const DEFAULT_FINANCE_CONVENTION: &str = "finance/{year}/{docKind}/{filename}";
const DEFAULT_INBOX_CONVENTION: &str = "inbox/{date}/{filename}";

pub fn propose_path(
    transduction: &Transduction,
    options: &DrivePutOptions,
    filename_hint: Option<&str>,
) -> String {
    if let Some(path) = options
        .suggested_path
        .as_ref()
        .filter(|path| !path.trim().is_empty())
    {
        return path.clone();
    }

    let title = transduction
        .title
        .as_deref()
        .or_else(|| filename_hint.and_then(|name| name.rsplit('/').next()))
        .unwrap_or("untitled");
    let filename = title_filename(title, filename_hint, &transduction.mime);
    let doc_kind = transduction
        .doc_kind
        .as_deref()
        .filter(|kind| !kind.is_empty())
        .unwrap_or("docs");
    let title_slug = slug(title);
    let doc_kind_slug = slug(doc_kind);

    if let Some(project) = transduction
        .project
        .as_ref()
        .or(options.project.as_ref())
        .filter(|project| !project.trim().is_empty())
    {
        let project_slug = slug(project);
        return render_path_template(
            template_or_default(
                options.conventions.project.as_deref(),
                DEFAULT_PROJECT_CONVENTION,
            ),
            &[
                ("project", project_slug),
                ("docKind", doc_kind_slug),
                ("title", title_slug),
                ("filename", filename),
            ],
        );
    }

    if matches!(doc_kind, "invoice" | "receipt") {
        let year = transduction
            .dates
            .first()
            .and_then(|date| date.get(0..4))
            .and_then(|year| year.parse::<i32>().ok())
            .unwrap_or_else(|| Utc::now().year());
        return render_path_template(
            template_or_default(
                options.conventions.finance.as_deref(),
                DEFAULT_FINANCE_CONVENTION,
            ),
            &[
                ("year", year.to_string()),
                ("docKind", doc_kind_slug),
                ("title", title_slug),
                ("filename", filename),
            ],
        );
    }

    let today = Utc::now().date_naive();
    render_path_template(
        template_or_default(
            options.conventions.inbox.as_deref(),
            DEFAULT_INBOX_CONVENTION,
        ),
        &[
            ("date", today.format("%Y-%m-%d").to_string()),
            ("docKind", doc_kind_slug),
            ("title", title_slug),
            ("filename", filename),
        ],
    )
}

pub fn title_filename(title: &str, filename_hint: Option<&str>, mime: &str) -> String {
    let extension = filename_hint
        .and_then(|name| name.rsplit('/').next())
        .and_then(|name| {
            name.rsplit_once('.')
                .map(|(_, ext)| ext.to_ascii_lowercase())
        })
        .or_else(|| extension_for_mime(mime).map(str::to_string))
        .unwrap_or_else(|| "txt".to_string());
    format!("{}.{}", slug(title), extension.trim_start_matches('.'))
}

pub fn slug(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

pub fn apply_tags(existing: &[String], new_tags: &[String]) -> Vec<String> {
    let mut tags = BTreeSet::new();
    for tag in existing.iter().chain(new_tags.iter()) {
        let tag = tag.trim().trim_start_matches('#').to_ascii_lowercase();
        if !tag.is_empty() {
            tags.insert(tag);
        }
    }
    tags.into_iter().collect()
}

fn template_or_default<'a>(template: Option<&'a str>, default: &'static str) -> &'a str {
    template
        .filter(|template| !template.trim().is_empty())
        .unwrap_or(default)
}

fn render_path_template(template: &str, values: &[(&str, String)]) -> String {
    let mut rendered = template.trim().to_string();
    for (key, value) in values {
        rendered = rendered.replace(&format!("{{{key}}}"), value);
    }
    rendered.trim_matches('/').to_string()
}

pub fn generate_organizer_proposals(entries: &[DriveEntry]) -> Vec<OrganizerProposal> {
    generate_organizer_proposals_for_run(entries, Uuid::new_v4())
}

pub fn generate_organizer_proposals_for_run(
    entries: &[DriveEntry],
    source_run_id: Uuid,
) -> Vec<OrganizerProposal> {
    entries
        .iter()
        .filter(|entry| entry.status == DriveEntryStatus::Active)
        .filter_map(|entry| proposal_for_entry(entry, source_run_id))
        .collect()
}

fn proposal_for_entry(entry: &DriveEntry, source_run_id: Uuid) -> Option<OrganizerProposal> {
    let desired_path = proposed_path_for_entry(entry);
    if desired_path == entry.path {
        return None;
    }
    let now = Utc::now();
    let mut replay_metadata = BTreeMap::new();
    replay_metadata.insert("contentHash".to_string(), json!(entry.content_hash));
    replay_metadata.insert("sourceUri".to_string(), json!(entry.uri));

    Some(OrganizerProposal {
        id: Uuid::new_v4(),
        action: OrganizerActionKind::Move,
        entry_id: entry.id,
        source_path: entry.path.clone(),
        proposed_path: Some(desired_path),
        proposed_tags: Vec::new(),
        proposed_doc_kind: entry.doc_kind.clone(),
        proposed_project: entry.project.clone(),
        evidence: vec![DriveEvidence {
            snippet: entry
                .summary
                .clone()
                .or_else(|| entry.title.clone())
                .unwrap_or_else(|| entry.path.clone()),
            selector: None,
        }],
        confidence: 0.72,
        policy_decision: PolicyDecision::ApprovalRequired,
        approval_id: None,
        status: ProposalStatus::Pending,
        source_run_id,
        replay_metadata,
        created_at: now,
        updated_at: now,
    })
}

fn proposed_path_for_entry(entry: &DriveEntry) -> String {
    let filename = title_filename(
        entry.title.as_deref().unwrap_or(&entry.path),
        Some(&entry.path),
        &entry.mime,
    );
    let doc_kind = entry.doc_kind.as_deref().unwrap_or("docs");
    if let Some(project) = entry
        .project
        .as_deref()
        .filter(|project| !project.is_empty())
    {
        return format!("projects/{}/{}/{}", slug(project), slug(doc_kind), filename);
    }
    if matches!(doc_kind, "invoice" | "receipt") {
        let year = entry
            .dates
            .first()
            .and_then(|date| date.get(0..4))
            .and_then(|year| year.parse::<i32>().ok())
            .unwrap_or_else(|| Utc::now().year());
        return format!("finance/{}/{}/{}", year, slug(doc_kind), filename);
    }
    entry.path.clone()
}

fn extension_for_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "text/markdown" => Some("md"),
        "text/plain" => Some("txt"),
        "application/json" => Some("json"),
        "text/csv" => Some("csv"),
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "application/pdf" => Some("pdf"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::{DriveConventions, DrivePutOptions, TransducerInput, transduce_document};

    use super::*;

    #[test]
    fn project_docs_land_under_project_kind_title() {
        let opts = DrivePutOptions {
            auto: true,
            project: Some("Tempest Miku".to_string()),
            ..DrivePutOptions::default()
        };
        let tx = transduce_document(TransducerInput {
            bytes: b"# P5 Plan\nRoadmap milestone notes",
            filename: Some("plan.md"),
            options: &opts,
        })
        .unwrap();

        assert_eq!(
            propose_path(&tx, &opts, Some("plan.md")),
            "projects/tempest-miku/project-doc/p5-plan.md"
        );
    }

    #[test]
    fn custom_conventions_can_override_project_layout() {
        let opts = DrivePutOptions {
            auto: true,
            project: Some("Tempest Miku".to_string()),
            conventions: DriveConventions {
                project: Some("work/{project}/{docKind}/{filename}".to_string()),
                ..DriveConventions::default()
            },
            ..DrivePutOptions::default()
        };
        let tx = transduce_document(TransducerInput {
            bytes: b"# P5 Plan\nRoadmap milestone notes",
            filename: Some("plan.md"),
            options: &opts,
        })
        .unwrap();

        assert_eq!(
            propose_path(&tx, &opts, Some("plan.md")),
            "work/tempest-miku/project-doc/p5-plan.md"
        );
    }

    #[test]
    fn invoice_without_project_lands_under_finance() {
        let opts = DrivePutOptions::default();
        let tx = transduce_document(TransducerInput {
            bytes: b"Invoice\nDate 2026-07-08\nAmount due $12.00",
            filename: Some("invoice.txt"),
            options: &opts,
        })
        .unwrap();

        assert_eq!(
            propose_path(&tx, &opts, Some("invoice.txt")),
            "finance/2026/invoice/invoice.txt"
        );
    }
}

use serde_json::{Value, json};

use crate::{DriveEntry, DriveLinkPlan, DriveUnlinkResult, OrganizerActionKind, OrganizerProposal};

pub(crate) fn organizer_started_payload(apply: bool) -> Value {
    json!({
        "apply": apply,
    })
}

pub(crate) fn organizer_completed_payload(apply: bool, proposals: &[OrganizerProposal]) -> Value {
    json!({
        "apply": apply,
        "runId": proposals.first().map(|proposal| proposal.source_run_id),
        "proposalCount": proposals.len(),
        "proposals": proposals.iter().map(organizer_proposal_event_payload).collect::<Vec<_>>(),
        "resourceRefs": proposals.iter().flat_map(organizer_resource_refs).collect::<Vec<_>>(),
    })
}

pub(crate) fn organizer_failed_payload(apply: bool, error: &str) -> Value {
    organizer_failed_payload_with_proposals(apply, error, &[])
}

pub(crate) fn organizer_failed_payload_with_proposals(
    apply: bool,
    error: &str,
    proposals: &[OrganizerProposal],
) -> Value {
    json!({
        "apply": apply,
        "error": error,
        "runId": proposals.first().map(|proposal| proposal.source_run_id),
        "proposalCount": proposals.len(),
        "proposals": proposals.iter().map(organizer_proposal_event_payload).collect::<Vec<_>>(),
        "resourceRefs": proposals.iter().flat_map(organizer_resource_refs).collect::<Vec<_>>(),
    })
}

pub(crate) fn drive_write_proposal_payload(proposal: &OrganizerProposal) -> Value {
    let mut payload = organizer_proposal_event_payload(proposal);
    if let Value::Object(map) = &mut payload {
        map.insert("kind".to_string(), json!("drive"));
        map.insert("proposalId".to_string(), json!(proposal.id));
        map.insert("driveProposal".to_string(), json!(proposal));
    }
    payload
}

pub(crate) fn drive_entry_event_payload(action: &str, entry: &DriveEntry) -> Value {
    let title = match action {
        "put" => "Filed drive document",
        "tag" => "Tagged drive document",
        _ => "Updated drive document",
    };
    drive_entry_payload(action, title, &entry.path, entry)
}

pub(crate) fn drive_moved_payload(from_path: &str, entry: &DriveEntry) -> Value {
    let mut payload = drive_entry_payload(
        "move",
        "Moved drive document",
        &format!("{from_path} -> {}", entry.path),
        entry,
    );
    if let Value::Object(map) = &mut payload {
        map.insert("fromPath".to_string(), json!(from_path));
        map.insert(
            "fromUri".to_string(),
            json!(DriveEntry::drive_uri(from_path)),
        );
        map.insert("toPath".to_string(), json!(&entry.path));
        map.insert("toUri".to_string(), json!(&entry.uri));
        map.insert(
            "resourceRefs".to_string(),
            json!([
                {
                    "role": "previous",
                    "uri": DriveEntry::drive_uri(from_path),
                    "kind": "drive_document",
                    "title": drive_path_title(from_path),
                },
                drive_entry_resource_ref("current", entry),
            ]),
        );
    }
    payload
}

pub(crate) fn drive_entry_payload(
    action: &str,
    preview_title: &str,
    preview_subtitle: &str,
    entry: &DriveEntry,
) -> Value {
    json!({
        "action": action,
        "entryId": entry.id,
        "path": &entry.path,
        "uri": &entry.uri,
        "title": &entry.title,
        "docKind": &entry.doc_kind,
        "project": &entry.project,
        "tags": &entry.tags,
        "mime": &entry.mime,
        "sizeBytes": entry.size_bytes,
        "contentHash": &entry.content_hash,
        "sourceUri": &entry.source_uri,
        "status": entry.status,
        "preview": {
            "title": preview_title,
            "subtitle": compact_preview_text(preview_subtitle, 160),
            "snippet": drive_entry_snippet(entry),
        },
        "resourceRefs": [drive_entry_resource_ref("document", entry)],
    })
}

pub(crate) fn drive_entry_resource_ref(role: &str, entry: &DriveEntry) -> Value {
    json!({
        "role": role,
        "uri": &entry.uri,
        "kind": "drive_document",
        "title": drive_entry_title(entry),
        "path": &entry.path,
    })
}

pub(crate) fn drive_entry_title(entry: &DriveEntry) -> String {
    entry
        .title
        .clone()
        .unwrap_or_else(|| drive_path_title(&entry.path))
}

pub(crate) fn drive_path_title(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

pub(crate) fn drive_entry_snippet(entry: &DriveEntry) -> Option<String> {
    entry
        .summary
        .as_deref()
        .or_else(|| {
            entry.attributes.iter().find_map(|attribute| {
                attribute
                    .evidence
                    .as_ref()
                    .map(|evidence| evidence.snippet.as_str())
            })
        })
        .map(|snippet| compact_preview_text(snippet, 180))
}

pub(crate) fn project_linked_payload(plan: &DriveLinkPlan) -> Value {
    json!({
        "action": "link",
        "alias": &plan.alias,
        "linkedUri": &plan.linked_uri,
        "mode": &plan.mode,
        "project": &plan.project,
        "memoryScope": &plan.memory_scope,
        "requiresApproval": plan.requires_approval,
        "preview": {
            "title": "Linked project folder",
            "subtitle": compact_preview_text(&format!("{} -> {}", plan.project, plan.linked_uri), 160),
            "snippet": compact_preview_text(&plan.canonical_root, 180),
        },
        "resourceRefs": [{
            "role": "linked",
            "uri": &plan.linked_uri,
            "kind": "linked_folder",
            "title": &plan.project,
        }],
    })
}

pub(crate) fn project_unlinked_payload(result: &DriveUnlinkResult) -> Value {
    json!({
        "action": "unlink",
        "alias": &result.alias,
        "linkedUri": &result.linked_uri,
        "memoryScope": &result.memory_scope,
        "revokedAt": result.revoked_at,
        "preview": {
            "title": "Unlinked project folder",
            "subtitle": compact_preview_text(&result.linked_uri, 160),
            "snippet": compact_preview_text(&result.canonical_root, 180),
        },
        "resourceRefs": [{
            "role": "revoked",
            "uri": &result.linked_uri,
            "kind": "linked_folder",
            "title": &result.alias,
        }],
    })
}

pub(crate) fn organizer_proposal_event_payload(proposal: &OrganizerProposal) -> Value {
    json!({
        "proposalId": proposal.id,
        "runId": proposal.source_run_id,
        "action": proposal.action,
        "status": proposal.status,
        "policyDecision": proposal.policy_decision,
        "sourcePath": proposal.source_path,
        "sourceUri": DriveEntry::drive_uri(&proposal.source_path),
        "proposedPath": proposal.proposed_path,
        "proposedUri": proposal.proposed_path.as_deref().map(DriveEntry::drive_uri),
        "proposedTags": proposal.proposed_tags,
        "proposedDocKind": proposal.proposed_doc_kind,
        "proposedProject": proposal.proposed_project,
        "confidence": proposal.confidence,
        "preview": organizer_preview(proposal),
        "resourceRefs": organizer_resource_refs(proposal),
    })
}

pub(crate) fn organizer_resource_refs(proposal: &OrganizerProposal) -> Vec<Value> {
    let mut refs = vec![json!({
        "role": "source",
        "uri": DriveEntry::drive_uri(&proposal.source_path),
        "kind": "drive_document",
        "title": proposal.source_path.rsplit('/').next().unwrap_or(&proposal.source_path),
    })];
    if let Some(path) = proposal.proposed_path.as_deref() {
        refs.push(json!({
            "role": "proposed",
            "uri": DriveEntry::drive_uri(path),
            "kind": "drive_document",
            "title": path.rsplit('/').next().unwrap_or(path),
        }));
    }
    refs
}

pub(crate) fn organizer_preview(proposal: &OrganizerProposal) -> Value {
    let title = match proposal.action {
        OrganizerActionKind::Move => "Move drive document",
        OrganizerActionKind::Tag => "Tag drive document",
        OrganizerActionKind::Dedupe => "Deduplicate drive document",
        OrganizerActionKind::Archive => "Archive drive document",
        OrganizerActionKind::SetDocKind => "Set document kind",
        OrganizerActionKind::SetProject => "Set project",
    };
    let subtitle = proposal
        .proposed_path
        .as_ref()
        .map(|path| format!("{} -> {path}", proposal.source_path))
        .unwrap_or_else(|| proposal.source_path.clone());
    json!({
        "title": title,
        "subtitle": compact_preview_text(&subtitle, 160),
        "snippet": proposal.evidence.first().map(|evidence| compact_preview_text(&evidence.snippet, 180)),
    })
}

pub(crate) fn compact_preview_text(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut out = compact.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
}

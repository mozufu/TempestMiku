use serde_json::{Value, json};

use crate::SharedDriveStore;
use tm_host::HostError;
use tm_host::{GrantDoc, ToolDocs, ToolErrorDoc, ToolExample};

pub(crate) async fn content_to_bytes(
    store: &SharedDriveStore,
    content: &Value,
) -> tm_host::Result<Vec<u8>> {
    if let Some(text) = content.as_str() {
        if text.starts_with("blob:sha256:") {
            return store
                .read_blob(text)
                .await
                .map_err(|err| HostError::NotFound(err.to_string()));
        }
        return Ok(text.as_bytes().to_vec());
    }
    if let Some(text) = content.get("text").and_then(Value::as_str) {
        return Ok(text.as_bytes().to_vec());
    }
    if let Some(uri) = content.get("uri").and_then(Value::as_str)
        && uri.starts_with("blob:sha256:")
    {
        return store
            .read_blob(uri)
            .await
            .map_err(|err| HostError::NotFound(err.to_string()));
    }
    if let Some(uri) = content.get("uri").and_then(Value::as_str) {
        return Err(HostError::InvalidArgs(format!(
            "drive.put content.uri only supports blob:sha256: refs in P5 v1, got {uri}"
        )));
    }
    serde_json::to_vec(content).map_err(|err| HostError::InvalidArgs(err.to_string()))
}

pub(crate) fn drive_docs(name: &str, summary: &str, approval: &str, sensitive: bool) -> ToolDocs {
    ToolDocs {
        name: name.to_string(),
        namespace: "drive".to_string(),
        summary: summary.to_string(),
        description: Some(format!(
            "{summary}. Drive is local-first and exposes user documents through drive:// resources."
        )),
        signature: drive_signature(name),
        args_schema: drive_args_schema(name),
        result_schema: Some(drive_result_schema(name)),
        examples: drive_examples(name),
        errors: vec![
            tool_error(
                "CapabilityDeniedError",
                "The session lacks the drive capability grant.",
                false,
            ),
            tool_error(
                "ApprovalDeniedError",
                "The user denies a required drive mutation.",
                false,
            ),
            tool_error(
                "ApprovalTimeoutError",
                "A required drive mutation times out and defaults to deny.",
                true,
            ),
            tool_error(
                "InvalidPathError",
                "A drive path contains traversal, a raw host path, or a resource URI from another scheme.",
                false,
            ),
            tool_error(
                "NotFoundError",
                "The requested drive document does not exist.",
                false,
            ),
            tool_error(
                "HostCallError",
                "The drive store or blob integrity check fails.",
                false,
            ),
        ],
        grants: vec![GrantDoc {
            kind: "capability".to_string(),
            description: format!("Requires the {name} capability grant."),
        }],
        sensitive,
        approval: approval.to_string(),
        since: "P5".to_string(),
        stability: "experimental".to_string(),
    }
}

pub(crate) fn research_drive_docs() -> ToolDocs {
    let mut docs = drive_docs(
        "research.drive",
        "Build a bounded, cited research digest from local drive documents",
        "none",
        false,
    );
    docs.namespace = "research".to_string();
    docs.signature = "@research.drive ResearchDriveOptions -> ResearchDriveResult".to_string();
    docs.args_schema = json!({
        "type": "object",
        "properties": {
            "query": { "type": "string" },
            "project": { "type": "string" },
            "docKind": { "type": "string" },
            "tags": { "type": "array", "items": { "type": "string" } },
            "selector": { "type": "string" },
            "maxDocs": { "type": "integer", "minimum": 1, "maximum": 10 },
            "maxSnippets": { "type": "integer", "minimum": 1, "maximum": 10 },
            "maxBytesPerDoc": { "type": "integer", "minimum": 1, "maximum": 8000 },
            "maxDigestBytes": { "type": "integer", "minimum": 32, "maximum": 2000 },
            "maxWorkers": { "type": "integer", "minimum": 0, "maximum": 10 },
            "workerTimeoutMs": { "type": "integer", "minimum": 100, "maximum": 120000 },
            "totalTimeoutMs": { "type": "integer", "minimum": 100, "maximum": 300000 }
        }
    });
    docs.result_schema = Some(json!({ "type": "object" }));
    docs.examples = vec![ToolExample {
        title: None,
        code: "let result = @research.drive {query: \"approval policy\", project: \"TempestMiku\", maxDocs: 3}".to_string(),
        notes: Some("Returns deterministic local digests and drive:// citations.".to_string()),
    }];
    docs
}

fn drive_signature(name: &str) -> String {
    match name {
        "drive.put" => "@drive.put {content, options?} -> DrivePutResult",
        "drive.get" => "@drive.get {path?|uri?, selector?} -> ResourceContent",
        "drive.ls" => "@drive.ls DriveListOptions -> List DriveEntry",
        "drive.move" => "@drive.move {from, to, collision?, overwrite?} -> DriveEntry",
        "drive.search" => "@drive.search DriveSearchOptions -> List DriveSearchResult",
        "drive.tag" => "@drive.tag {path, tags} -> DriveEntry",
        "project.link" => "@project.link {hostPath, mode?, project?} -> DriveLinkPlan",
        "project.unlink" => "@project.unlink {alias} -> DriveUnlinkResult",
        "drive.organize" => "@drive.organize DriveOrganizeOptions -> List OrganizerProposal",
        _ => "@drive.unknown args -> value",
    }
    .to_string()
}

fn drive_args_schema(name: &str) -> Value {
    match name {
        "drive.put" => json!({
            "type": "object",
            "required": ["content"],
            "properties": {
                "content": {},
                "options": { "type": "object" }
            }
        }),
        "drive.get" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "uri": { "type": "string" },
                "selector": { "type": "string" }
            }
        }),
        "drive.move" => json!({
            "type": "object",
            "required": ["from", "to"],
            "properties": {
                "from": { "type": "string" },
                "to": { "type": "string" },
                "collision": { "enum": ["keep-both", "reject", "overwrite"] },
                "overwrite": { "type": "boolean" }
            }
        }),
        "drive.tag" => json!({
            "type": "object",
            "required": ["path", "tags"],
            "properties": {
                "path": { "type": "string" },
                "tags": { "type": "array", "items": { "type": "string" } }
            }
        }),
        "project.link" => json!({
            "type": "object",
            "required": ["hostPath"],
            "properties": {
                "hostPath": { "type": "string" },
                "mode": { "enum": ["ro", "rw"], "default": "ro" },
                "project": { "type": "string" }
            }
        }),
        "project.unlink" => json!({
            "type": "object",
            "required": ["alias"],
            "properties": {
                "alias": { "type": "string", "description": "Linked folder alias or linked:// URI." }
            }
        }),
        "drive.organize" => json!({
            "type": "object",
            "properties": {
                "apply": { "type": "boolean" },
                "config": {
                    "type": "object",
                    "additionalProperties": false,
                    "description": "Host SDK calls generate conservative proposals only; auto-apply rules are trusted server policy.",
                    "properties": {
                        "tier": { "enum": ["conservative"], "default": "conservative" }
                    }
                }
            }
        }),
        _ => json!({ "type": "object" }),
    }
}

fn drive_result_schema(name: &str) -> Value {
    match name {
        "drive.get" => json!({ "type": "object", "description": "ResourceContent" }),
        "drive.search" | "drive.organize" | "drive.ls" => json!({ "type": "array" }),
        _ => json!({ "type": "object" }),
    }
}

fn drive_examples(name: &str) -> Vec<ToolExample> {
    let code = match name {
        "drive.put" => {
            "let filed = @drive.put {content: \"# Note\\nhello\", options: {auto: true, project: \"TempestMiku\"}}"
        }
        "drive.get" => {
            "let doc = @drive.get {path: \"projects/tempestmiku/docs/note.md\", selector: \"1-20\"}"
        }
        "drive.ls" => "let invoices = @drive.ls {path: \"/by-type/invoice\"}",
        "drive.move" => {
            "@drive.move {from: \"inbox/today/note.md\", to: \"projects/tempestmiku/notes/note.md\"}"
        }
        "drive.search" => {
            "let hits = @drive.search {query: \"approval policy\", project: \"TempestMiku\", returnSnippets: true}"
        }
        "drive.tag" => {
            "@drive.tag {path: \"projects/tempestmiku/notes/note.md\", tags: [\"planning\"]}"
        }
        "project.link" => {
            "let plan = @project.link {hostPath: \"/path/to/project\", mode: \"ro\", project: \"TempestMiku\"}"
        }
        "project.unlink" => "let revoked = @project.unlink {alias: \"tempestmiku\"}",
        "drive.organize" => "let proposals = @drive.organize {}",
        _ => "@tools.call {name: \"drive.unknown\", args: {}}",
    };
    vec![ToolExample {
        title: None,
        code: code.to_string(),
        notes: None,
    }]
}

fn tool_error(name: &str, when: &str, retryable: bool) -> ToolErrorDoc {
    ToolErrorDoc {
        name: name.to_string(),
        when: when.to_string(),
        retryable,
    }
}

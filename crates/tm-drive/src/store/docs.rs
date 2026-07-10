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

fn drive_signature(name: &str) -> String {
    match name {
        "drive.put" => "drive.put(content: DriveContent, opts?: DrivePutOptions): Promise<DrivePutResult>",
        "drive.get" => "drive.get(pathOrUri: string, opts?: { selector?: ResourceSelector }): Promise<ResourceContent>",
        "drive.ls" => "drive.ls(pathOrQuery?: string, opts?: DriveListOptions): Promise<DriveEntry[]>",
        "drive.move" => "drive.move(from: string, to: string, opts?: DriveMoveOptions): Promise<DriveEntry>",
        "drive.search" => "drive.search(query?: string, opts?: DriveSearchOptions): Promise<DriveSearchResult[]>",
        "drive.tag" => "drive.tag(path: string, tags: string[]): Promise<DriveEntry>",
        "drive.link" => "drive.link(hostPath: string, mode?: 'ro' | 'rw', opts?: { project?: string }): Promise<DriveLinkPlan>",
        "drive.unlink" => "drive.unlink(aliasOrUri: string): Promise<DriveUnlinkResult>",
        "drive.organize" => "drive.organize(opts?: DriveOrganizeOptions): Promise<OrganizerProposal[]>",
        _ => "drive.unknown(args): Promise<unknown>",
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
        "drive.link" => json!({
            "type": "object",
            "required": ["hostPath"],
            "properties": {
                "hostPath": { "type": "string" },
                "mode": { "enum": ["ro", "rw"], "default": "ro" },
                "project": { "type": "string" }
            }
        }),
        "drive.unlink" => json!({
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
            "const filed = await drive.put('# Note\\nhello', { auto: true, project: 'TempestMiku' });"
        }
        "drive.get" => {
            "const doc = await drive.get('projects/tempestmiku/docs/note.md', { selector: '1-20' });"
        }
        "drive.ls" => "const invoices = await drive.ls('/by-type/invoice');",
        "drive.move" => {
            "await drive.move('inbox/today/note.md', 'projects/tempestmiku/notes/note.md');"
        }
        "drive.search" => {
            "const hits = await drive.search('approval policy', { project: 'TempestMiku', returnSnippets: true });"
        }
        "drive.tag" => "await drive.tag('projects/tempestmiku/notes/note.md', ['planning']);",
        "drive.link" => {
            "const plan = await drive.link('/path/to/project', 'ro', { project: 'TempestMiku' });"
        }
        "drive.unlink" => "const revoked = await drive.unlink('tempestmiku');",
        "drive.organize" => "const proposals = await drive.organize();",
        _ => "await tools.call('drive.unknown', {});",
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

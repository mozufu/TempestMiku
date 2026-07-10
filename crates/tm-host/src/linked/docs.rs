use serde_json::{Value, json};

use crate::{GrantDoc, ToolDocs, ToolErrorDoc, ToolExample};

pub(super) fn docs(name: &str, namespace: &str, summary: &str, sensitive: bool) -> ToolDocs {
    let base = ToolDocs {
        name: name.to_string(),
        namespace: namespace.to_string(),
        summary: summary.to_string(),
        description: None,
        signature: format!("{name}(args)"),
        args_schema: json!({ "type": "object" }),
        result_schema: None,
        examples: Vec::new(),
        errors: Vec::new(),
        grants: vec![GrantDoc {
            kind: "workspace".to_string(),
            description: format!("Allows the {name} capability."),
        }],
        sensitive,
        approval: if sensitive { "policy" } else { "none" }.to_string(),
        since: "P0".to_string(),
        stability: "experimental".to_string(),
    };

    match name {
        "fs.read" => ToolDocs {
            description: Some(
                "Read a UTF-8 text file from a granted linked folder and return a ResourceContent envelope."
                    .to_string(),
            ),
            signature: "fs.read(path: SdkPath, opts?: FsReadOptions): Promise<ResourceContent>"
                .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["path"],
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string", "description": "Linked path such as tempestmiku:src/lib.rs or linked://tempestmiku/src/lib.rs." },
                    "selector": { "type": "string", "description": "Optional 1-based inclusive line range, for example 10-40." },
                    "raw": { "type": "boolean", "description": "Reserved in P0; reads still return ResourceContent." }
                }
            }),
            result_schema: Some(resource_content_schema()),
            examples: vec![ToolExample {
                title: Some("Read a Rust file".to_string()),
                code: "const file = await fs.read('tempestmiku:src/lib.rs');\ndisplay(file.content, { kind: 'text' });"
                    .to_string(),
                notes: Some("The result is an envelope with uri, kind, mime, content, preview, sizeBytes, and hasMore.".to_string()),
            }],
            errors: vec![
                tool_error("CapabilityDeniedError", "The session lacks the fs.read grant.", false),
                tool_error("InvalidPathError", "The linked alias is unknown, the path escapes the root, or the file does not exist.", false),
                tool_error("InvalidArgsError", "The selector is malformed or the file is not UTF-8 text.", false),
                tool_error("HostCallError", "The host filesystem read fails after policy checks.", false),
            ],
            grants: vec![linked_grant("Read access to the target linked folder.")],
            approval: "none".to_string(),
            ..base
        },
        "fs.write" => ToolDocs {
            description: Some(
                "Write UTF-8 text below a writable linked-folder grant. Overwrites require approval. Binary writes are deferred."
                    .to_string(),
            ),
            signature:
                "fs.write(path: SdkPath, data: string, opts?: FsWriteOptions): Promise<FsWriteResult>"
                    .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["path", "data"],
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string" },
                    "data": { "type": "string" },
                    "createParents": { "type": "boolean", "default": false },
                    "overwrite": { "type": "boolean", "default": false },
                    "mime": { "type": "string", "description": "Reserved metadata hint in P0." }
                }
            }),
            result_schema: Some(write_result_schema()),
            examples: vec![ToolExample {
                title: Some("Create a file".to_string()),
                code: "await fs.write('tempestmiku:notes/todo.txt', 'ship P1\\n', { createParents: true });"
                    .to_string(),
                notes: None,
            }],
            errors: vec![
                tool_error("CapabilityDeniedError", "The linked folder is read-only or fs.write is not granted.", false),
                tool_error("ApprovalDeniedError", "The user denies an overwrite.", false),
                tool_error("ApprovalTimeoutError", "The overwrite approval request times out and defaults to deny.", true),
                tool_error("InvalidPathError", "The path is outside the linked root or the parent is unavailable.", false),
                tool_error("InvalidArgsError", "The target exists without overwrite=true, or args do not match the schema.", false),
                tool_error("HostCallError", "The host write fails after policy checks.", false),
            ],
            grants: vec![linked_grant("Write access to the target linked folder.")],
            approval: "on-write".to_string(),
            ..base
        },
        "fs.ls" => ToolDocs {
            description: Some("List files and directories under a linked folder.".to_string()),
            signature: "fs.ls(path?: SdkPath, opts?: FsListOptions): Promise<FsEntry[]>"
                .to_string(),
            args_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string", "description": "Defaults to the first linked folder alias." },
                    "recursive": { "type": "boolean", "default": false },
                    "limit": { "type": "integer", "minimum": 1, "default": 1000 },
                    "includeHidden": { "type": "boolean", "default": false }
                }
            }),
            result_schema: Some(json!({ "type": "array", "items": fs_entry_schema() })),
            examples: vec![ToolExample {
                title: Some("List source files".to_string()),
                code: "const entries = await fs.ls('tempestmiku:src');\ndisplay(entries, { kind: 'json' });"
                    .to_string(),
                notes: None,
            }],
            errors: read_only_errors("fs.ls"),
            grants: vec![linked_grant("List access to the target linked folder.")],
            approval: "none".to_string(),
            ..base
        },
        "fs.find" => ToolDocs {
            description: Some(
                "Find linked-folder entries by glob, honoring gitignore by default.".to_string(),
            ),
            signature:
                "fs.find(patterns: string | string[], opts?: FsFindOptions): Promise<FsEntry[]>"
                    .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["patterns"],
                "additionalProperties": false,
                "properties": {
                    "patterns": {
                        "oneOf": [
                            { "type": "string" },
                            { "type": "array", "items": { "type": "string" } }
                        ]
                    },
                    "cwd": { "type": "string", "description": "Linked path to search from; defaults to the first linked folder." },
                    "limit": { "type": "integer", "minimum": 1, "default": 1000 },
                    "includeHidden": { "type": "boolean", "default": false },
                    "respectGitignore": { "type": "boolean", "default": true }
                }
            }),
            result_schema: Some(json!({ "type": "array", "items": fs_entry_schema() })),
            examples: vec![ToolExample {
                title: Some("Find Rust files".to_string()),
                code: "const files = await fs.find('**/*.rs', { cwd: 'tempestmiku:' });"
                    .to_string(),
                notes: Some("Patterns are matched against both the cwd-relative and linked-root-relative path.".to_string()),
            }],
            errors: read_only_errors("fs.find"),
            grants: vec![linked_grant("Search access to the target linked folder.")],
            approval: "none".to_string(),
            ..base
        },
        "code.search" => ToolDocs {
            description: Some(
                "Search UTF-8 files with regex or literal text and return optimistic-concurrency tags."
                    .to_string(),
            ),
            signature: "code.search(query: CodeSearchQuery): Promise<CodeSearchResult[]>"
                .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["pattern", "paths"],
                "additionalProperties": false,
                "properties": {
                    "pattern": { "type": "string" },
                    "paths": { "type": "array", "minItems": 1, "items": { "type": "string" } },
                    "caseSensitive": { "type": "boolean", "default": true },
                    "regex": { "type": "boolean", "default": true },
                    "contextLines": { "type": "integer", "minimum": 0, "default": 0 },
                    "limit": { "type": "integer", "minimum": 1, "default": 1000 }
                }
            }),
            result_schema: Some(json!({ "type": "array", "items": code_search_result_schema() })),
            examples: vec![ToolExample {
                title: Some("Search literal text".to_string()),
                code: "const hits = await code.search({ pattern: 'TODO', paths: ['tempestmiku:'], regex: false, contextLines: 1 });"
                    .to_string(),
                notes: Some("Use a hit's tag with code.edit when editing an existing file.".to_string()),
            }],
            errors: read_only_errors("code.search"),
            grants: vec![linked_grant("Read/search access to the target linked folder.")],
            approval: "none".to_string(),
            ..base
        },
        "code.edit" => ToolDocs {
            description: Some(
                "Apply narrow JSON line hunks with optimistic concurrency tags; raw patch strings are not accepted."
                    .to_string(),
            ),
            signature:
                "code.edit(patch: PatchEdit, opts?: CodeEditOptions): Promise<CodeEditResult>"
                    .to_string(),
            args_schema: code_edit_args_schema(),
            result_schema: Some(code_edit_result_schema()),
            examples: vec![ToolExample {
                title: Some("Replace one line".to_string()),
                code: "const hits = await code.search({ pattern: 'old', paths: ['tempestmiku:src/lib.rs'], regex: false });\nconst hit = hits[0];\nawait code.edit({ path: hit.path, tag: hit.tag, hunks: [{ op: 'replace', startLine: hit.line, endLine: hit.line, lines: ['new'] }] });"
                    .to_string(),
                notes: Some("Existing files require a fresh tag from code.search or another read/search path.".to_string()),
            }],
            errors: vec![
                tool_error("CapabilityDeniedError", "code.edit is not granted or the linked folder is read-only.", false),
                tool_error("ApprovalDeniedError", "The user denies a destructive remove or overwrite move.", false),
                tool_error("ApprovalTimeoutError", "The approval request times out and defaults to deny.", true),
                tool_error("InvalidArgsError", "The tag is stale, hunks are malformed, or remove is mixed with other hunks.", false),
                tool_error("InvalidPathError", "The path escapes the linked root or move crosses aliases.", false),
                tool_error("HostCallError", "The host edit fails after policy checks.", false),
            ],
            grants: vec![linked_grant("Write access to the target linked folder.")],
            approval: "policy".to_string(),
            ..base
        },
        "proc.run" => ToolDocs {
            description: Some(
                "Run an allowlisted command with argv-vector arguments inside a linked-folder cwd; shell strings, pipes, redirects, env overrides, and stdin are rejected."
                    .to_string(),
            ),
            signature: "proc.run(cmd: string, args?: string[], opts?: ProcRunOptions): Promise<ProcOutput>"
                .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["cmd"],
                "additionalProperties": false,
                "properties": {
                    "cmd": { "type": "string", "description": "Executable name only, for example cargo. Shell strings like cargo test are invalid." },
                    "args": { "type": "array", "items": { "type": "string" }, "default": [] },
                    "cwd": { "type": "string", "description": "Linked cwd; defaults to the first linked folder." },
                    "timeoutMs": { "type": "integer", "minimum": 1, "maximum": 180000, "default": 180000 },
                    "outputBytes": { "type": "integer", "minimum": 1, "default": 50000 },
                    "env": { "type": "object", "description": "Reserved in P0; non-empty env overrides are rejected." },
                    "stdin": { "description": "Reserved in P0; non-empty stdin is rejected." }
                }
            }),
            result_schema: Some(proc_output_schema()),
            examples: vec![ToolExample {
                title: Some("Run tests".to_string()),
                code: "const run = await proc.run('cargo', ['test'], { cwd: 'tempestmiku:' });\ndisplay(run, { kind: 'json' });"
                    .to_string(),
                notes: Some("Use cmd plus args. Do not pass proc.run('cargo test').".to_string()),
            }],
            errors: vec![
                tool_error("CapabilityDeniedError", "proc.run is not granted or the command is not allowlisted for the linked cwd.", false),
                tool_error("ApprovalDeniedError", "The user denies a command outside safe argv prefixes.", false),
                tool_error("ApprovalTimeoutError", "The approval request times out and defaults to deny.", true),
                tool_error("InvalidArgsError", "cmd is not a bare executable, or stdin/env/shell-style args are requested.", false),
                tool_error("InvalidPathError", "cwd is outside the linked root or missing.", false),
                tool_error("HostCallError", "The host process cannot be spawned or collected.", false),
            ],
            grants: vec![GrantDoc {
                kind: "process".to_string(),
                description: "Allowlisted process execution for the linked folder cwd.".to_string(),
            }],
            approval: "policy".to_string(),
            ..base
        },
        _ => base,
    }
}

fn linked_grant(description: &str) -> GrantDoc {
    GrantDoc {
        kind: "linked-folder".to_string(),
        description: description.to_string(),
    }
}

fn tool_error(name: &str, when: &str, retryable: bool) -> ToolErrorDoc {
    ToolErrorDoc {
        name: name.to_string(),
        when: when.to_string(),
        retryable,
    }
}

fn read_only_errors(capability: &str) -> Vec<ToolErrorDoc> {
    vec![
        tool_error(
            "CapabilityDeniedError",
            &format!("The session lacks the {capability} grant."),
            false,
        ),
        tool_error(
            "InvalidPathError",
            "The linked alias is unknown or the path escapes the root.",
            false,
        ),
        tool_error(
            "InvalidArgsError",
            "Arguments do not match the schema, a glob/regex is invalid, or a file is not UTF-8 text.",
            false,
        ),
        tool_error(
            "HostCallError",
            "The host filesystem operation fails after policy checks.",
            false,
        ),
    ]
}

fn resource_content_schema() -> Value {
    json!({
        "type": "object",
        "required": ["uri", "kind", "mime", "sizeBytes", "hasMore", "content", "preview"],
        "properties": {
            "uri": { "type": "string" },
            "kind": { "type": "string" },
            "mime": { "type": "string" },
            "title": { "type": ["string", "null"] },
            "sizeBytes": { "type": "integer" },
            "selector": { "type": ["string", "null"] },
            "hasMore": { "type": "boolean" },
            "content": { "type": "string" },
            "preview": { "type": "string" }
        }
    })
}

fn write_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "uri", "bytesWritten", "created", "overwritten"],
        "properties": {
            "path": { "type": "string" },
            "uri": { "type": "string" },
            "bytesWritten": { "type": "integer" },
            "created": { "type": "boolean" },
            "overwritten": { "type": "boolean" }
        }
    })
}

fn fs_entry_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "uri", "name", "kind"],
        "properties": {
            "path": { "type": "string" },
            "uri": { "type": "string" },
            "name": { "type": "string" },
            "kind": { "enum": ["file", "directory", "symlink", "other"] },
            "sizeBytes": { "type": ["integer", "null"] },
            "modifiedAt": { "type": ["string", "null"] }
        }
    })
}

fn code_search_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "uri", "line", "column", "text", "before", "after", "tag"],
        "properties": {
            "path": { "type": "string" },
            "uri": { "type": "string" },
            "line": { "type": "integer", "minimum": 1 },
            "column": { "type": "integer", "minimum": 1 },
            "text": { "type": "string" },
            "before": { "type": "array", "items": { "type": "string" } },
            "after": { "type": "array", "items": { "type": "string" } },
            "tag": { "type": "string", "description": "Optimistic concurrency tag for code.edit." }
        }
    })
}

fn code_edit_args_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "hunks"],
        "additionalProperties": false,
        "properties": {
            "path": { "type": "string" },
            "tag": { "type": "string", "description": "Required for existing files." },
            "format": { "type": "boolean", "description": "Reserved in P0." },
            "hunks": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "required": ["op", "startLine", "endLine", "lines"],
                            "properties": {
                                "op": { "const": "replace" },
                                "startLine": { "type": "integer", "minimum": 1 },
                                "endLine": { "type": "integer", "minimum": 1 },
                                "lines": { "type": "array", "items": { "type": "string" } }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["op", "startLine", "endLine"],
                            "properties": {
                                "op": { "const": "delete" },
                                "startLine": { "type": "integer", "minimum": 1 },
                                "endLine": { "type": "integer", "minimum": 1 }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["op", "at", "lines"],
                            "properties": {
                                "op": { "const": "insert" },
                                "at": { "enum": ["head", "tail", "before", "after"] },
                                "line": { "type": "integer", "minimum": 1 },
                                "lines": { "type": "array", "items": { "type": "string" } }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["op", "dest"],
                            "properties": {
                                "op": { "const": "move" },
                                "dest": { "type": "string" }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["op"],
                            "properties": {
                                "op": { "const": "remove" }
                            }
                        }
                    ]
                }
            }
        }
    })
}

fn code_edit_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "changed", "diff", "diagnostics"],
        "properties": {
            "path": { "type": "string" },
            "changed": { "type": "boolean" },
            "diff": { "type": "string" },
            "newTag": { "type": ["string", "null"] },
            "diagnostics": { "type": "array" }
        }
    })
}

fn artifact_ref_schema() -> Value {
    json!({
        "type": "object",
        "required": ["uri", "id", "kind", "mime", "sizeBytes", "preview"],
        "properties": {
            "uri": { "type": "string" },
            "id": { "type": "string" },
            "kind": { "type": "string" },
            "mime": { "type": "string" },
            "title": { "type": ["string", "null"] },
            "sizeBytes": { "type": "integer" },
            "preview": { "type": "string" }
        }
    })
}

fn proc_output_schema() -> Value {
    json!({
        "type": "object",
        "required": ["cmd", "args", "cwd", "exitCode", "stdout", "stderr", "timedOut", "durationMs", "truncated"],
        "properties": {
            "cmd": { "type": "string" },
            "args": { "type": "array", "items": { "type": "string" } },
            "cwd": { "type": "string" },
            "exitCode": { "type": "integer" },
            "stdout": { "type": "string" },
            "stderr": { "type": "string" },
            "timedOut": { "type": "boolean" },
            "durationMs": { "type": "integer" },
            "truncated": { "type": "boolean" },
            "artifact": {
                "oneOf": [
                    artifact_ref_schema(),
                    { "type": "null" }
                ]
            }
        }
    })
}

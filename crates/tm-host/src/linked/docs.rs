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
        // Trace sensitivity and user approval are independent contracts. Individual capability
        // docs below describe approval; `sensitive` only controls persistence/telemetry privacy.
        approval: "none".to_string(),
        since: "P0".to_string(),
        stability: "experimental".to_string(),
    };

    match name {
        "fs.read" => ToolDocs {
            description: Some(
                "Read a UTF-8 text file from a granted linked folder and return a ResourceContent envelope."
                    .to_string(),
            ),
            signature: "@fs.read FsReadArgs -> ResourceContent"
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
                code: "let file = @fs.read {path: \"tempestmiku:src/lib.rs\"};\nfile.content |> display {kind: \"text\"}"
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
                "@fs.write FsWriteArgs -> FsWriteResult"
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
                code: "@fs.write {path: \"tempestmiku:notes/todo.txt\", data: \"ship P1\\n\", createParents: true}"
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
        "fs.patch" => ToolDocs {
            description: Some(
                "Atomically apply expected-context line hunks to an existing UTF-8 file using an optimistic-concurrency tag. Large diffs spill to an artifact."
                    .to_string(),
            ),
            signature: "@fs.patch FsPatchArgs -> FsPatchResult".to_string(),
            args_schema: fs_patch_args_schema(),
            result_schema: Some(fs_patch_result_schema()),
            examples: vec![ToolExample {
                title: Some("Replace one line".to_string()),
                code: "let hits = @fs.grep {pattern: \"old\", paths: [\"tempestmiku:src/lib.rs\"], regex: false};\nlet hit = match hits { | first :: _ -> first | [] -> null };\n@fs.patch {path: hit.path, tag: hit.tag, hunks: [{op: \"replace\", startLine: hit.line, endLine: hit.line, expectedLines: [hit.text], lines: [\"new\"]}]}"
                    .to_string(),
                notes: Some("Use fs.write for new files. Existing files require a fresh tag from fs.grep or another tagged read path.".to_string()),
            }],
            errors: vec![
                tool_error("CapabilityDeniedError", "fs.patch is not granted or the linked folder is read-only.", false),
                tool_error("InvalidArgsError", "The tag is stale, expected context does not match, no hunks are supplied, or a line range is invalid.", false),
                tool_error("InvalidPathError", "The path escapes the linked root, does not exist, or is not a file.", false),
                tool_error("HostCallError", "The atomic write or diff artifact persistence fails.", false),
            ],
            grants: vec![linked_grant("Patch access to the target linked folder.")],
            approval: "none".to_string(),
            ..base
        },
        "fs.move" => ToolDocs {
            description: Some(
                "Move an existing tagged file within one linked-folder alias. Overwriting a destination requires approval."
                    .to_string(),
            ),
            signature: "@fs.move FsMoveArgs -> FsMoveResult".to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["path", "dest", "tag"],
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string" },
                    "dest": { "type": "string" },
                    "tag": { "type": "string" },
                    "createParents": { "type": "boolean", "default": false },
                    "overwrite": { "type": "boolean", "default": false }
                }
            }),
            result_schema: Some(json!({
                "type": "object",
                "required": ["path", "dest", "overwritten", "newTag"],
                "properties": {
                    "path": { "type": "string" },
                    "dest": { "type": "string" },
                    "overwritten": { "type": "boolean" },
                    "newTag": { "type": "string" }
                }
            })),
            examples: vec![ToolExample {
                title: Some("Rename a file".to_string()),
                code: "@fs.move {path: hit.path, dest: \"tempestmiku:src/new.rs\", tag: hit.tag}".to_string(),
                notes: None,
            }],
            errors: write_errors("fs.move", "an overwrite"),
            grants: vec![linked_grant("Move access to the target linked folder.")],
            approval: "on-overwrite".to_string(),
            ..base
        },
        "fs.remove" => ToolDocs {
            description: Some(
                "Remove an existing tagged file after explicit approval.".to_string(),
            ),
            signature: "@fs.remove FsRemoveArgs -> FsRemoveResult".to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["path", "tag"],
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string" },
                    "tag": { "type": "string" }
                }
            }),
            result_schema: Some(json!({
                "type": "object",
                "required": ["path", "removed"],
                "properties": {
                    "path": { "type": "string" },
                    "removed": { "type": "boolean" }
                }
            })),
            examples: vec![ToolExample {
                title: Some("Remove a file".to_string()),
                code: "@fs.remove {path: hit.path, tag: hit.tag}".to_string(),
                notes: Some("Removal always requires approval and defaults to deny on timeout.".to_string()),
            }],
            errors: write_errors("fs.remove", "file removal"),
            grants: vec![linked_grant("Remove access to the target linked folder.")],
            approval: "always".to_string(),
            ..base
        },
        "fs.ls" => ToolDocs {
            description: Some("List files and directories under a linked folder.".to_string()),
            signature: "@fs.ls FsListOptions -> List FsEntry"
                .to_string(),
            args_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string", "description": "Defaults to the first linked folder alias." },
                    "recursive": { "type": "boolean", "default": false },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 10000, "default": 1000 },
                    "includeHidden": { "type": "boolean", "default": false }
                }
            }),
            result_schema: Some(json!({ "type": "array", "items": fs_entry_schema() })),
            examples: vec![ToolExample {
                title: Some("List source files".to_string()),
                code: "let entries = @fs.ls {path: \"tempestmiku:src\"};\nentries |> display {kind: \"json\"}"
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
                "@fs.find FsFindOptions -> List FsEntry"
                    .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["patterns"],
                "additionalProperties": false,
                "properties": {
                    "patterns": {
                        "oneOf": [
                            { "type": "string", "maxLength": 4096 },
                            { "type": "array", "minItems": 1, "maxItems": 64, "items": { "type": "string", "maxLength": 4096 } }
                        ]
                    },
                    "cwd": { "type": "string", "description": "Linked path to search from; defaults to the first linked folder." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 10000, "default": 1000 },
                    "includeHidden": { "type": "boolean", "default": false },
                    "respectGitignore": { "type": "boolean", "default": true }
                }
            }),
            result_schema: Some(json!({ "type": "array", "items": fs_entry_schema() })),
            examples: vec![ToolExample {
                title: Some("Find Rust files".to_string()),
                code: "let files = @fs.find {pattern: \"**/*.rs\", cwd: \"tempestmiku:\"}"
                    .to_string(),
                notes: Some("Patterns are matched against both the cwd-relative and linked-root-relative path.".to_string()),
            }],
            errors: read_only_errors("fs.find"),
            grants: vec![linked_grant("Search access to the target linked folder.")],
            approval: "none".to_string(),
            ..base
        },
        "fs.grep" => ToolDocs {
            description: Some(
                "Search UTF-8 files with regex or literal text and return optimistic-concurrency tags."
                    .to_string(),
            ),
            signature: "@fs.grep {pattern, paths, caseSensitive?, regex?, contextLines?, limit?} -> List SearchMatch"
                .to_string(),
            args_schema: json!({
                "type": "object",
                "required": ["pattern", "paths"],
                "additionalProperties": false,
                "properties": {
                    "pattern": { "type": "string", "maxLength": 16384 },
                    "paths": { "type": "array", "minItems": 1, "maxItems": 64, "items": { "type": "string" } },
                    "caseSensitive": { "type": "boolean", "default": true },
                    "regex": { "type": "boolean", "default": true },
                    "contextLines": { "type": "integer", "minimum": 0, "maximum": 20, "default": 0 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 10000, "default": 1000 }
                }
            }),
            result_schema: Some(json!({ "type": "array", "items": search_match_schema() })),
            examples: vec![ToolExample {
                title: Some("Search literal text".to_string()),
                code: "let hits = @fs.grep {pattern: \"TODO\", paths: [\"tempestmiku:\"], regex: false, contextLines: 1}"
                    .to_string(),
                notes: Some("Use a hit's tag with fs.patch, fs.move, or fs.remove.".to_string()),
            }],
            errors: read_only_errors("fs.grep"),
            grants: vec![linked_grant("Read/search access to the target linked folder.")],
            approval: "none".to_string(),
            ..base
        },
        "git.status" | "git.diff" | "git.grep" | "git.log" | "git.show" | "git.clone"
        | "git.init" | "git.add" | "git.mv" | "git.restore" | "git.rm" | "git.bisect"
        | "git.commit" | "git.push" | "git.pull" => {
            let cwd = json!({
                "type": "string",
                "description": "Linked repository directory; defaults to the first linked folder."
            });
            let paths = json!({
                "type": "array",
                "minItems": 1,
                "maxItems": 64,
                "items": { "type": "string", "minLength": 1, "maxLength": 4096 },
                "description": "Literal normalized repository-relative paths; flags, pathspec magic, absolute paths, and traversal are rejected."
            });
            let full_oid = json!({
                "type": "string",
                "pattern": "^(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})$",
                "description": "Full hexadecimal Git object ID."
            });
            let (description, signature, args_schema, example, notes, approval, grant) = match name {
                "git.status" => (
                    "Return repository status using a fixed, read-only Git invocation. Caller-supplied refs, pathspecs, flags, config, and environment are rejected.",
                    "@git.status GitCwdArgs -> GitResult",
                    json!({ "type": "object", "additionalProperties": false, "properties": { "cwd": cwd } }),
                    "@git.status {cwd: \"tempestmiku:\"}",
                    "Status is trace-sensitive but approval-free.",
                    "none",
                    "Read-only Git status inspection of the linked repository.",
                ),
                "git.diff" => (
                    "Return the unstaged repository diff using a fixed, read-only Git invocation with external diff and text conversion disabled.",
                    "@git.diff GitCwdArgs -> GitResult",
                    json!({ "type": "object", "additionalProperties": false, "properties": { "cwd": cwd } }),
                    "@git.diff {cwd: \"tempestmiku:\"}",
                    "Diff is trace-sensitive but approval-free.",
                    "none",
                    "Read-only Git diff inspection of the linked repository.",
                ),
                "git.grep" => (
                    "Search tracked repository content using a fixed Git grep invocation. The bounded pattern is passed literally; caller flags, revisions, and pathspecs are rejected.",
                    "@git.grep GitGrepArgs -> GitResult",
                    json!({
                        "type": "object", "required": ["pattern"], "additionalProperties": false,
                        "properties": {
                            "cwd": cwd,
                            "pattern": { "type": "string", "minLength": 1, "maxLength": 4096 },
                            "caseSensitive": { "type": "boolean", "default": true }
                        }
                    }),
                    "@git.grep {cwd: \"tempestmiku:\", pattern: \"CapabilityDenied\", caseSensitive: true}",
                    "Grep is the only content-searching Git operation that is approval-free.",
                    "none",
                    "Approval-free read-only search of tracked repository content.",
                ),
                "git.log" => (
                    "After approval, return the latest 20 commits using a fixed machine-readable format. Refs, ranges, pathspecs, formats, and caller flags are rejected.",
                    "@git.log GitCwdArgs -> GitResult",
                    json!({ "type": "object", "additionalProperties": false, "properties": { "cwd": cwd } }),
                    "@git.log {cwd: \"tempestmiku:\"}",
                    "Repository history always requires owner approval.",
                    "always",
                    "Approval-backed recent Git history inspection.",
                ),
                "git.show" => (
                    "After approval, show one snapshotted Git object using a fixed invocation. An omitted revision resolves to the snapshotted HEAD; a supplied revision must be a full object ID.",
                    "@git.show GitShowArgs -> GitResult",
                    json!({
                        "type": "object", "additionalProperties": false,
                        "properties": { "cwd": cwd, "revision": full_oid }
                    }),
                    "@git.show {cwd: \"tempestmiku:\"}",
                    "Object and history inspection always requires owner approval.",
                    "always",
                    "Approval-backed inspection of a pinned Git object.",
                ),
                "git.clone" => (
                    "After approval, clone a credential-free HTTPS repository into the pinned empty linked cwd using the fixed destination '.'. Branch selection, local/file transports, helpers, prompts, hooks, and caller flags are rejected.",
                    "@git.clone GitCloneArgs -> GitResult",
                    json!({
                        "type": "object", "required": ["url"], "additionalProperties": false,
                        "properties": {
                            "cwd": cwd,
                            "url": { "type": "string", "minLength": 1, "maxLength": 4096, "format": "uri", "pattern": "^https://" }
                        }
                    }),
                    "@git.clone {cwd: \"tempestmiku:\", url: \"https://github.com/example/project.git\"}",
                    "The cwd must be writable and empty; the host fixes the clone destination to '.'.",
                    "always",
                    "Approval-backed HTTPS clone into an empty linked folder.",
                ),
                "git.init" => (
                    "After approval, initialize exactly the writable linked cwd using a fixed Git invocation. Templates, object formats, initial branches, shared modes, and caller flags are rejected.",
                    "@git.init GitCwdArgs -> GitResult",
                    json!({ "type": "object", "additionalProperties": false, "properties": { "cwd": cwd } }),
                    "@git.init {cwd: \"tempestmiku:\"}",
                    "Initialization mutates the linked folder and always requires approval.",
                    "always",
                    "Approval-backed initialization of a linked repository.",
                ),
                "git.add" => (
                    "After approval, stage only the supplied literal normalized repository-relative paths. Flags, pathspec magic, traversal, and implicit all-repository staging are rejected.",
                    "@git.add GitPathsArgs -> GitResult",
                    json!({ "type": "object", "required": ["paths"], "additionalProperties": false, "properties": { "cwd": cwd, "paths": paths } }),
                    "@git.add {cwd: \"tempestmiku:\", paths: [\"src/lib.rs\", \"Cargo.toml\"]}",
                    "Only explicitly listed paths are staged.",
                    "always",
                    "Approval-backed staging of exact linked-repository paths.",
                ),
                "git.mv" => (
                    "After approval, move one literal top-level repository entry to another literal top-level name. Nested paths, traversal, flags, pathspec magic, and overwrites are rejected.",
                    "@git.mv GitMvArgs -> GitResult",
                    json!({
                        "type": "object", "required": ["path", "dest"], "additionalProperties": false,
                        "properties": {
                            "cwd": cwd,
                            "path": { "type": "string", "minLength": 1, "maxLength": 4096, "pattern": "^[^/\\\\]+$" },
                            "dest": { "type": "string", "minLength": 1, "maxLength": 4096, "pattern": "^[^/\\\\]+$" }
                        }
                    }),
                    "@git.mv {cwd: \"tempestmiku:\", path: \"old-name\", dest: \"new-name\"}",
                    "Both names are pinned top-level literal entries and must differ.",
                    "always",
                    "Approval-backed move of an exact top-level repository entry.",
                ),
                "git.restore" => (
                    "After approval, restore only the supplied literal normalized repository-relative paths from the index into the worktree. Sources, revisions, staged mode, flags, and pathspec magic are rejected.",
                    "@git.restore GitPathsArgs -> GitResult",
                    json!({ "type": "object", "required": ["paths"], "additionalProperties": false, "properties": { "cwd": cwd, "paths": paths } }),
                    "@git.restore {cwd: \"tempestmiku:\", paths: [\"src/lib.rs\"]}",
                    "Restore is fixed to index-to-worktree for explicitly listed paths.",
                    "always",
                    "Approval-backed restoration of exact linked-repository paths.",
                ),
                "git.rm" => (
                    "After approval, remove only the supplied literal normalized repository-relative paths from the worktree and index. Force, cached mode, recursion flags, and pathspec magic are rejected.",
                    "@git.rm GitPathsArgs -> GitResult",
                    json!({ "type": "object", "required": ["paths"], "additionalProperties": false, "properties": { "cwd": cwd, "paths": paths } }),
                    "@git.rm {cwd: \"tempestmiku:\", paths: [\"obsolete.rs\"]}",
                    "Removal is limited to explicitly listed paths and always requires approval.",
                    "always",
                    "Approval-backed removal of exact linked-repository paths.",
                ),
                "git.bisect" => (
                    "After approval, perform one closed no-checkout bisect transition: start from full object IDs, mark good/bad/skip, or reset. Run, replay, terms, arbitrary revisions, and caller flags are never exposed.",
                    "@git.bisect GitBisectArgs -> GitResult",
                    json!({
                        "oneOf": [
                            {
                                "type": "object", "required": ["action", "bad", "good"], "additionalProperties": false,
                                "properties": {
                                    "cwd": cwd, "action": { "const": "start" }, "bad": full_oid,
                                    "good": { "type": "array", "minItems": 1, "maxItems": 32, "uniqueItems": true, "items": full_oid }
                                }
                            },
                            {
                                "type": "object", "required": ["action"], "additionalProperties": false,
                                "properties": { "cwd": cwd, "action": { "enum": ["good", "bad", "skip"] }, "revision": full_oid }
                            },
                            {
                                "type": "object", "required": ["action"], "additionalProperties": false,
                                "properties": { "cwd": cwd, "action": { "const": "reset" } }
                            }
                        ]
                    }),
                    "@git.bisect {cwd: \"tempestmiku:\", action: \"start\", bad: \"0123456789012345678901234567890123456789\", good: [\"abcdefabcdefabcdefabcdefabcdefabcdefabcd\"]}",
                    "Mark without revision uses the pinned BISECT_HEAD; every transition is no-checkout and approval-gated.",
                    "always",
                    "Approval-backed closed Git bisect state transition.",
                ),
                "git.commit" => (
                    "After approval, commit exactly the currently staged index with the supplied bounded message. It never stages files and disables hooks and signing.",
                    "@git.commit GitCommitArgs -> GitResult",
                    json!({
                        "type": "object", "required": ["message"], "additionalProperties": false,
                        "properties": { "cwd": cwd, "message": { "type": "string", "minLength": 1, "maxLength": 4096 } }
                    }),
                    "@git.commit {cwd: \"tempestmiku:\", message: \"fix(host/git): harden upstream checks\"}",
                    "The host owns every Git argument except the bounded commit message.",
                    "always",
                    "Approval-backed commit of the staged linked-repository index.",
                ),
                "git.push" => (
                    "After approval, push HEAD to the current branch's configured HTTPS upstream. Remote/ref selection, force, tags, URL rewrites, credential helpers, prompts, and non-HTTPS transports are rejected.",
                    "@git.push GitCwdArgs -> GitResult",
                    json!({ "type": "object", "additionalProperties": false, "properties": { "cwd": cwd } }),
                    "@git.push {cwd: \"tempestmiku:\"}",
                    "Push targets only the current configured credential-free HTTPS upstream.",
                    "always",
                    "Approval-backed push to the exact configured HTTPS upstream.",
                ),
                "git.pull" => (
                    "After approval, fast-forward the current branch from its configured HTTPS upstream. Rebase, merge commits, remote/ref selection, URL rewrites, credential helpers, prompts, and non-HTTPS transports are rejected.",
                    "@git.pull GitCwdArgs -> GitResult",
                    json!({ "type": "object", "additionalProperties": false, "properties": { "cwd": cwd } }),
                    "@git.pull {cwd: \"tempestmiku:\"}",
                    "Pull is fixed to a fast-forward from the current credential-free HTTPS upstream.",
                    "always",
                    "Approval-backed fast-forward from the exact configured HTTPS upstream.",
                ),
                _ => unreachable!("the outer exact-name match restricts Git documentation"),
            };
            let mut errors = vec![
                tool_error("CapabilityDeniedError", "The exact grant, linked-project authority, writable policy, approved HTTPS endpoint, or usable Git executable is missing.", false),
                tool_error("InvalidArgsError", "Arguments, paths, object IDs, repository state, or upstream configuration violate the fixed contract.", false),
                tool_error("InvalidPathError", "cwd or an exact path is outside the linked root, missing, or has the wrong kind.", false),
                tool_error("TimeoutError", "The fixed Git operation exceeds its host timeout.", true),
                tool_error("HostCallError", "Git cannot be spawned or collected.", false),
            ];
            if approval == "always" {
                errors.insert(1, tool_error("ApprovalDeniedError", "The owner denies this approval-gated Git operation.", false));
                errors.insert(2, tool_error("ApprovalTimeoutError", "The approval request times out and defaults to deny.", true));
            }
            ToolDocs {
                description: Some(description.to_string()),
                signature: signature.to_string(),
                args_schema,
                result_schema: Some(json!({
                    "type": "object",
                    "required": ["operation", "cwd", "exitCode", "stdout", "stderr", "truncated", "durationMs"],
                    "additionalProperties": false,
                    "properties": {
                        "operation": { "type": "string" }, "cwd": { "type": "string" },
                        "exitCode": { "type": "integer" }, "stdout": { "type": "string" },
                        "stderr": { "type": "string" }, "truncated": { "type": "boolean" },
                        "artifact": { "type": ["object", "null"] }, "durationMs": { "type": "integer", "minimum": 0 }
                    }
                })),
                examples: vec![ToolExample {
                    title: Some(format!("Call {name}")),
                    code: example.to_string(),
                    notes: Some(notes.to_string()),
                }],
                errors,
                grants: vec![linked_grant(grant)],
                approval: approval.to_string(),
                ..base
            }
        }
        "proc.run" => ToolDocs {
            description: Some(
                "Run an allowlisted command with argv-vector arguments inside a linked-folder cwd; optional UTF-8 string stdin is capped at 1 MiB, while shell strings, pipes, redirects, and env overrides are rejected."
                    .to_string(),
            ),
            signature: "@proc.run ProcRunArgs -> ProcOutput"
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
                    "stdin": { "type": "string", "maxLength": 1048576, "description": "Optional UTF-8 process input, capped by encoded byte length at 1 MiB." }
                }
            }),
            result_schema: Some(proc_output_schema()),
            examples: vec![ToolExample {
                title: Some("Run tests".to_string()),
                code: "let run = @proc.run {cmd: \"cargo\", args: [\"test\"], cwd: \"tempestmiku:\"};\nrun |> display {kind: \"json\"}"
                    .to_string(),
                notes: Some("Use cmd plus args. Do not pass proc.run('cargo test').".to_string()),
            }],
            errors: vec![
                tool_error("CapabilityDeniedError", "proc.run is not granted or the command is not allowlisted for the linked cwd.", false),
                tool_error("ApprovalDeniedError", "The user denies process execution. Every command requires approval; optional Linux isolation is defense in depth.", false),
                tool_error("ApprovalTimeoutError", "The approval request times out and defaults to deny.", true),
                tool_error("InvalidArgsError", "cmd is not a bare executable, stdin is not a string or exceeds 1 MiB, or env/shell-style args are requested.", false),
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

pub fn linked_tool_docs(name: &str) -> Option<ToolDocs> {
    let (namespace, summary) = match name {
        "fs.read" => ("fs", "Read UTF-8 text from a linked folder"),
        "fs.write" => ("fs", "Write UTF-8 text under a writable linked folder"),
        "fs.patch" => ("fs", "Atomically patch a linked UTF-8 file"),
        "fs.move" => ("fs", "Move a linked file"),
        "fs.remove" => ("fs", "Remove a linked file"),
        "fs.ls" => ("fs", "List linked-folder entries"),
        "fs.find" => ("fs", "Find linked-folder entries"),
        "fs.grep" => ("fs", "Search UTF-8 linked files"),
        "git.status" => ("git", "Inspect linked repository status"),
        "git.diff" => ("git", "Inspect linked repository diff"),
        "git.log" => ("git", "Inspect recent repository history"),
        "git.commit" => ("git", "Commit the staged repository index"),
        "git.push" => ("git", "Push HEAD to the configured HTTPS upstream"),
        "git.pull" => ("git", "Fast-forward from the configured HTTPS upstream"),
        "git.clone" => (
            "git",
            "Clone an HTTPS repository into an empty linked folder",
        ),
        "git.init" => ("git", "Initialize a linked Git repository"),
        "git.add" => ("git", "Stage exact linked-repository paths"),
        "git.mv" => ("git", "Move an exact linked-repository entry"),
        "git.restore" => ("git", "Restore exact linked-repository paths"),
        "git.rm" => ("git", "Remove exact linked-repository paths"),
        "git.bisect" => ("git", "Advance a closed Git bisect transition"),
        "git.grep" => ("git", "Search tracked repository content"),
        "git.show" => ("git", "Inspect a snapshotted Git object"),
        "proc.run" => ("proc", "Run allowlisted argv-vector commands"),
        _ => return None,
    };
    Some(docs(name, namespace, summary, true))
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

fn write_errors(capability: &str, approval_action: &str) -> Vec<ToolErrorDoc> {
    vec![
        tool_error(
            "CapabilityDeniedError",
            &format!("The session lacks the {capability} grant or the linked folder is read-only."),
            false,
        ),
        tool_error(
            "ApprovalDeniedError",
            &format!("The user denies {approval_action}."),
            false,
        ),
        tool_error(
            "ApprovalTimeoutError",
            "The approval request times out and defaults to deny.",
            true,
        ),
        tool_error(
            "InvalidPathError",
            "The linked alias is unknown, the path escapes the root, or the target is not a file.",
            false,
        ),
        tool_error(
            "InvalidArgsError",
            "The tag is stale or arguments do not match the schema.",
            false,
        ),
        tool_error(
            "HostCallError",
            "The filesystem operation fails after policy checks.",
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

fn search_match_schema() -> Value {
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
            "tag": { "type": "string", "description": "Optimistic concurrency tag for fs.patch, fs.move, or fs.remove." }
        }
    })
}

fn fs_patch_args_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "tag", "hunks"],
        "additionalProperties": false,
        "properties": {
            "path": { "type": "string" },
            "tag": { "type": "string", "description": "Required fresh tag for the existing file." },
            "hunks": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["op", "startLine", "endLine", "expectedLines", "lines"],
                            "properties": {
                                "op": { "const": "replace" },
                                "startLine": { "type": "integer", "minimum": 1 },
                                "endLine": { "type": "integer", "minimum": 1 },
                                "expectedLines": { "type": "array", "minItems": 1, "items": { "type": "string" }, "description": "Exact current lines in the selected range." },
                                "lines": { "type": "array", "items": { "type": "string" } }
                            }
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["op", "startLine", "endLine", "expectedLines"],
                            "properties": {
                                "op": { "const": "delete" },
                                "startLine": { "type": "integer", "minimum": 1 },
                                "endLine": { "type": "integer", "minimum": 1 },
                                "expectedLines": { "type": "array", "minItems": 1, "items": { "type": "string" }, "description": "Exact current lines in the selected range." }
                            }
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["op", "line", "expectedLine", "lines"],
                            "properties": {
                                "op": { "enum": ["insertBefore", "insertAfter"] },
                                "line": { "type": "integer", "minimum": 1 },
                                "expectedLine": { "type": "string", "description": "Exact current anchor line." },
                                "lines": { "type": "array", "items": { "type": "string" } }
                            }
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["op", "lines"],
                            "properties": {
                                "op": { "enum": ["prepend", "append"] },
                                "lines": { "type": "array", "items": { "type": "string" } }
                            }
                        }
                    ]
                }
            }
        }
    })
}

fn fs_patch_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "changed", "newTag", "summary", "diffPreview", "truncated"],
        "properties": {
            "path": { "type": "string" },
            "changed": { "type": "boolean" },
            "newTag": { "type": "string" },
            "summary": { "type": "string" },
            "diffPreview": { "type": "string" },
            "diffArtifact": {
                "oneOf": [artifact_ref_schema(), { "type": "null" }]
            },
            "truncated": { "type": "boolean" }
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

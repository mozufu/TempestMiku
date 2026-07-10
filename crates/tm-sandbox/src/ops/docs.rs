use super::*;

pub(super) fn core_tool_docs() -> BTreeMap<String, ToolDocs> {
    [
        core_doc(
            "tools.search",
            "tools",
            "Search the runtime capability catalog",
            "tools.search(query: string, opts?: ToolSearchOptions): Promise<ToolSummary[]>",
            "Search the runtime capability catalog without loading the whole SDK into the model context. Results include host-dispatched capabilities plus docs-only entries for core direct namespace methods.",
            json!({
                "type": "object",
                "required": ["query"],
                "additionalProperties": false,
                "properties": {
                    "query": { "type": "string" },
                    "namespace": { "type": "string", "description": "Optional namespace filter such as fs, code, resources, artifacts, proc, http, or tools." },
                    "limit": { "type": "integer", "minimum": 1, "default": 20 }
                }
            }),
            Some(json!({ "type": "array", "items": tool_summary_schema() })),
            vec![ToolExample {
                title: Some("Find edit capabilities".to_string()),
                code: "const found = await tools.search('edit', { namespace: 'code' });\ndisplay(found, { kind: 'json' });".to_string(),
                notes: Some("Search returns summaries only; call tools.docs(name) for the full SDK contract.".to_string()),
            }],
            vec![tool_error(
                "InvalidArgsError",
                "The query or search options cannot be serialized into the catalog search request.",
                false,
            )],
            vec![GrantDoc {
                kind: "catalog".to_string(),
                description: "Catalog search is available inside the sandbox; result grants describe each returned capability.".to_string(),
            }],
        ),
        core_doc(
            "tools.docs",
            "tools",
            "Read docs for one runtime capability",
            "tools.docs(name: CapabilityName): Promise<ToolDocs>",
            "Return the full SDK contract for one catalog entry: signature, schemas, examples, fail-closed errors, grants, approval policy, since, and stability.",
            json!({
                "type": "object",
                "required": ["name"],
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string", "description": "Capability or direct namespace method name such as fs.read, resources.read, or tools.search." }
                }
            }),
            Some(tool_docs_schema()),
            vec![ToolExample {
                title: Some("Read docs for fs.read".to_string()),
                code: "const docs = await tools.docs('fs.read');\ndisplay(docs.signature);".to_string(),
                notes: Some("Unknown names fail closed with NotFoundError.".to_string()),
            }],
            vec![
                tool_error("NotFoundError", "The requested catalog entry does not exist.", false),
                tool_error(
                    "InvalidArgsError",
                    "The capability name cannot be serialized into the docs request.",
                    false,
                ),
            ],
            vec![GrantDoc {
                kind: "catalog".to_string(),
                description: "Catalog docs lookup is available inside the sandbox; the returned docs describe any target grants.".to_string(),
            }],
        ),
        core_doc(
            "tools.call",
            "tools",
            "Dispatch a capability-gated host call",
            "tools.call<T = unknown>(name: CapabilityName, args?: JsonValue): Promise<T>",
            "Dispatch a capability-gated host call by name. Prefer typed namespace wrappers when one exists. Unknown or ungranted capabilities fail closed before the host function runs.",
            json!({
                "type": "object",
                "required": ["name"],
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string" },
                    "args": { "description": "JSON-compatible arguments for the named capability." }
                }
            }),
            None,
            vec![ToolExample {
                title: Some("Call a capability directly".to_string()),
                code: "const doc = await tools.call('fs.read', { path: 'tempestmiku:README.md' });".to_string(),
                notes: Some("The typed fs.read(...) wrapper is preferred when available.".to_string()),
            }],
            vec![
                tool_error("CapabilityDeniedError", "The named capability is unknown or not granted.", false),
                tool_error("InvalidArgsError", "The args do not match the named capability schema.", false),
                tool_error("HostCallError", "The host capability fails after policy checks.", false),
            ],
            vec![GrantDoc {
                kind: "capability".to_string(),
                description: "Requires the grant for the named capability; tools.call itself does not bypass capability checks.".to_string(),
            }],
        ),
        core_doc(
            "resources.read",
            "resources",
            "Read a registered resource URI",
            "resources.read(uri: ResourceUri, selector?: ResourceSelector): Promise<ResourceContent>",
            "Read a URI through the scheme-dispatched resource registry. Current handlers cover artifact:// in every Deno session and can cover linked:// plus the P2 memory:// surface when the host registers those handlers. Scheme-specific grants such as resources.read:artifact, resources.read:linked, and resources.read:memory still apply. skill:// labels are prompt-composition-only until the P4/P7 skill lifecycle registers a handler, so reading them fails closed as an unknown scheme.",
            json!({
                "type": "object",
                "required": ["uri"],
                "additionalProperties": false,
                "properties": {
                    "uri": { "type": "string" },
                    "selector": { "type": "string", "description": "Optional resource selector such as a 1-based line range." }
                }
            }),
            Some(resource_content_schema()),
            vec![ToolExample {
                title: Some("Read artifact lines".to_string()),
                code: "const ref = artifacts.put('one\\ntwo');\nconst content = await resources.read(ref.uri, '2-2');\ndisplay(content.content);".to_string(),
                notes: Some("Unknown schemes and missing scheme grants fail closed with CapabilityDeniedError.".to_string()),
            }],
            resource_errors("resources.read"),
            vec![
                GrantDoc {
                    kind: "artifact".to_string(),
                    description: "Read access to artifact:// session artifacts through resources.read:artifact.".to_string(),
                },
                GrantDoc {
                    kind: "linked-folder".to_string(),
                    description: "Read access to linked:// resources when a linked-folder handler is registered and resources.read:linked is granted.".to_string(),
                },
                GrantDoc {
                    kind: "memory".to_string(),
                    description: "Read access to the P2 memory:// resource gateway when a memory handler is registered and resources.read:memory is granted.".to_string(),
                },
            ],
        ),
        core_doc(
            "resources.preview",
            "resources",
            "Preview a registered resource URI",
            "resources.preview(uri: ResourceUri): Promise<ResourceContent>",
            "Return a ResourceContent envelope with preview metadata for a registered resource URI. Uses the same scheme-specific resource grants as resources.read.",
            json!({
                "type": "object",
                "required": ["uri"],
                "additionalProperties": false,
                "properties": {
                    "uri": { "type": "string" }
                }
            }),
            Some(resource_content_schema()),
            vec![ToolExample {
                title: Some("Preview an artifact".to_string()),
                code: "const ref = artifacts.put('long text');\nconst preview = await resources.preview(ref.uri);".to_string(),
                notes: None,
            }],
            resource_errors("resources.preview"),
            vec![
                GrantDoc {
                    kind: "artifact".to_string(),
                    description: "Preview access to artifact:// session artifacts through resources.read:artifact.".to_string(),
                },
                GrantDoc {
                    kind: "linked-folder".to_string(),
                    description: "Preview access to linked:// resources when resources.read:linked is granted.".to_string(),
                },
                GrantDoc {
                    kind: "memory".to_string(),
                    description: "Preview access to memory:// resources when resources.read:memory is granted.".to_string(),
                },
            ],
        ),
        core_doc(
            "resources.list",
            "resources",
            "List registered resource schemes or entries",
            "resources.list(uri?: ResourceUri): Promise<ResourceEntry[]>",
            "List registered resource schemes, or entries beneath a URI when that scheme supports listing. Listing a specific URI uses that scheme's resource grant.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "uri": { "type": "string", "description": "Omit to list registered schemes." }
                }
            }),
            Some(json!({ "type": "array", "items": resource_entry_schema() })),
            vec![ToolExample {
                title: Some("List schemes".to_string()),
                code: "const schemes = await resources.list();\ndisplay(schemes, { kind: 'json' });".to_string(),
                notes: None,
            }],
            resource_errors("resources.list"),
            vec![
                GrantDoc {
                    kind: "artifact".to_string(),
                    description: "List artifact:// entries through resources.read:artifact.".to_string(),
                },
                GrantDoc {
                    kind: "linked-folder".to_string(),
                    description: "List linked:// entries when resources.read:linked is granted.".to_string(),
                },
                GrantDoc {
                    kind: "memory".to_string(),
                    description: "List memory:// entries when resources.read:memory is granted.".to_string(),
                },
            ],
        ),
        core_doc(
            "artifacts.put",
            "artifacts",
            "Store session-local text or JSON",
            "artifacts.put(data: ArtifactInput, opts?: ArtifactPutOptions): ArtifactRef",
            "Store a session-local text or JSON artifact and return an artifact:// handle. P0 artifacts are text-backed.",
            json!({
                "type": "object",
                "required": ["data"],
                "additionalProperties": false,
                "properties": {
                    "data": {},
                    "title": { "type": "string" },
                    "mime": { "type": "string", "default": "text/plain" },
                    "kind": { "type": "string", "description": "Reserved metadata hint in P0." },
                    "filename": { "type": "string", "description": "Reserved metadata hint in P0." }
                }
            }),
            Some(artifact_ref_schema()),
            vec![ToolExample {
                title: Some("Create an artifact".to_string()),
                code: "const ref = artifacts.put('notes\\n', { title: 'notes' });\ndisplay(ref.uri);".to_string(),
                notes: None,
            }],
            vec![tool_error("HostCallError", "The artifact store cannot write the artifact.", false)],
            vec![GrantDoc {
                kind: "artifact".to_string(),
                description: "Session-local artifact writes are always available inside the sandbox.".to_string(),
            }],
        ),
        core_doc(
            "artifacts.get",
            "artifacts",
            "Read a session artifact",
            "artifacts.get(ref: ArtifactUri | ArtifactRef, opts?: ArtifactReadOptions): Promise<ResourceContent>",
            "Read a session artifact by artifact:// URI or ArtifactRef and return a ResourceContent envelope.",
            json!({
                "type": "object",
                "required": ["ref"],
                "additionalProperties": false,
                "properties": {
                    "ref": {},
                    "selector": { "type": "string", "description": "Optional 1-based inclusive line range." }
                }
            }),
            Some(resource_content_schema()),
            vec![ToolExample {
                title: Some("Read an artifact".to_string()),
                code: "const ref = artifacts.put('one\\ntwo');\nconst content = await artifacts.get(ref, { selector: '1-1' });".to_string(),
                notes: None,
            }],
            resource_errors("artifacts.get"),
            vec![GrantDoc {
                kind: "artifact".to_string(),
                description: "Reads use the resources.read:artifact grant.".to_string(),
            }],
        ),
        core_doc(
            "artifacts.slice",
            "artifacts",
            "Read a selected artifact slice",
            "artifacts.slice(ref: ArtifactUri | ArtifactRef, selector: ResourceSelector): Promise<ResourceContent>",
            "Read a selected range from a session artifact.",
            json!({
                "type": "object",
                "required": ["ref", "selector"],
                "additionalProperties": false,
                "properties": {
                    "ref": {},
                    "selector": { "type": "string" }
                }
            }),
            Some(resource_content_schema()),
            vec![ToolExample {
                title: Some("Slice an artifact".to_string()),
                code: "const ref = artifacts.put('one\\ntwo');\nconst line = await artifacts.slice(ref, '2-2');".to_string(),
                notes: None,
            }],
            resource_errors("artifacts.slice"),
            vec![GrantDoc {
                kind: "artifact".to_string(),
                description: "Reads use the resources.read:artifact grant.".to_string(),
            }],
        ),
        core_doc(
            "artifacts.list",
            "artifacts",
            "List session artifacts",
            "artifacts.list(): ArtifactRef[]",
            "List artifact handles created in the current session.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }),
            Some(json!({ "type": "array", "items": artifact_ref_schema() })),
            vec![ToolExample {
                title: Some("List artifacts".to_string()),
                code: "const refs = artifacts.list();\ndisplay(refs, { kind: 'json' });".to_string(),
                notes: None,
            }],
            vec![tool_error("HostCallError", "The artifact store cannot list artifacts.", false)],
            vec![GrantDoc {
                kind: "artifact".to_string(),
                description: "Session-local artifact listing is always available inside the sandbox.".to_string(),
            }],
        ),
    ]
    .into_iter()
    .map(|docs| (docs.name.clone(), docs))
    .collect()
}

#[allow(clippy::too_many_arguments)]
fn core_doc(
    name: &str,
    namespace: &str,
    summary: &str,
    signature: &str,
    description: &str,
    args_schema: Value,
    result_schema: Option<Value>,
    examples: Vec<ToolExample>,
    errors: Vec<ToolErrorDoc>,
    grants: Vec<GrantDoc>,
) -> ToolDocs {
    ToolDocs {
        name: name.to_string(),
        namespace: namespace.to_string(),
        summary: summary.to_string(),
        description: Some(description.to_string()),
        signature: signature.to_string(),
        args_schema,
        result_schema,
        examples,
        errors,
        grants,
        sensitive: false,
        approval: "none".to_string(),
        since: "M1".to_string(),
        stability: "experimental".to_string(),
    }
}

fn resource_errors(capability: &str) -> Vec<ToolErrorDoc> {
    vec![
        tool_error(
            "CapabilityDeniedError",
            &format!("{capability} is not granted for the requested resource scheme."),
            false,
        ),
        tool_error(
            "NotFoundError",
            "The resource or artifact does not exist.",
            false,
        ),
        tool_error(
            "InvalidArgsError",
            "The URI, selector, or arguments are malformed.",
            false,
        ),
    ]
}

fn tool_error(name: &str, when: &str, retryable: bool) -> ToolErrorDoc {
    ToolErrorDoc {
        name: name.to_string(),
        when: when.to_string(),
        retryable,
    }
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

fn resource_entry_schema() -> Value {
    json!({
        "type": "object",
        "required": ["uri", "name", "kind"],
        "properties": {
            "uri": { "type": "string" },
            "name": { "type": "string" },
            "kind": { "type": "string" },
            "title": { "type": ["string", "null"] },
            "sizeBytes": { "type": ["integer", "null"] },
            "modifiedAt": { "type": ["string", "null"] }
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

fn tool_summary_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name", "namespace", "summary", "sensitive", "granted"],
        "properties": {
            "name": { "type": "string" },
            "namespace": { "type": "string" },
            "summary": { "type": "string" },
            "sensitive": { "type": "boolean" },
            "granted": { "type": "boolean" }
        }
    })
}

fn tool_docs_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name", "namespace", "summary", "signature", "argsSchema", "examples", "errors", "grants", "sensitive", "approval", "since", "stability"],
        "properties": {
            "name": { "type": "string" },
            "namespace": { "type": "string" },
            "summary": { "type": "string" },
            "description": { "type": ["string", "null"] },
            "signature": { "type": "string" },
            "argsSchema": { "type": "object" },
            "resultSchema": { "type": ["object", "null"] },
            "examples": { "type": "array" },
            "errors": { "type": "array" },
            "grants": { "type": "array" },
            "sensitive": { "type": "boolean" },
            "approval": { "type": "string" },
            "since": { "type": "string" },
            "stability": { "type": "string" }
        }
    })
}

pub(super) fn core_doc_granted(name: &str, ctx: &InvocationCtx) -> bool {
    match name {
        "tools.search" | "tools.docs" | "tools.call" => true,
        "artifacts.put" | "artifacts.list" | "resources.list" => true,
        "artifacts.get" | "artifacts.slice" => ctx.grants.permits("resources.read:artifact"),
        "resources.read" | "resources.preview" => ctx
            .grants
            .names()
            .any(|grant| grant.starts_with("resources.read:")),
        _ => ctx.grants.permits(name),
    }
}

pub(super) fn core_doc_matches(docs: &ToolDocs, query: &str, namespace: Option<&str>) -> bool {
    if let Some(namespace) = namespace
        && docs.namespace != namespace
    {
        return false;
    }
    let needle = query.to_lowercase();
    let haystack = format!("{} {} {}", docs.name, docs.namespace, docs.summary).to_lowercase();
    needle.is_empty() || haystack.contains(&needle)
}

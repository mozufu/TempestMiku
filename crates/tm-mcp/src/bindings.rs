use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tm_artifacts::ResourceContent;
use tm_host::{
    GrantDoc, HostError, HostFn, HostRegistry, InvocationCtx, ResourceEntry, ResourceHandler,
    ResourceRegistry, ToolDocs, ToolErrorDoc,
};
use url::Url;

use crate::{
    MCP_PROTOCOL_VERSION, McpCatalogManager, McpError, McpMutationEffectRecord,
    McpMutationEffectStatus, McpMutationEffectStore, McpMutationIntent, McpPromptArgumentView,
    McpProvenance, Result, UntrustedMcpData,
    catalog::{ImportedPrompt, ImportedResource, ImportedTool, McpRuntime},
    validate::{ensure_value_bound, sha256_hex, validate_schema_instance, value_digest, value_len},
};

pub struct McpBindings {
    generation: u64,
    digest: String,
    host_functions: Vec<Arc<dyn HostFn>>,
    resource_handler: Arc<McpResourceHandler>,
}

impl McpBindings {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn host_functions(&self) -> &[Arc<dyn HostFn>] {
        &self.host_functions
    }

    pub fn resource_handler(&self) -> Arc<McpResourceHandler> {
        Arc::clone(&self.resource_handler)
    }

    /// Install the host's durable mutation-effect boundary. Server wiring calls this before the
    /// first user turn; the CLI intentionally retains the process-local implementation.
    pub fn install_mutation_effect_store(&self, store: Arc<dyn McpMutationEffectStore>) {
        *self.resource_handler.runtime.mutation_effects.write() = store;
    }

    pub fn register_into(
        &self,
        host_registry: &mut HostRegistry,
        resource_registry: &mut ResourceRegistry,
    ) -> Result<()> {
        let inspection = InvocationCtx::new(tm_host::CapabilityGrants::default());
        let occupied = host_registry
            .search("", None, usize::MAX, &inspection)
            .into_iter()
            .map(|summary| summary.name)
            .collect::<BTreeSet<_>>();
        if let Some(name) = self
            .host_functions
            .iter()
            .map(|function| function.name())
            .find(|name| occupied.contains(*name))
        {
            return Err(McpError::Collision(format!(
                "imported capability {name} collides with an existing host function"
            )));
        }
        if resource_registry
            .schemes()
            .iter()
            .any(|scheme| scheme == "mcp")
        {
            return Err(McpError::Collision(
                "resource scheme mcp is already registered".to_string(),
            ));
        }
        for function in &self.host_functions {
            host_registry.register(Arc::clone(function));
        }
        resource_registry.register(self.resource_handler());
        Ok(())
    }
}

impl McpCatalogManager {
    pub fn bindings(&self) -> Result<McpBindings> {
        self.bindings_checked(&BTreeSet::new())
    }

    /// Build host/resource adapters while refusing to replace any occupied host capability name.
    /// Callers should pass the names already registered in their static host registry.
    pub fn bindings_checked(&self, occupied: &BTreeSet<String>) -> Result<McpBindings> {
        let snapshot = self.runtime.snapshot();
        if let Some(name) = snapshot
            .tools
            .keys()
            .chain(snapshot.prompts.keys())
            .find(|name| occupied.contains(*name))
        {
            return Err(McpError::Collision(format!(
                "imported capability {name} collides with an existing host function"
            )));
        }
        let mut host_functions = Vec::<Arc<dyn HostFn>>::new();
        for tool in snapshot.tools.values() {
            host_functions.push(Arc::new(McpToolFn::new(
                Arc::clone(&self.runtime),
                snapshot.view.generation,
                snapshot.view.digest.clone(),
                tool.clone(),
            )));
        }
        for prompt in snapshot.prompts.values() {
            host_functions.push(Arc::new(McpPromptFn::new(
                Arc::clone(&self.runtime),
                snapshot.view.generation,
                snapshot.view.digest.clone(),
                prompt.clone(),
            )));
        }
        Ok(McpBindings {
            generation: snapshot.view.generation,
            digest: snapshot.view.digest.clone(),
            host_functions,
            resource_handler: Arc::new(McpResourceHandler {
                runtime: Arc::clone(&self.runtime),
            }),
        })
    }
}

struct McpToolFn {
    runtime: Arc<McpRuntime>,
    generation: u64,
    catalog_digest: String,
    imported: ImportedTool,
    docs: ToolDocs,
}

impl McpToolFn {
    fn new(
        runtime: Arc<McpRuntime>,
        generation: u64,
        catalog_digest: String,
        imported: ImportedTool,
    ) -> Self {
        let approval = if imported.mutation { "manual" } else { "none" };
        let docs = ToolDocs {
            name: imported.capability.clone(),
            namespace: format!("mcp.{}", imported.server),
            summary: format!(
                "Call allowlisted MCP tool {} on server {}; remote descriptions are withheld as untrusted data.",
                imported.name, imported.server
            ),
            description: None,
            signature: format!("@{} args -> UntrustedMcpData", imported.capability),
            args_schema: imported.disclosed_input_schema.clone(),
            result_schema: Some(untrusted_result_schema()),
            examples: Vec::new(),
            errors: common_errors(imported.mutation),
            grants: vec![GrantDoc {
                kind: "exact".to_string(),
                description: format!("Requires exact capability {}", imported.capability),
            }],
            sensitive: true,
            approval: approval.to_string(),
            since: "P10".to_string(),
            stability: "experimental".to_string(),
        };
        Self {
            runtime,
            generation,
            catalog_digest,
            imported,
            docs,
        }
    }
}

#[async_trait]
impl HostFn for McpToolFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        require_exact_grant(ctx, &self.imported.capability)?;
        if !args.is_object() {
            return Err(HostError::InvalidArgs(
                "MCP tool arguments must be an object".to_string(),
            ));
        }
        let args_bytes = ensure_value_bound(
            &args,
            self.runtime.bounds.max_request_bytes,
            "MCP tool arguments",
        )
        .map_err(host_error)?;
        validate_schema_instance(
            &self.imported.server,
            &self.imported.input_schema,
            &args,
            "tool arguments",
        )
        .map_err(host_error)?;
        let args_digest = value_digest(&args).map_err(host_error)?;
        let snapshot = self.runtime.snapshot();
        require_binding_snapshot(&snapshot, self.generation, &self.catalog_digest)
            .map_err(host_error)?;
        let active = snapshot
            .tools
            .get(&self.imported.capability)
            .filter(|active| active.target_digest == self.imported.target_digest)
            .ok_or_else(|| HostError::CapabilityDenied(self.imported.capability.clone()))?;

        emit_invocation(
            ctx,
            &snapshot.view,
            active,
            &args_digest,
            args_bytes,
            "requested",
            None,
        )
        .await?;
        if active.mutation {
            let preview = mutation_preview(&args);
            let approval = ctx
                .require_approval(&format!(
                    "MCP mutation {}/{} target={} input={}",
                    active.server, active.name, active.target_digest, preview
                ))
                .await;
            if let Err(error) = approval {
                emit_invocation(
                    ctx,
                    &snapshot.view,
                    active,
                    &args_digest,
                    args_bytes,
                    "denied",
                    None,
                )
                .await?;
                return Err(error);
            }
        }
        let mutation_effect = if active.mutation {
            match begin_mutation_effect(
                &self.runtime,
                ctx,
                &snapshot.view,
                active,
                &args_digest,
                args_bytes,
            )
            .await
            .map_err(host_error)?
            {
                MutationExecution::Execute(record) => Some(record),
                MutationExecution::Replay(record) => {
                    let envelope = mutation_replay_envelope(&snapshot.view, active, &record)
                        .map_err(host_error)?;
                    emit_invocation(
                        ctx,
                        &snapshot.view,
                        active,
                        &args_digest,
                        args_bytes,
                        "replayed",
                        Some(&envelope.provenance),
                    )
                    .await?;
                    return serde_json::to_value(envelope).map_err(|error| {
                        HostError::HostCall(format!("MCP envelope encoding failed: {error}"))
                    });
                }
            }
        } else {
            None
        };
        let result = self
            .runtime
            .rpc(
                ctx,
                &active.server,
                "tools/call",
                json!({ "name": active.name, "arguments": args }),
            )
            .await;
        let result = match result {
            Ok(result) => result,
            Err(McpError::Rpc { code, digest, .. }) => {
                let data = json!({
                    "kind": "mcp_remote_error",
                    "trust": "untrusted",
                    "code": code,
                    "errorDigest": digest,
                });
                let envelope = untrusted_envelope(
                    &snapshot.view,
                    &active.server,
                    "tool_error",
                    &active.name,
                    &active.target_digest,
                    data,
                )
                .map_err(host_error)?;
                if let Some(effect) = &mutation_effect {
                    finish_mutation_effect(
                        &self.runtime,
                        &effect.intent.effect_id,
                        McpMutationEffectStatus::Failed,
                        Some(&envelope.provenance.payload_digest),
                        Some(envelope.provenance.payload_bytes),
                        Some("remote_rpc_error"),
                        Some(&digest),
                    )
                    .await
                    .map_err(host_error)?;
                }
                emit_invocation(
                    ctx,
                    &snapshot.view,
                    active,
                    &args_digest,
                    args_bytes,
                    "failed",
                    Some(&envelope.provenance),
                )
                .await?;
                return serde_json::to_value(envelope).map_err(|error| {
                    HostError::HostCall(format!("MCP envelope encoding failed: {error}"))
                });
            }
            Err(error) => {
                mark_mutation_uncertain(&self.runtime, mutation_effect.as_ref(), &error).await?;
                return Err(host_error(error));
            }
        };
        if let Err(error) = validate_tool_result(active, &result, &self.runtime.bounds) {
            mark_mutation_uncertain(&self.runtime, mutation_effect.as_ref(), &error).await?;
            return Err(host_error(error));
        }
        let tool_reported_error = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let envelope = untrusted_envelope(
            &snapshot.view,
            &active.server,
            "tool",
            &active.name,
            &active.target_digest,
            result,
        )
        .map_err(host_error)?;
        if let Some(effect) = &mutation_effect {
            finish_mutation_effect(
                &self.runtime,
                &effect.intent.effect_id,
                if tool_reported_error {
                    McpMutationEffectStatus::Failed
                } else {
                    McpMutationEffectStatus::Succeeded
                },
                Some(&envelope.provenance.payload_digest),
                Some(envelope.provenance.payload_bytes),
                tool_reported_error.then_some("tool_execution_error"),
                None,
            )
            .await
            .map_err(host_error)?;
        }
        emit_invocation(
            ctx,
            &snapshot.view,
            active,
            &args_digest,
            args_bytes,
            "completed",
            Some(&envelope.provenance),
        )
        .await?;
        serde_json::to_value(envelope)
            .map_err(|error| HostError::HostCall(format!("MCP envelope encoding failed: {error}")))
    }
}

struct McpPromptFn {
    runtime: Arc<McpRuntime>,
    generation: u64,
    catalog_digest: String,
    imported: ImportedPrompt,
    docs: ToolDocs,
}

impl McpPromptFn {
    fn new(
        runtime: Arc<McpRuntime>,
        generation: u64,
        catalog_digest: String,
        imported: ImportedPrompt,
    ) -> Self {
        let docs = ToolDocs {
            name: imported.capability.clone(),
            namespace: format!("mcp.{}.prompts", imported.server),
            summary: format!(
                "Fetch allowlisted MCP prompt {} as explicit untrusted data; it is never a system prompt.",
                imported.name
            ),
            description: None,
            signature: format!("@{} args -> UntrustedMcpData", imported.capability),
            args_schema: prompt_args_schema(&imported.arguments),
            result_schema: Some(untrusted_result_schema()),
            examples: Vec::new(),
            errors: common_errors(false),
            grants: vec![GrantDoc {
                kind: "exact".to_string(),
                description: format!("Requires exact capability {}", imported.capability),
            }],
            sensitive: true,
            approval: "none".to_string(),
            since: "P10".to_string(),
            stability: "experimental".to_string(),
        };
        Self {
            runtime,
            generation,
            catalog_digest,
            imported,
            docs,
        }
    }
}

#[async_trait]
impl HostFn for McpPromptFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        require_exact_grant(ctx, &self.imported.capability)?;
        validate_prompt_args(&self.imported, &args, &self.runtime.bounds)?;
        let args_bytes = value_len(&args).map_err(host_error)?;
        let args_digest = value_digest(&args).map_err(host_error)?;
        let snapshot = self.runtime.snapshot();
        require_binding_snapshot(&snapshot, self.generation, &self.catalog_digest)
            .map_err(host_error)?;
        let active = snapshot
            .prompts
            .get(&self.imported.capability)
            .filter(|active| active.target_digest == self.imported.target_digest)
            .ok_or_else(|| HostError::CapabilityDenied(self.imported.capability.clone()))?;
        emit_prompt_audit(
            ctx,
            &snapshot.view,
            active,
            &args_digest,
            args_bytes,
            "requested",
            None,
        )
        .await?;
        let result = self
            .runtime
            .rpc(
                ctx,
                &active.server,
                "prompts/get",
                json!({ "name": active.name, "arguments": args }),
            )
            .await;
        let result = match result {
            Ok(result) => result,
            Err(McpError::Rpc { code, digest, .. }) => {
                let envelope = untrusted_envelope(
                    &snapshot.view,
                    &active.server,
                    "prompt_error",
                    &active.name,
                    &active.target_digest,
                    json!({
                        "kind": "mcp_remote_error",
                        "trust": "untrusted",
                        "code": code,
                        "errorDigest": digest,
                    }),
                )
                .map_err(host_error)?;
                emit_prompt_audit(
                    ctx,
                    &snapshot.view,
                    active,
                    &args_digest,
                    args_bytes,
                    "failed",
                    Some(&envelope.provenance),
                )
                .await?;
                return serde_json::to_value(envelope).map_err(|error| {
                    HostError::HostCall(format!("MCP envelope encoding failed: {error}"))
                });
            }
            Err(error) => return Err(host_error(error)),
        };
        validate_prompt_result(&active.server, &result, &self.runtime.bounds)
            .map_err(host_error)?;
        let envelope = untrusted_envelope(
            &snapshot.view,
            &active.server,
            "prompt",
            &active.name,
            &active.target_digest,
            result,
        )
        .map_err(host_error)?;
        emit_prompt_audit(
            ctx,
            &snapshot.view,
            active,
            &args_digest,
            args_bytes,
            "completed",
            Some(&envelope.provenance),
        )
        .await?;
        serde_json::to_value(envelope)
            .map_err(|error| HostError::HostCall(format!("MCP envelope encoding failed: {error}")))
    }
}

#[derive(Clone)]
pub struct McpResourceHandler {
    runtime: Arc<McpRuntime>,
}

#[async_trait]
impl ResourceHandler for McpResourceHandler {
    fn scheme(&self) -> &str {
        "mcp"
    }

    fn capability(&self) -> &str {
        "resources.read:mcp"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<ResourceContent> {
        if selector.is_some() {
            return Err(HostError::InvalidArgs(
                "MCP resource selectors are unsupported; the bounded remote result is atomic"
                    .to_string(),
            ));
        }
        let snapshot = self.runtime.snapshot();
        let resource = snapshot
            .resources
            .get(uri)
            .ok_or_else(|| HostError::NotFound(format!("MCP resource {uri}")))?;
        require_exact_grant(ctx, &resource.capability)?;
        let request_digest =
            value_digest(&json!({ "uriDigest": resource.target_digest })).map_err(host_error)?;
        emit_resource_audit(
            ctx,
            &snapshot.view,
            resource,
            &request_digest,
            "requested",
            None,
        )
        .await?;
        let result = self
            .runtime
            .rpc(
                ctx,
                &resource.server,
                "resources/read",
                json!({ "uri": resource.source_uri }),
            )
            .await;
        let result = match result {
            Ok(result) => result,
            Err(McpError::Rpc { code, digest, .. }) => {
                let envelope = untrusted_envelope(
                    &snapshot.view,
                    &resource.server,
                    "resource_error",
                    resource.local_uri.rsplit('/').next().unwrap_or("resource"),
                    &resource.target_digest,
                    json!({
                        "kind": "mcp_remote_error",
                        "trust": "untrusted",
                        "code": code,
                        "errorDigest": digest,
                    }),
                )
                .map_err(host_error)?;
                emit_resource_audit(
                    ctx,
                    &snapshot.view,
                    resource,
                    &request_digest,
                    "failed",
                    Some(&envelope.provenance),
                )
                .await?;
                return resource_content(resource, &envelope);
            }
            Err(error) => return Err(host_error(error)),
        };
        validate_resource_result(resource, &result, &self.runtime.bounds).map_err(host_error)?;
        let envelope = untrusted_envelope(
            &snapshot.view,
            &resource.server,
            "resource",
            resource.local_uri.rsplit('/').next().unwrap_or("resource"),
            &resource.target_digest,
            result,
        )
        .map_err(host_error)?;
        emit_resource_audit(
            ctx,
            &snapshot.view,
            resource,
            &request_digest,
            "completed",
            Some(&envelope.provenance),
        )
        .await?;
        resource_content(resource, &envelope)
    }

    async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> tm_host::Result<ResourceContent> {
        let snapshot = self.runtime.snapshot();
        let resource = snapshot
            .resources
            .get(uri)
            .ok_or_else(|| HostError::NotFound(format!("MCP resource {uri}")))?;
        require_exact_grant(ctx, &resource.capability)?;
        Ok(ResourceContent {
            uri: resource.local_uri.clone(),
            kind: "mcp_untrusted_resource".to_string(),
            mime: "application/json".to_string(),
            title: None,
            size_bytes: 0,
            selector: None,
            has_more: false,
            content: String::new(),
            preview: format!(
                "allowlisted MCP resource from {} (target digest {})",
                resource.server, resource.target_digest
            ),
        })
    }

    async fn list(
        &self,
        uri: Option<&str>,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Vec<ResourceEntry>> {
        let snapshot = self.runtime.snapshot();
        let alias_filter = parse_resource_list_filter(uri)?;
        Ok(snapshot
            .resources
            .values()
            .filter(|resource| {
                alias_filter
                    .as_deref()
                    .is_none_or(|alias| resource.server == alias)
                    && ctx
                        .grants
                        .names()
                        .any(|granted| granted == resource.capability)
            })
            .map(|resource| {
                let id = resource.local_uri.rsplit('/').next().unwrap_or("resource");
                ResourceEntry {
                    uri: resource.local_uri.clone(),
                    name: format!("resource-{}", &id[..id.len().min(12)]),
                    kind: "mcp_untrusted_resource".to_string(),
                    title: None,
                    size_bytes: None,
                    modified_at: None,
                }
            })
            .collect())
    }
}

fn resource_content(
    resource: &ImportedResource,
    envelope: &UntrustedMcpData,
) -> tm_host::Result<ResourceContent> {
    let content = serde_json::to_string(envelope)
        .map_err(|error| HostError::HostCall(format!("MCP envelope encoding failed: {error}")))?;
    let size_bytes = content.len();
    Ok(ResourceContent {
        uri: resource.local_uri.clone(),
        kind: "mcp_untrusted_resource".to_string(),
        mime: "application/json".to_string(),
        title: None,
        size_bytes,
        selector: None,
        has_more: false,
        content,
        preview: format!(
            "untrusted MCP resource from {} ({} bytes, digest {})",
            resource.server, size_bytes, envelope.provenance.payload_digest
        ),
    })
}

fn require_binding_snapshot(
    snapshot: &crate::catalog::CatalogState,
    generation: u64,
    catalog_digest: &str,
) -> Result<()> {
    if generation != snapshot.view.generation || catalog_digest != snapshot.view.digest {
        return Err(McpError::StaleBinding {
            binding_generation: generation,
            active_generation: snapshot.view.generation,
        });
    }
    Ok(())
}

enum MutationExecution {
    Execute(McpMutationEffectRecord),
    Replay(McpMutationEffectRecord),
}

async fn begin_mutation_effect(
    runtime: &McpRuntime,
    ctx: &InvocationCtx,
    catalog: &crate::McpCatalogView,
    tool: &ImportedTool,
    request_digest: &str,
    request_bytes: usize,
) -> Result<MutationExecution> {
    let effect_scope_id = ctx.events.effect_scope_id().ok_or_else(|| {
        McpError::Unavailable("MCP mutation requires a host-owned durable effect scope".to_string())
    })?;
    let effect_id = value_digest(&json!({
        "sessionId": ctx.session_id,
        "effectScopeId": effect_scope_id,
        "actorId": ctx.actor_id,
        "server": tool.server,
        "tool": tool.name,
        "targetDigest": tool.target_digest,
        "requestDigest": request_digest,
    }))?;
    let intent = McpMutationIntent {
        effect_id,
        session_id: ctx.session_id.clone(),
        effect_scope_id,
        actor_id: ctx.actor_id.clone(),
        catalog_generation: catalog.generation,
        catalog_digest: catalog.digest.clone(),
        server: tool.server.clone(),
        tool: tool.name.clone(),
        target_digest: tool.target_digest.clone(),
        request_digest: request_digest.to_string(),
        request_bytes,
    };
    let store = runtime.mutation_effects.read().clone();
    let claim = store.begin(intent).await?;
    if claim.created {
        return Ok(MutationExecution::Execute(claim.record));
    }
    let record = if claim.record.status == McpMutationEffectStatus::Started {
        store
            .finish(
                &claim.record.intent.effect_id,
                McpMutationEffectStatus::Uncertain,
                None,
                None,
                Some("interrupted_before_terminal_persistence"),
                None,
            )
            .await?
    } else {
        claim.record
    };
    Ok(MutationExecution::Replay(record))
}

#[allow(clippy::too_many_arguments)]
async fn finish_mutation_effect(
    runtime: &McpRuntime,
    effect_id: &str,
    status: McpMutationEffectStatus,
    result_digest: Option<&str>,
    result_bytes: Option<usize>,
    error_code: Option<&str>,
    error_digest: Option<&str>,
) -> Result<McpMutationEffectRecord> {
    let store = runtime.mutation_effects.read().clone();
    store
        .finish(
            effect_id,
            status,
            result_digest,
            result_bytes,
            error_code,
            error_digest,
        )
        .await
}

async fn mark_mutation_uncertain(
    runtime: &McpRuntime,
    effect: Option<&McpMutationEffectRecord>,
    error: &McpError,
) -> tm_host::Result<()> {
    let Some(effect) = effect else {
        return Ok(());
    };
    let error_digest = sha256_hex(error.to_string().as_bytes());
    finish_mutation_effect(
        runtime,
        &effect.intent.effect_id,
        McpMutationEffectStatus::Uncertain,
        None,
        None,
        Some("remote_outcome_uncertain"),
        Some(&error_digest),
    )
    .await
    .map(|_| ())
    .map_err(host_error)
}

fn mutation_replay_envelope(
    catalog: &crate::McpCatalogView,
    tool: &ImportedTool,
    effect: &McpMutationEffectRecord,
) -> Result<UntrustedMcpData> {
    untrusted_envelope(
        catalog,
        &tool.server,
        "mutation_replay",
        &tool.name,
        &tool.target_digest,
        json!({
            "kind": "mcp_mutation_replay_receipt",
            "trust": "untrusted",
            "effectId": effect.intent.effect_id,
            "status": effect.status,
            "resultDigest": effect.result_digest,
            "resultBytes": effect.result_bytes,
            "errorCode": effect.error_code,
            "errorDigest": effect.error_digest,
            "reexecuted": false,
        }),
    )
}

fn mutation_preview(args: &Value) -> String {
    const MAX_FIELDS: usize = 12;
    const MAX_VALUE_CHARS: usize = 80;
    let mut entries = Vec::new();
    collect_preview_fields(args, "$", None, &mut entries, MAX_FIELDS, MAX_VALUE_CHARS);
    if entries.is_empty() {
        "{}".to_string()
    } else {
        format!("{{{}}}", entries.join(", "))
    }
}

fn collect_preview_fields(
    value: &Value,
    path: &str,
    field: Option<&str>,
    entries: &mut Vec<String>,
    max_fields: usize,
    max_value_chars: usize,
) {
    if entries.len() >= max_fields {
        return;
    }
    match value {
        Value::Object(values) => {
            for (name, value) in values {
                let next = format!("{path}.{}", safe_preview_field(name));
                collect_preview_fields(
                    value,
                    &next,
                    Some(name),
                    entries,
                    max_fields,
                    max_value_chars,
                );
                if entries.len() >= max_fields {
                    break;
                }
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_preview_fields(
                    value,
                    &format!("{path}[{index}]"),
                    field,
                    entries,
                    max_fields,
                    max_value_chars,
                );
                if entries.len() >= max_fields {
                    break;
                }
            }
        }
        _ => {
            let rendered = if field.is_some_and(is_sensitive_field) || is_sensitive_value(value) {
                "<redacted>".to_string()
            } else {
                bounded_preview_scalar(value, max_value_chars)
            };
            entries.push(format!("{path}={rendered}"));
        }
    }
}

fn is_sensitive_field(field: &str) -> bool {
    let normalized = field.to_ascii_lowercase().replace(['-', '_'], "");
    [
        "authorization",
        "cookie",
        "credential",
        "password",
        "passwd",
        "privatekey",
        "apikey",
        "accesstoken",
        "refreshtoken",
        "secret",
        "token",
    ]
    .iter()
    .any(|sensitive| normalized.contains(sensitive))
}

fn is_sensitive_value(value: &Value) -> bool {
    let Some(value) = value.as_str() else {
        return false;
    };
    let lower = value.to_ascii_lowercase();
    lower.starts_with("bearer ")
        || lower.starts_with("basic ")
        || value.starts_with("sk-")
        || value.starts_with("ghp_")
        || value.starts_with("github_pat_")
        || value.starts_with("AKIA")
        || (value.len() >= 48
            && (value.matches('.').count() == 2
                || value.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
                })))
}

fn bounded_preview_scalar(value: &Value, max_chars: usize) -> String {
    let rendered = serde_json::to_string(value).unwrap_or_else(|_| "<invalid>".to_string());
    if rendered.chars().count() <= max_chars {
        return rendered;
    }
    let prefix = rendered.chars().take(max_chars).collect::<String>();
    format!("{prefix}…")
}

fn safe_preview_field(field: &str) -> String {
    field
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

fn require_exact_grant(ctx: &InvocationCtx, capability: &str) -> tm_host::Result<()> {
    if ctx.grants.names().any(|granted| granted == capability) {
        Ok(())
    } else {
        Err(HostError::CapabilityDenied(capability.to_string()))
    }
}

fn host_error(error: McpError) -> HostError {
    match error {
        McpError::Bounds { target, limit } => {
            HostError::QuotaExceeded(format!("{target}: {limit}"))
        }
        McpError::MissingAllowlistedObject { name, .. } => HostError::NotFound(name),
        McpError::StaleBinding { .. } => HostError::CapabilityDenied(error.to_string()),
        McpError::InvalidConfig(message) => HostError::InvalidArgs(message),
        McpError::InvalidRemote { server, message } => HostError::InvalidArgs(format!(
            "MCP peer {server} returned invalid bounded data ({})",
            sha256_hex(message.as_bytes())
        )),
        McpError::ProtocolVersion { server, .. } => HostError::InvalidArgs(format!(
            "MCP peer {server} returned an incompatible protocol version"
        )),
        _ => HostError::HostCall(error.to_string()),
    }
}

fn common_errors(mutation: bool) -> Vec<ToolErrorDoc> {
    let mut errors = vec![
        ToolErrorDoc {
            name: "CapabilityDeniedError".to_string(),
            when: "The exact imported capability is not granted or its catalog binding is stale."
                .to_string(),
            retryable: false,
        },
        ToolErrorDoc {
            name: "QuotaExceededError".to_string(),
            when: "The bounded MCP request or result limit is exceeded.".to_string(),
            retryable: false,
        },
        ToolErrorDoc {
            name: "HostCallError".to_string(),
            when: "The MCP transport or JSON-RPC peer fails.".to_string(),
            retryable: true,
        },
    ];
    if mutation {
        errors.push(ToolErrorDoc {
            name: "ApprovalDeniedError".to_string(),
            when: "The user denies the manual mutation approval.".to_string(),
            retryable: false,
        });
    }
    errors
}

fn untrusted_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["kind", "trust", "provenance", "data"],
        "properties": {
            "kind": { "const": "mcp_untrusted_data" },
            "trust": { "const": "untrusted" },
            "provenance": { "type": "object" },
            "data": {}
        }
    })
}

fn prompt_args_schema(arguments: &[McpPromptArgumentView]) -> Value {
    let properties = arguments
        .iter()
        .map(|argument| (argument.name.clone(), json!({ "type": "string" })))
        .collect::<Map<_, _>>();
    let required = arguments
        .iter()
        .filter(|argument| argument.required)
        .map(|argument| Value::String(argument.name.clone()))
        .collect::<Vec<_>>();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn validate_prompt_args(
    prompt: &ImportedPrompt,
    args: &Value,
    bounds: &crate::McpBounds,
) -> tm_host::Result<()> {
    ensure_value_bound(args, bounds.max_request_bytes, "MCP prompt arguments")
        .map_err(host_error)?;
    let args = args.as_object().ok_or_else(|| {
        HostError::InvalidArgs("MCP prompt arguments must be an object".to_string())
    })?;
    for (name, value) in args {
        if !prompt
            .arguments
            .iter()
            .any(|argument| argument.name == *name)
        {
            return Err(HostError::InvalidArgs(format!(
                "unknown MCP prompt argument {name}"
            )));
        }
        if !value.is_string() {
            return Err(HostError::InvalidArgs(format!(
                "MCP prompt argument {name} must be a string"
            )));
        }
    }
    if let Some(missing) = prompt
        .arguments
        .iter()
        .find(|argument| argument.required && !args.contains_key(&argument.name))
    {
        return Err(HostError::InvalidArgs(format!(
            "missing MCP prompt argument {}",
            missing.name
        )));
    }
    Ok(())
}

fn validate_tool_result(
    tool: &ImportedTool,
    result: &Value,
    bounds: &crate::McpBounds,
) -> Result<()> {
    let server = tool.server.as_str();
    let object = result.as_object().ok_or_else(|| McpError::InvalidRemote {
        server: server.to_string(),
        message: "tools/call result is not an object".to_string(),
    })?;
    validate_content_array(server, object, "content", bounds)?;
    if object
        .get("structuredContent")
        .is_some_and(|value| !value.is_object())
    {
        return Err(McpError::InvalidRemote {
            server: server.to_string(),
            message: "tools/call structuredContent is not an object".to_string(),
        });
    }
    match (&tool.output_schema, object.get("structuredContent")) {
        (Some(schema), Some(structured)) => {
            validate_schema_instance(server, schema, structured, "structuredContent")?;
        }
        (Some(_), None) => {
            return Err(McpError::InvalidRemote {
                server: server.to_string(),
                message: "tools/call omitted structuredContent required by outputSchema"
                    .to_string(),
            });
        }
        (None, _) => {}
    }
    if object
        .get("isError")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(McpError::InvalidRemote {
            server: server.to_string(),
            message: "tools/call isError is not boolean".to_string(),
        });
    }
    Ok(())
}

fn validate_prompt_result(server: &str, result: &Value, bounds: &crate::McpBounds) -> Result<()> {
    let object = result.as_object().ok_or_else(|| McpError::InvalidRemote {
        server: server.to_string(),
        message: "prompts/get result is not an object".to_string(),
    })?;
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| McpError::InvalidRemote {
            server: server.to_string(),
            message: "prompts/get result has no messages array".to_string(),
        })?;
    if messages.len() > bounds.max_content_items {
        return Err(McpError::Bounds {
            target: format!("{server} prompt messages"),
            limit: format!(
                "{} messages exceeds {}",
                messages.len(),
                bounds.max_content_items
            ),
        });
    }
    for message in messages {
        let message = message.as_object().ok_or_else(|| McpError::InvalidRemote {
            server: server.to_string(),
            message: "prompt message is not an object".to_string(),
        })?;
        if !matches!(
            message.get("role").and_then(Value::as_str),
            Some("user" | "assistant")
        ) || !message.get("content").is_some_and(Value::is_object)
        {
            return Err(McpError::InvalidRemote {
                server: server.to_string(),
                message: "prompt message role/content is invalid".to_string(),
            });
        }
    }
    Ok(())
}

fn validate_resource_result(
    resource: &ImportedResource,
    result: &Value,
    bounds: &crate::McpBounds,
) -> Result<()> {
    let object = result.as_object().ok_or_else(|| McpError::InvalidRemote {
        server: resource.server.clone(),
        message: "resources/read result is not an object".to_string(),
    })?;
    let contents = object
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| McpError::InvalidRemote {
            server: resource.server.clone(),
            message: "resources/read result has no contents array".to_string(),
        })?;
    if contents.len() > bounds.max_content_items {
        return Err(McpError::Bounds {
            target: format!("{} resource contents", resource.server),
            limit: format!(
                "{} contents exceeds {}",
                contents.len(),
                bounds.max_content_items
            ),
        });
    }
    for content in contents {
        let content = content.as_object().ok_or_else(|| McpError::InvalidRemote {
            server: resource.server.clone(),
            message: "resource content is not an object".to_string(),
        })?;
        if content.get("uri").and_then(Value::as_str) != Some(resource.source_uri.as_str()) {
            return Err(McpError::InvalidRemote {
                server: resource.server.clone(),
                message: "resource content URI does not match the requested target".to_string(),
            });
        }
        let text = content.get("text").is_some_and(Value::is_string);
        let blob = content.get("blob").is_some_and(Value::is_string);
        if text == blob {
            return Err(McpError::InvalidRemote {
                server: resource.server.clone(),
                message: "resource content must contain exactly one text/blob string".to_string(),
            });
        }
    }
    Ok(())
}

fn validate_content_array(
    server: &str,
    object: &Map<String, Value>,
    field: &str,
    bounds: &crate::McpBounds,
) -> Result<()> {
    let content =
        object
            .get(field)
            .and_then(Value::as_array)
            .ok_or_else(|| McpError::InvalidRemote {
                server: server.to_string(),
                message: format!("result has no {field} array"),
            })?;
    if content.len() > bounds.max_content_items {
        return Err(McpError::Bounds {
            target: format!("{server} result content"),
            limit: format!(
                "{} content items exceeds {}",
                content.len(),
                bounds.max_content_items
            ),
        });
    }
    if content.iter().any(|item| !item.is_object()) {
        return Err(McpError::InvalidRemote {
            server: server.to_string(),
            message: "result content item is not an object".to_string(),
        });
    }
    Ok(())
}

fn untrusted_envelope(
    catalog: &crate::McpCatalogView,
    server: &str,
    object_kind: &str,
    object_name: &str,
    target_digest: &str,
    data: Value,
) -> Result<UntrustedMcpData> {
    let payload_bytes = value_len(&data)?;
    let payload_digest = value_digest(&data)?;
    Ok(UntrustedMcpData::new(
        McpProvenance {
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            catalog_generation: catalog.generation,
            catalog_digest: catalog.digest.clone(),
            server: server.to_string(),
            object_kind: object_kind.to_string(),
            object_name: object_name.to_string(),
            target_digest: target_digest.to_string(),
            payload_digest,
            payload_bytes,
        },
        data,
    ))
}

async fn emit_invocation(
    ctx: &InvocationCtx,
    catalog: &crate::McpCatalogView,
    tool: &ImportedTool,
    request_digest: &str,
    request_bytes: usize,
    status: &str,
    result: Option<&McpProvenance>,
) -> tm_host::Result<()> {
    emit_bounded_audit(
        ctx,
        catalog,
        &tool.server,
        "tool",
        &tool.name,
        &tool.target_digest,
        request_digest,
        request_bytes,
        status,
        result,
    )
    .await
}

async fn emit_prompt_audit(
    ctx: &InvocationCtx,
    catalog: &crate::McpCatalogView,
    prompt: &ImportedPrompt,
    request_digest: &str,
    request_bytes: usize,
    status: &str,
    result: Option<&McpProvenance>,
) -> tm_host::Result<()> {
    emit_bounded_audit(
        ctx,
        catalog,
        &prompt.server,
        "prompt",
        &prompt.name,
        &prompt.target_digest,
        request_digest,
        request_bytes,
        status,
        result,
    )
    .await
}

async fn emit_resource_audit(
    ctx: &InvocationCtx,
    catalog: &crate::McpCatalogView,
    resource: &ImportedResource,
    request_digest: &str,
    status: &str,
    result: Option<&McpProvenance>,
) -> tm_host::Result<()> {
    emit_bounded_audit(
        ctx,
        catalog,
        &resource.server,
        "resource",
        resource.local_uri.rsplit('/').next().unwrap_or("resource"),
        &resource.target_digest,
        request_digest,
        request_digest.len(),
        status,
        result,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn emit_bounded_audit(
    ctx: &InvocationCtx,
    catalog: &crate::McpCatalogView,
    server: &str,
    object_kind: &str,
    object_name: &str,
    target_digest: &str,
    request_digest: &str,
    request_bytes: usize,
    status: &str,
    result: Option<&McpProvenance>,
) -> tm_host::Result<()> {
    ctx.emit_event(
        "mcp_invocation",
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "catalogGeneration": catalog.generation,
            "catalogDigest": catalog.digest,
            "server": server,
            "objectKind": object_kind,
            "objectName": object_name,
            "targetDigest": target_digest,
            "requestDigest": request_digest,
            "requestBytes": request_bytes,
            "status": status,
            "resultDigest": result.map(|value| value.payload_digest.as_str()),
            "resultBytes": result.map(|value| value.payload_bytes),
        }),
    )
    .await
}

fn parse_resource_list_filter(uri: Option<&str>) -> tm_host::Result<Option<String>> {
    let Some(uri) = uri.filter(|uri| !uri.is_empty() && *uri != "mcp://") else {
        return Ok(None);
    };
    let parsed =
        Url::parse(uri).map_err(|_| HostError::InvalidArgs("invalid mcp URI".to_string()))?;
    if parsed.scheme() != "mcp" || parsed.username() != "" || parsed.password().is_some() {
        return Err(HostError::InvalidArgs("invalid mcp URI".to_string()));
    }
    let alias = parsed
        .host_str()
        .ok_or_else(|| HostError::InvalidArgs("mcp URI has no server alias".to_string()))?;
    if parsed.path() != "/" && !parsed.path().is_empty() {
        return Err(HostError::InvalidArgs(
            "resource listing accepts only an mcp://<server>/ root".to_string(),
        ));
    }
    Ok(Some(alias.to_string()))
}

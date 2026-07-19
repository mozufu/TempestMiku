use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use parking_lot::RwLock;
use serde_json::{Map, Value, json};
use tm_host::InvocationCtx;
use tokio::sync::Mutex;

use crate::{
    MCP_PROTOCOL_VERSION, McpBounds, McpCatalogContext, McpCatalogView, McpError,
    McpObjectAllowlist, McpPromptArgumentView, McpPromptView, McpResourceView, McpServerSpec,
    McpServerView, McpToolPolicy, McpToolView, McpTransport, ReloadReport, Result,
    effects::{McpMutationEffectStore, volatile_effect_store},
    validate::{
        local_resource_uri, schema_for_disclosure, sha256_hex, validate_name, validate_remote_name,
        validate_schema, validate_server_alias, validate_tool_namespace, validate_uri,
        value_digest,
    },
};

#[derive(Clone)]
pub struct McpCatalogManager {
    pub(crate) runtime: Arc<McpRuntime>,
}

pub(crate) struct McpRuntime {
    pub(crate) transport: Arc<dyn McpTransport>,
    pub(crate) catalog_context: McpCatalogContext,
    pub(crate) bounds: McpBounds,
    pub(crate) active: RwLock<Arc<CatalogState>>,
    pub(crate) mutation_effects: RwLock<Arc<dyn McpMutationEffectStore>>,
    next_request_id: AtomicU64,
    reload_lock: Mutex<()>,
}

#[derive(Debug)]
pub(crate) struct CatalogState {
    pub(crate) view: McpCatalogView,
    pub(crate) tools: BTreeMap<String, ImportedTool>,
    pub(crate) resources: BTreeMap<String, ImportedResource>,
    pub(crate) prompts: BTreeMap<String, ImportedPrompt>,
}

#[derive(Debug, Clone)]
pub(crate) struct ImportedTool {
    pub(crate) server: String,
    pub(crate) name: String,
    pub(crate) capability: String,
    pub(crate) mutation: bool,
    pub(crate) input_schema: Value,
    pub(crate) disclosed_input_schema: Value,
    pub(crate) output_schema: Option<Value>,
    pub(crate) target_digest: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ImportedResource {
    pub(crate) server: String,
    pub(crate) source_uri: String,
    pub(crate) local_uri: String,
    pub(crate) capability: String,
    pub(crate) target_digest: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ImportedPrompt {
    pub(crate) server: String,
    pub(crate) name: String,
    pub(crate) capability: String,
    pub(crate) arguments: Vec<McpPromptArgumentView>,
    pub(crate) target_digest: String,
}

impl CatalogState {
    fn empty() -> Self {
        Self {
            view: McpCatalogView::empty(),
            tools: BTreeMap::new(),
            resources: BTreeMap::new(),
            prompts: BTreeMap::new(),
        }
    }
}

impl McpCatalogManager {
    pub fn new(
        transport: Arc<dyn McpTransport>,
        bounds: McpBounds,
        catalog_context: McpCatalogContext,
    ) -> Result<Self> {
        bounds.validate()?;
        Ok(Self {
            runtime: Arc::new(McpRuntime {
                transport,
                catalog_context,
                bounds,
                active: RwLock::new(Arc::new(CatalogState::empty())),
                mutation_effects: RwLock::new(volatile_effect_store()),
                next_request_id: AtomicU64::new(1),
                reload_lock: Mutex::new(()),
            }),
        })
    }

    pub fn catalog(&self) -> McpCatalogView {
        self.runtime.active.read().view.clone()
    }

    /// Discover all allowlisted objects into a private staging catalog, then atomically activate it.
    /// Any negotiation, validation, pagination, bounds, or collision failure leaves the active
    /// generation and digest untouched.
    pub async fn reload(&self, specs: &[McpServerSpec]) -> Result<ReloadReport> {
        let _guard = self.runtime.reload_lock.lock().await;
        let previous = self.runtime.active.read().clone();
        let generation = previous
            .view
            .generation
            .checked_add(1)
            .ok_or_else(|| McpError::Unavailable("catalog generation overflow".to_string()))?;
        let staged = self.stage(specs, generation).await?;
        let report = ReloadReport {
            previous_generation: previous.view.generation,
            generation,
            digest: staged.view.digest.clone(),
            servers: staged.view.servers.len(),
            tools: staged.tools.len(),
            resources: staged.resources.len(),
            prompts: staged.prompts.len(),
        };
        *self.runtime.active.write() = Arc::new(staged);
        Ok(report)
    }

    async fn stage(&self, specs: &[McpServerSpec], generation: u64) -> Result<CatalogState> {
        if specs.len() > self.runtime.bounds.max_servers {
            return Err(McpError::Bounds {
                target: "MCP servers".to_string(),
                limit: format!(
                    "{} servers exceeds {}",
                    specs.len(),
                    self.runtime.bounds.max_servers
                ),
            });
        }

        let mut aliases = BTreeSet::new();
        for spec in specs {
            validate_spec(spec, &self.runtime.bounds)?;
            if !aliases.insert(spec.alias.clone()) {
                return Err(McpError::Collision(format!(
                    "duplicate server alias {}",
                    spec.alias
                )));
            }
        }

        let mut ordered = specs.iter().collect::<Vec<_>>();
        ordered.sort_by(|left, right| left.alias.cmp(&right.alias));
        let mut total_items = 0usize;
        let mut servers = Vec::with_capacity(ordered.len());
        let mut tools = BTreeMap::new();
        let mut resources = BTreeMap::new();
        let mut prompts = BTreeMap::new();

        for spec in ordered {
            let staged = self.initialize_and_discover(spec, &mut total_items).await?;
            for tool in staged.tools {
                if tools
                    .insert(tool.capability.clone(), tool.clone())
                    .is_some()
                    || prompts.contains_key(&tool.capability)
                {
                    return Err(McpError::Collision(tool.capability));
                }
            }
            for resource in staged.resources {
                if resources
                    .insert(resource.local_uri.clone(), resource.clone())
                    .is_some()
                {
                    return Err(McpError::Collision(resource.local_uri));
                }
            }
            for prompt in staged.prompts {
                if prompts
                    .insert(prompt.capability.clone(), prompt.clone())
                    .is_some()
                    || tools.contains_key(&prompt.capability)
                {
                    return Err(McpError::Collision(prompt.capability));
                }
            }
            servers.push(staged.view);
        }

        let digest_value = json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "servers": servers,
        });
        let digest = value_digest(&digest_value)?;
        Ok(CatalogState {
            view: McpCatalogView {
                generation,
                digest,
                protocol_version: MCP_PROTOCOL_VERSION.to_string(),
                servers,
            },
            tools,
            resources,
            prompts,
        })
    }

    async fn initialize_and_discover(
        &self,
        spec: &McpServerSpec,
        total_items: &mut usize,
    ) -> Result<StagedServer> {
        let initialize = self
            .runtime
            .rpc(
                self.runtime.catalog_context.invocation(),
                &spec.alias,
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "TempestMiku",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
            .await?;
        let protocol = required_string(&initialize, "protocolVersion", &spec.alias, "initialize")?;
        if protocol != MCP_PROTOCOL_VERSION {
            return Err(McpError::ProtocolVersion {
                server: spec.alias.clone(),
                expected: MCP_PROTOCOL_VERSION.to_string(),
                actual: protocol.to_string(),
            });
        }
        let capabilities = required_object(&initialize, "capabilities", &spec.alias, "initialize")?;
        require_negotiated_capabilities(spec, capabilities)?;
        let server_info = initialize
            .get("serverInfo")
            .and_then(Value::as_object)
            .ok_or_else(|| McpError::InvalidRemote {
                server: spec.alias.clone(),
                message: "initialize result has no serverInfo object".to_string(),
            })?;
        validate_implementation(&spec.alias, server_info, &self.runtime.bounds)?;
        let implementation_digest = value_digest(&Value::Object(server_info.clone()))?;

        self.runtime
            .notify(
                self.runtime.catalog_context.invocation(),
                &spec.alias,
                "notifications/initialized",
                None,
            )
            .await?;

        let mut server_items = 0usize;
        let tool_values = if spec.allow.tools.is_empty() {
            Vec::new()
        } else {
            self.runtime
                .paginated(
                    self.runtime.catalog_context.invocation(),
                    &spec.alias,
                    "tools/list",
                    "tools",
                    &mut server_items,
                    total_items,
                )
                .await?
        };
        let resource_values = if spec.allow.resources.is_empty() {
            Vec::new()
        } else {
            self.runtime
                .paginated(
                    self.runtime.catalog_context.invocation(),
                    &spec.alias,
                    "resources/list",
                    "resources",
                    &mut server_items,
                    total_items,
                )
                .await?
        };
        let prompt_values = if spec.allow.prompts.is_empty() {
            Vec::new()
        } else {
            self.runtime
                .paginated(
                    self.runtime.catalog_context.invocation(),
                    &spec.alias,
                    "prompts/list",
                    "prompts",
                    &mut server_items,
                    total_items,
                )
                .await?
        };

        let tools = select_tools(&spec.alias, &spec.allow, tool_values, &self.runtime.bounds)?;
        let resources = select_resources(
            &spec.alias,
            &spec.allow,
            resource_values,
            &self.runtime.bounds,
        )?;
        let prompts = select_prompts(
            &spec.alias,
            &spec.allow,
            prompt_values,
            &self.runtime.bounds,
        )?;
        let capability_names = ["tools", "resources", "prompts"]
            .into_iter()
            .filter(|name| capabilities.contains_key(*name))
            .map(str::to_string)
            .collect::<BTreeSet<_>>();

        let view = McpServerView {
            alias: spec.alias.clone(),
            implementation_digest,
            capabilities: capability_names,
            tools: tools
                .iter()
                .map(|tool| {
                    Ok(McpToolView {
                        name: tool.name.clone(),
                        capability: tool.capability.clone(),
                        mutation: tool.mutation,
                        input_schema_digest: value_digest(&tool.input_schema)?,
                        output_schema_digest: tool
                            .output_schema
                            .as_ref()
                            .map(value_digest)
                            .transpose()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            resources: resources
                .iter()
                .map(|resource| McpResourceView {
                    local_uri: resource.local_uri.clone(),
                    capability: resource.capability.clone(),
                    source_uri_digest: sha256_hex(resource.source_uri.as_bytes()),
                })
                .collect(),
            prompts: prompts
                .iter()
                .map(|prompt| McpPromptView {
                    name: prompt.name.clone(),
                    capability: prompt.capability.clone(),
                    arguments: prompt.arguments.clone(),
                })
                .collect(),
        };
        Ok(StagedServer {
            view,
            tools,
            resources,
            prompts,
        })
    }
}

impl McpRuntime {
    pub(crate) fn snapshot(&self) -> Arc<CatalogState> {
        self.active.read().clone()
    }

    pub(crate) async fn rpc(
        &self,
        ctx: &InvocationCtx,
        server: &str,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        let id = self
            .next_request_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| {
                McpError::Unavailable("JSON-RPC request id space exhausted".to_string())
            })?;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let request = serde_json::to_vec(&request)
            .map_err(|error| McpError::Unavailable(format!("request encoding failed: {error}")))?;
        if request.len() > self.bounds.max_request_bytes {
            return Err(McpError::Bounds {
                target: "MCP request".to_string(),
                limit: format!(
                    "{} bytes exceeds {}",
                    request.len(),
                    self.bounds.max_request_bytes
                ),
            });
        }
        let response = self.transport.request(ctx, server, &request).await?;
        if response.len() > self.bounds.max_result_bytes {
            return Err(McpError::Bounds {
                target: "MCP response".to_string(),
                limit: format!(
                    "{} bytes exceeds {}",
                    response.len(),
                    self.bounds.max_result_bytes
                ),
            });
        }
        let response: Value =
            serde_json::from_slice(&response).map_err(|error| McpError::InvalidRemote {
                server: server.to_string(),
                message: format!("{method} response is invalid JSON: {error}"),
            })?;
        let object = response
            .as_object()
            .ok_or_else(|| McpError::InvalidRemote {
                server: server.to_string(),
                message: format!("{method} response is not an object"),
            })?;
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || object.get("id").and_then(Value::as_u64) != Some(id)
        {
            return Err(McpError::InvalidRemote {
                server: server.to_string(),
                message: format!("{method} response has invalid jsonrpc or id"),
            });
        }
        match (object.get("result"), object.get("error")) {
            (Some(result), None) if result.is_object() => Ok(result.clone()),
            (None, Some(Value::Object(error))) => {
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(-32603);
                let digest = value_digest(&Value::Object(error.clone()))?;
                Err(McpError::Rpc {
                    server: server.to_string(),
                    method: method.to_string(),
                    code,
                    digest,
                })
            }
            _ => Err(McpError::InvalidRemote {
                server: server.to_string(),
                message: format!("{method} response must contain exactly one result/error object"),
            }),
        }
    }

    async fn notify(
        &self,
        ctx: &InvocationCtx,
        server: &str,
        method: &str,
        params: Option<Value>,
    ) -> Result<()> {
        let mut notification = Map::from_iter([
            ("jsonrpc".to_string(), Value::String("2.0".to_string())),
            ("method".to_string(), Value::String(method.to_string())),
        ]);
        if let Some(params) = params {
            notification.insert("params".to_string(), params);
        }
        let notification = Value::Object(notification);
        let notification = serde_json::to_vec(&notification).map_err(|error| {
            McpError::Unavailable(format!("notification encoding failed: {error}"))
        })?;
        if notification.len() > self.bounds.max_request_bytes {
            return Err(McpError::Bounds {
                target: "MCP notification".to_string(),
                limit: format!(
                    "{} bytes exceeds {}",
                    notification.len(),
                    self.bounds.max_request_bytes
                ),
            });
        }
        self.transport.notify(ctx, server, &notification).await
    }

    async fn paginated(
        &self,
        ctx: &InvocationCtx,
        server: &str,
        method: &str,
        array_key: &str,
        server_items: &mut usize,
        total_items: &mut usize,
    ) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = BTreeSet::new();
        for _ in 0..self.bounds.max_pages_per_list {
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
            let result = self.rpc(ctx, server, method, params).await?;
            let page = result
                .get(array_key)
                .and_then(Value::as_array)
                .ok_or_else(|| McpError::InvalidRemote {
                    server: server.to_string(),
                    message: format!("{method} result has no {array_key} array"),
                })?;
            values.extend(page.iter().cloned());
            *server_items = server_items.saturating_add(page.len());
            *total_items = total_items.saturating_add(page.len());
            if *server_items > self.bounds.max_items_per_server {
                return Err(McpError::Bounds {
                    target: format!("{server} catalog items"),
                    limit: format!(
                        "{} items exceeds {}",
                        *server_items, self.bounds.max_items_per_server
                    ),
                });
            }
            if *total_items > self.bounds.max_total_items {
                return Err(McpError::Bounds {
                    target: "total MCP catalog items".to_string(),
                    limit: format!(
                        "{} items exceeds {}",
                        *total_items, self.bounds.max_total_items
                    ),
                });
            }
            let Some(next) = result.get("nextCursor") else {
                return Ok(values);
            };
            let next = next.as_str().ok_or_else(|| McpError::InvalidRemote {
                server: server.to_string(),
                message: format!("{method} nextCursor is not a string"),
            })?;
            if next.is_empty() || next.len() > self.bounds.max_cursor_bytes {
                return Err(McpError::Bounds {
                    target: format!("{server} {method} cursor"),
                    limit: format!(
                        "cursor must contain 1..={} bytes",
                        self.bounds.max_cursor_bytes
                    ),
                });
            }
            if !seen.insert(next.to_string()) {
                return Err(McpError::InvalidRemote {
                    server: server.to_string(),
                    message: format!("{method} repeated pagination cursor"),
                });
            }
            cursor = Some(next.to_string());
        }
        Err(McpError::Bounds {
            target: format!("{server} {method} pagination"),
            limit: format!("more than {} pages", self.bounds.max_pages_per_list),
        })
    }
}

struct StagedServer {
    view: McpServerView,
    tools: Vec<ImportedTool>,
    resources: Vec<ImportedResource>,
    prompts: Vec<ImportedPrompt>,
}

fn validate_spec(spec: &McpServerSpec, bounds: &McpBounds) -> Result<()> {
    validate_server_alias(&spec.alias)?;
    for name in spec.allow.tools.keys() {
        validate_name(name, bounds, "allowlisted tool")?;
        validate_tool_namespace(name)?;
    }
    for uri in &spec.allow.resources {
        validate_uri(uri, bounds, "allowlisted resource")?;
    }
    for name in &spec.allow.prompts {
        validate_name(name, bounds, "allowlisted prompt")?;
    }
    Ok(())
}

fn require_negotiated_capabilities(
    spec: &McpServerSpec,
    capabilities: &Map<String, Value>,
) -> Result<()> {
    for (required, selected) in [
        ("tools", !spec.allow.tools.is_empty()),
        ("resources", !spec.allow.resources.is_empty()),
        ("prompts", !spec.allow.prompts.is_empty()),
    ] {
        if selected && !capabilities.get(required).is_some_and(Value::is_object) {
            return Err(McpError::MissingCapability {
                server: spec.alias.clone(),
                capability: required.to_string(),
            });
        }
    }
    Ok(())
}

fn validate_implementation(
    server: &str,
    info: &Map<String, Value>,
    bounds: &McpBounds,
) -> Result<()> {
    for field in ["name", "version"] {
        let value =
            info.get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::InvalidRemote {
                    server: server.to_string(),
                    message: format!("serverInfo.{field} must be a string"),
                })?;
        if value.is_empty() || value.len() > bounds.max_name_bytes {
            return Err(McpError::Bounds {
                target: format!("{server} serverInfo.{field}"),
                limit: format!("must contain 1..={} bytes", bounds.max_name_bytes),
            });
        }
    }
    Ok(())
}

fn select_tools(
    server: &str,
    allow: &McpObjectAllowlist,
    values: Vec<Value>,
    bounds: &McpBounds,
) -> Result<Vec<ImportedTool>> {
    let mut advertised = BTreeSet::new();
    let mut selected = Vec::new();
    for value in values {
        let object = value.as_object().ok_or_else(|| McpError::InvalidRemote {
            server: server.to_string(),
            message: "tools/list item is not an object".to_string(),
        })?;
        let name = required_string_object(object, "name", server, "tool")?;
        validate_remote_name(server, name, bounds, "tool")?;
        if !advertised.insert(name.to_string()) {
            return Err(McpError::Collision(format!(
                "duplicate MCP tool {server}/{name}"
            )));
        }
        let Some(policy) = allow.tools.get(name) else {
            continue;
        };
        validate_tool_namespace(name)?;
        let input = object
            .get("inputSchema")
            .ok_or_else(|| McpError::InvalidRemote {
                server: server.to_string(),
                message: format!("tool {name} has no inputSchema"),
            })?;
        let input_schema = validate_schema(server, input, bounds, "inputSchema")?;
        let output_schema = object
            .get("outputSchema")
            .map(|schema| validate_schema(server, schema, bounds, "outputSchema"))
            .transpose()?;
        if object
            .get("execution")
            .and_then(Value::as_object)
            .and_then(|execution| execution.get("taskSupport"))
            .and_then(Value::as_str)
            == Some("required")
        {
            return Err(McpError::InvalidRemote {
                server: server.to_string(),
                message: format!(
                    "tool {name} requires experimental tasks, which this importer does not negotiate"
                ),
            });
        }
        selected.push(imported_tool(
            server,
            name,
            policy,
            input_schema,
            output_schema,
        )?);
    }
    require_all_selected(server, "tool", allow.tools.keys(), &advertised)?;
    selected.sort_by(|left, right| left.capability.cmp(&right.capability));
    Ok(selected)
}

fn imported_tool(
    server: &str,
    name: &str,
    policy: &McpToolPolicy,
    input_schema: Value,
    output_schema: Option<Value>,
) -> Result<ImportedTool> {
    let disclosed_input_schema = schema_for_disclosure(&input_schema);
    let target_digest = value_digest(&json!({
        "server": server,
        "kind": "tool",
        "name": name,
        "mutation": policy.mutation,
        "inputSchema": input_schema,
        "outputSchema": output_schema,
    }))?;
    Ok(ImportedTool {
        server: server.to_string(),
        name: name.to_string(),
        capability: format!("mcp.{server}.{name}"),
        mutation: policy.mutation,
        input_schema,
        disclosed_input_schema,
        output_schema,
        target_digest,
    })
}

fn select_resources(
    server: &str,
    allow: &McpObjectAllowlist,
    values: Vec<Value>,
    bounds: &McpBounds,
) -> Result<Vec<ImportedResource>> {
    let mut advertised = BTreeSet::new();
    let mut selected = Vec::new();
    for value in values {
        let object = value.as_object().ok_or_else(|| McpError::InvalidRemote {
            server: server.to_string(),
            message: "resources/list item is not an object".to_string(),
        })?;
        let uri = required_string_object(object, "uri", server, "resource")?;
        validate_uri(uri, bounds, "remote resource").map_err(|error| McpError::InvalidRemote {
            server: server.to_string(),
            message: error.to_string(),
        })?;
        let name = required_string_object(object, "name", server, "resource")?;
        validate_remote_name(server, name, bounds, "resource")?;
        if !advertised.insert(uri.to_string()) {
            return Err(McpError::Collision(format!(
                "duplicate MCP resource URI from {server}"
            )));
        }
        if !allow.resources.contains(uri) {
            continue;
        }
        let local_uri = local_resource_uri(server, uri);
        let id = local_uri
            .rsplit('/')
            .next()
            .expect("local resource id")
            .to_string();
        selected.push(ImportedResource {
            server: server.to_string(),
            source_uri: uri.to_string(),
            local_uri,
            capability: format!("resources.read:mcp.{server}.{id}"),
            target_digest: target_digest(server, "resource", uri),
        });
    }
    require_all_selected(server, "resource", allow.resources.iter(), &advertised)?;
    selected.sort_by(|left, right| left.local_uri.cmp(&right.local_uri));
    Ok(selected)
}

fn select_prompts(
    server: &str,
    allow: &McpObjectAllowlist,
    values: Vec<Value>,
    bounds: &McpBounds,
) -> Result<Vec<ImportedPrompt>> {
    let mut advertised = BTreeSet::new();
    let mut selected = Vec::new();
    for value in values {
        let object = value.as_object().ok_or_else(|| McpError::InvalidRemote {
            server: server.to_string(),
            message: "prompts/list item is not an object".to_string(),
        })?;
        let name = required_string_object(object, "name", server, "prompt")?;
        validate_remote_name(server, name, bounds, "prompt")?;
        if !advertised.insert(name.to_string()) {
            return Err(McpError::Collision(format!(
                "duplicate MCP prompt {server}/{name}"
            )));
        }
        if !allow.prompts.contains(name) {
            continue;
        }
        let raw_arguments = match object.get("arguments") {
            None => Vec::new(),
            Some(Value::Array(arguments)) => arguments.clone(),
            Some(_) => {
                return Err(McpError::InvalidRemote {
                    server: server.to_string(),
                    message: format!("prompt {name} arguments is not an array"),
                });
            }
        };
        if raw_arguments.len() > bounds.max_content_items {
            return Err(McpError::Bounds {
                target: format!("{server} prompt {name} arguments"),
                limit: format!(
                    "{} arguments exceeds {}",
                    raw_arguments.len(),
                    bounds.max_content_items
                ),
            });
        }
        let mut argument_names = BTreeSet::new();
        let mut arguments = Vec::with_capacity(raw_arguments.len());
        for argument in raw_arguments {
            let argument = argument
                .as_object()
                .ok_or_else(|| McpError::InvalidRemote {
                    server: server.to_string(),
                    message: format!("prompt {name} argument is not an object"),
                })?;
            let argument_name =
                required_string_object(argument, "name", server, "prompt argument")?;
            validate_remote_name(server, argument_name, bounds, "prompt argument")?;
            if !argument_names.insert(argument_name.to_string()) {
                return Err(McpError::Collision(format!(
                    "duplicate prompt argument {server}/{name}/{argument_name}"
                )));
            }
            arguments.push(McpPromptArgumentView {
                name: argument_name.to_string(),
                required: match argument.get("required") {
                    None => false,
                    Some(Value::Bool(required)) => *required,
                    Some(_) => {
                        return Err(McpError::InvalidRemote {
                            server: server.to_string(),
                            message: format!(
                                "prompt {name} argument {argument_name} required is not boolean"
                            ),
                        });
                    }
                },
            });
        }
        let target_digest = value_digest(&json!({
            "server": server,
            "kind": "prompt",
            "name": name,
            "arguments": arguments,
        }))?;
        selected.push(ImportedPrompt {
            server: server.to_string(),
            name: name.to_string(),
            capability: format!("mcp.{server}.prompts.{name}"),
            arguments,
            target_digest,
        });
    }
    require_all_selected(server, "prompt", allow.prompts.iter(), &advertised)?;
    selected.sort_by(|left, right| left.capability.cmp(&right.capability));
    Ok(selected)
}

fn require_all_selected<'a>(
    server: &str,
    kind: &str,
    selected: impl Iterator<Item = &'a String>,
    advertised: &BTreeSet<String>,
) -> Result<()> {
    if let Some(name) = selected
        .into_iter()
        .find(|name| !advertised.contains(*name))
    {
        return Err(McpError::MissingAllowlistedObject {
            server: server.to_string(),
            kind: kind.to_string(),
            name: name.clone(),
        });
    }
    Ok(())
}

fn required_object<'a>(
    value: &'a Value,
    field: &str,
    server: &str,
    context: &str,
) -> Result<&'a Map<String, Value>> {
    value
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| McpError::InvalidRemote {
            server: server.to_string(),
            message: format!("{context}.{field} must be an object"),
        })
}

fn required_string<'a>(
    value: &'a Value,
    field: &str,
    server: &str,
    context: &str,
) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::InvalidRemote {
            server: server.to_string(),
            message: format!("{context}.{field} must be a string"),
        })
}

fn required_string_object<'a>(
    value: &'a Map<String, Value>,
    field: &str,
    server: &str,
    context: &str,
) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::InvalidRemote {
            server: server.to_string(),
            message: format!("{context}.{field} must be a string"),
        })
}

fn target_digest(server: &str, kind: &str, name: &str) -> String {
    sha256_hex(format!("{server}\0{kind}\0{name}").as_bytes())
}

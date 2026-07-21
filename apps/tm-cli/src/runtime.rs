use std::{sync::Arc, time::Duration};

use anyhow::Result;
use tm_artifacts::default_root;
use tm_core::{AgentConfig, CellBudget, Protocol, Sandbox};
use tm_egress::{EgressRuntime, register_egress_functions};
use tm_host::{CapabilityGrants, HostEventSink, InvocationCtx, LinkedFolders, P0HostConfig};
use tm_lang::{TmSandbox, TmSandboxOptions};
use tm_mcp::{
    EgressMcpTransport, McpBounds, McpCatalogContext, McpCatalogManager, McpHttpTransportBounds,
    McpRuntimeConfig,
};
use tm_modes::{ModeId, ModesConfig};

use super::{approval::approval_policy, cli::Args};

pub(super) fn build_agent_config(
    args: &Args,
    protocol: Protocol,
    host_config: &P0HostConfig,
    linked_folders: &LinkedFolders,
) -> AgentConfig {
    let model = args
        .model
        .clone()
        .or_else(|| std::env::var("OPENAI_MODEL").ok())
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "gpt-4o-mini".to_string());
    AgentConfig {
        model,
        max_turns: args.max_turns.unwrap_or(8),
        protocol,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        system_prompt: serious_engineer_prompt(host_config, linked_folders),
        ..AgentConfig::default()
    }
}

pub(super) fn serious_engineer_prompt(
    _host_config: &P0HostConfig,
    linked_folders: &LinkedFolders,
) -> String {
    let mut capability_notes = String::from(
        "Active mode: Serious Engineer. It is already selected and locked for this CLI run. \
Do not call modes.suggest or ask the user to switch modes; the listed fs.*, code.*, and proc.* \
grants are active now.\n",
    );
    let policies = linked_folders.policies();
    if policies.is_empty() {
        capability_notes
            .push_str("No linked folders configured; fs.*, code.*, and proc.* will fail closed.");
    } else {
        for policy in &policies {
            let mode = match policy.mode {
                tm_host::FsMode::Ro => "ro",
                tm_host::FsMode::Rw => "rw",
            };
            capability_notes.push_str(&format!(
                "Linked folders: {} ({mode}) at linked://{}/\n",
                policy.alias, policy.alias
            ));
        }
        let alias = &policies[0].alias;
        capability_notes.push_str(&format!(
            "\
Known linked-repo schemas; call these directly without tools.search/help:
- Search: @fs.grep {{pattern: \"needle\", paths: [\"{alias}:src\"], regex: false, contextLines: 2, limit: 20}}
- Read a bounded slice: @fs.read {{path: \"{alias}:src/file.ts\", selector: \"120-220\"}}
- Patch from a fresh search tag: @fs.patch {{path: hit.path, tag: hit.tag, hunks: [{{op: \"replace\", startLine: hit.line, endLine: hit.line, expectedLines: [hit.text], lines: [\"replacement\"]}}]}}
- Delete lines with an explicit range: @fs.patch {{path: hit.path, tag: hit.tag, hunks: [{{op: \"delete\", startLine: hit.line, endLine: hit.line, expectedLines: [hit.text]}}]}}
- Insert relative to a line: @fs.patch {{path: hit.path, tag: hit.tag, hunks: [{{op: \"insertAfter\", line: hit.line, expectedLine: hit.text, lines: [\"new line\"]}}]}}
- Create a new text file: @fs.write {{path: \"{alias}:test/new_test.ts\", data: \"test content\\n\", createParents: true}}
- Remove a file only with approval: @fs.remove {{path: hit.path, tag: hit.tag}}
- Run argv only: @proc.run {{cmd: \"git\", args: [\"status\", \"--short\"], cwd: \"{alias}:\"}}
Never pass a bare alias such as \"{alias}\" where a linked path is required. Deduplicate search-hit
paths before reading; never map full-file fs.read over many hits. Large files must use selector
ranges around relevant lines. Prefer bounded fs.read or `git grep` through proc.run; `sed` and
`grep` are not granted commands in the default CLI profile. `fs.remove` deletes an entire file and
requires approval; it is separate from patch operations. Replace/delete hunks must repeat the exact
current range in expectedLines, and relative inserts must repeat their anchor in expectedLine. If a
tag is stale or expected context mismatches, read/search again and retry with fresh evidence. After
changing tests, run the exact test file and confirm nonzero collection;
typechecking alone is not behavioral proof. Before finishing, run task-named gates plus git diff
and git status.\n"
        ));
    }
    ModesConfig::default()
        .build_system_prompt(
            &ModeId::from("serious_engineer"),
            tm_core::DEFAULT_SYSTEM_PROMPT,
            &capability_notes,
            // No live user message at CLI startup; always-on layered skills (e.g.
            // scope-guard) still compose, only keyword-triggered ones are skipped.
            "",
        )
        .system_prompt
}

pub(super) async fn build_sandbox(
    args: &Args,
    host_config: &P0HostConfig,
    mcp_config: &McpRuntimeConfig,
    linked_folders: LinkedFolders,
) -> Result<Arc<dyn Sandbox>> {
    // The standalone CLI is its own local authority boundary. A single configured
    // linked folder is unambiguous; multiple folders remain fail-closed until the
    // CLI grows an explicit project selector.
    let policies = linked_folders.policies();
    let session_scope = match policies.as_slice() {
        [policy] => Some(format!("project:{}", policy.alias)),
        [_, _, ..] => Some("cli:unscoped".to_string()),
        [] => None,
    };
    let linked_folders = (!linked_folders.is_empty()).then_some(linked_folders);
    let egress_runtime = EgressRuntime::new(host_config.egress.clone())?;
    let mut options = TmSandboxOptions {
        artifact_root: host_config
            .artifact_root
            .clone()
            .unwrap_or_else(default_root),
        session_id: args.session_id.clone().unwrap_or_else(|| "cli".to_string()),
        session_scope,
        linked_folders,
        grants: serious_engineer_grants().allow_many(host_config.egress.turn_capabilities()),
        approval_policy: approval_policy(host_config)?,
        approval_timeout: Duration::from_millis(host_config.approvals.timeout_ms),
        proc_run_timeout: Duration::from_millis(host_config.proc_run_timeout_ms),
        proc_isolation: host_config.proc_isolation.clone(),
        ..TmSandboxOptions::default()
    };
    register_egress_functions(&mut options.host_registry, egress_runtime.clone());
    if mcp_config.enabled {
        let bounds = McpBounds::default();
        let http_servers = mcp_config.http_servers();
        let transport = Arc::new(EgressMcpTransport::new(
            egress_runtime,
            http_servers.clone(),
            McpHttpTransportBounds::default(),
        )?);
        let mut catalog_grants = CapabilityGrants::default();
        for server in &http_servers {
            catalog_grants =
                catalog_grants.allow(format!("egress.destination:{}", server.destination_id));
            if let Some(secret_id) = &server.secret_id {
                catalog_grants = catalog_grants.allow(format!("secrets.use:{secret_id}"));
            }
        }
        let catalog_context = McpCatalogContext::new(
            InvocationCtx::new(catalog_grants)
                .with_session_id("mcp-catalog-host")
                .with_event_sink(Arc::new(CliCatalogAuditSink)),
        )?;
        let manager = McpCatalogManager::new(transport, bounds, catalog_context)?;
        let report = manager.reload(&mcp_config.specs()).await?;
        let catalog = manager.catalog();
        for server in &catalog.servers {
            let transport = http_servers
                .iter()
                .find(|transport| transport.alias == server.alias)
                .expect("validated MCP config has transport for every catalog server");
            options.grants = options
                .grants
                .clone()
                .allow(format!("egress.destination:{}", transport.destination_id));
            if let Some(secret_id) = &transport.secret_id {
                options.grants = options
                    .grants
                    .clone()
                    .allow(format!("secrets.use:{secret_id}"));
            }
            options.grants = options.grants.clone().allow_many(
                server
                    .tools
                    .iter()
                    .map(|tool| tool.capability.clone())
                    .chain(
                        server
                            .prompts
                            .iter()
                            .map(|prompt| prompt.capability.clone()),
                    )
                    .chain(
                        server
                            .resources
                            .iter()
                            .map(|resource| resource.capability.clone()),
                    ),
            );
            if !server.resources.is_empty() {
                options.grants = options.grants.clone().allow("resources.read:mcp");
            }
        }
        manager
            .bindings()?
            .register_into(&mut options.host_registry, &mut options.resource_registry)?;
        tracing::info!(
            generation = report.generation,
            digest = %report.digest,
            servers = report.servers,
            tools = report.tools,
            resources = report.resources,
            prompts = report.prompts,
            "activated immutable CLI MCP startup catalog"
        );
    }
    Ok(Arc::new(TmSandbox::new(options)))
}

#[derive(Debug)]
struct CliCatalogAuditSink;

#[async_trait::async_trait]
impl HostEventSink for CliCatalogAuditSink {
    async fn emit(&self, event_type: &str, payload_json: serde_json::Value) -> tm_host::Result<()> {
        tracing::info!(event_type, payload = %payload_json, "MCP catalog host audit");
        Ok(())
    }
}

pub(super) fn serious_engineer_grants() -> CapabilityGrants {
    let profile = ModesConfig::default()
        .load_assets()
        .profile_or_unknown(&ModeId::from("serious_engineer"));
    CapabilityGrants::default().allow_many(profile.capabilities)
}

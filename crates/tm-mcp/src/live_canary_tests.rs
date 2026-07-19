use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use serial_test::serial;
use tm_egress::EgressRuntime;
use tm_host::{
    CapabilityGrants, EgressConfig, EgressDestinationConfig, EgressSessionLimits, HostEventSink,
    HostRegistry, InvocationCtx, ResourceRegistry,
};

use crate::{
    EgressMcpTransport, MCP_PROTOCOL_VERSION, McpBounds, McpCatalogContext, McpCatalogManager,
    McpHttpServerConfig, McpHttpTransportBounds, McpObjectAllowlist, McpServerSpec, McpToolPolicy,
};

const LIVE_FLAG: &str = "TM_MCP_LIVE_TESTS";
const SERVER_ALIAS: &str = "cloudflare_docs";
const DESTINATION_ID: &str = "cloudflare-docs-mcp";
const ENDPOINT: &str = "https://docs.mcp.cloudflare.com/mcp";
const SEARCH_TOOL: &str = "search_cloudflare_documentation";
const SEARCH_CAPABILITY: &str = "mcp.cloudflare_docs.search_cloudflare_documentation";
const RESPONSE_CAP_BYTES: usize = 256 * 1024;

#[derive(Default)]
struct CaptureEvents(Mutex<Vec<(String, Value)>>);

#[async_trait]
impl HostEventSink for CaptureEvents {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        self.0
            .lock()
            .expect("live canary event lock")
            .push((event_type.to_string(), payload_json));
        Ok(())
    }
}

impl CaptureEvents {
    fn snapshot(&self) -> Vec<(String, Value)> {
        self.0.lock().expect("live canary event lock").clone()
    }
}

fn live_enabled() -> bool {
    std::env::var(LIVE_FLAG).as_deref() == Ok("1")
}

fn grants(include_tool: bool) -> CapabilityGrants {
    let grants = CapabilityGrants::default().allow(format!("egress.destination:{DESTINATION_ID}"));
    if include_tool {
        grants.allow(SEARCH_CAPABILITY)
    } else {
        grants
    }
}

fn egress_runtime() -> EgressRuntime {
    EgressRuntime::new(EgressConfig {
        enabled: true,
        session_limits: EgressSessionLimits {
            max_requests: 8,
            max_request_bytes: 128 * 1024,
            max_response_bytes: 1024 * 1024,
            max_time_ms: 60_000,
        },
        destinations: vec![EgressDestinationConfig {
            id: DESTINATION_ID.to_string(),
            version: 1,
            scheme: "https".to_string(),
            host: "docs.mcp.cloudflare.com".to_string(),
            port: 443,
            path_prefixes: vec!["/mcp".to_string()],
            methods: BTreeSet::from(["GET".to_string(), "POST".to_string()]),
            redirect_to: BTreeSet::new(),
            max_redirects: 0,
            allow_private_ips: false,
            request_timeout_ms: 30_000,
            connect_timeout_ms: 10_000,
            max_request_bytes: 32 * 1024,
            max_response_bytes: RESPONSE_CAP_BYTES,
            max_requests_per_session: 8,
            max_request_bytes_per_session: 128 * 1024,
            max_response_bytes_per_session: 1024 * 1024,
            max_time_ms_per_session: 60_000,
            allowed_request_headers: BTreeSet::from([
                "accept".to_string(),
                "content-type".to_string(),
                "last-event-id".to_string(),
                "mcp-protocol-version".to_string(),
                "mcp-session-id".to_string(),
            ]),
        }],
        secrets: Vec::new(),
    })
    .expect("production P9 egress policy for the Cloudflare docs MCP endpoint")
}

fn server_spec() -> McpServerSpec {
    McpServerSpec {
        alias: SERVER_ALIAS.to_string(),
        allow: McpObjectAllowlist {
            tools: BTreeMap::from([(SEARCH_TOOL.to_string(), McpToolPolicy::read_only())]),
            resources: BTreeSet::new(),
            prompts: BTreeSet::new(),
        },
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn contains_remote_payload_key(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "body" | "content" | "data" | "structuredContent" | "text" | "url"
            ) || contains_remote_payload_key(value)
        }),
        Value::Array(values) => values.iter().any(contains_remote_payload_key),
        _ => false,
    }
}

fn assert_bounded_metadata_only_audits(events: &[(String, Value)]) {
    assert!(!events.is_empty(), "live canary must capture audit events");
    for (_, payload) in events {
        assert!(
            serde_json::to_vec(payload)
                .expect("audit payload serialization")
                .len()
                <= 4 * 1024,
            "audit payload must remain bounded metadata"
        );
        assert!(
            !contains_remote_payload_key(payload),
            "audit payload must not contain remote MCP content"
        );
    }
}

/// Opt-in public-Internet proof. The Cloudflare docs server is an official, unauthenticated,
/// read-only endpoint. All discovery and the one search invocation still cross production P9
/// DNS/IP validation, exact destination policy, byte/request/time budgets, and egress auditing.
#[tokio::test]
#[serial]
async fn opt_in_cloudflare_docs_read_only_canary_uses_p9_and_untrusted_envelope() {
    if !live_enabled() {
        return;
    }

    let transport = Arc::new(
        EgressMcpTransport::new(
            egress_runtime(),
            vec![McpHttpServerConfig {
                alias: SERVER_ALIAS.to_string(),
                url: ENDPOINT.to_string(),
                destination_id: DESTINATION_ID.to_string(),
                secret_id: None,
                timeout_ms: Some(30_000),
            }],
            McpHttpTransportBounds {
                max_response_bytes: RESPONSE_CAP_BYTES,
                ..McpHttpTransportBounds::default()
            },
        )
        .expect("bounded production MCP transport"),
    );
    let catalog_events = Arc::new(CaptureEvents::default());
    let catalog_context = McpCatalogContext::new(
        InvocationCtx::new(grants(false))
            .with_session_id("mcp-cloudflare-docs-live-catalog")
            .with_event_sink(catalog_events.clone()),
    )
    .expect("host-owned live catalog context");
    let manager = McpCatalogManager::new(
        transport,
        McpBounds {
            max_result_bytes: RESPONSE_CAP_BYTES,
            ..McpBounds::default()
        },
        catalog_context,
    )
    .expect("live catalog manager");

    let report = manager
        .reload(&[server_spec()])
        .await
        .expect("Cloudflare docs MCP 2025-11-25 catalog discovery through P9");
    assert_eq!(report.previous_generation, 0);
    assert_eq!(report.generation, 1);
    assert_eq!(report.servers, 1);
    assert_eq!(report.tools, 1);
    assert_eq!(report.resources, 0);
    assert_eq!(report.prompts, 0);
    assert!(is_sha256(&report.digest));

    let catalog = manager.catalog();
    assert_eq!(catalog.protocol_version, MCP_PROTOCOL_VERSION);
    assert_eq!(catalog.servers.len(), 1);
    assert_eq!(catalog.servers[0].alias, SERVER_ALIAS);
    assert_eq!(catalog.servers[0].tools.len(), 1);
    assert_eq!(catalog.servers[0].tools[0].name, SEARCH_TOOL);
    assert_eq!(catalog.servers[0].tools[0].capability, SEARCH_CAPABILITY);
    assert!(!catalog.servers[0].tools[0].mutation);

    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    manager
        .bindings()
        .expect("live catalog bindings")
        .register_into(&mut host, &mut resources)
        .expect("register exact live search binding");

    let invocation_events = Arc::new(CaptureEvents::default());
    let invocation = InvocationCtx::new(grants(true))
        .with_session_id("mcp-cloudflare-docs-live-call")
        .with_event_sink(invocation_events.clone());
    let result = host
        .invoke(
            SEARCH_CAPABILITY,
            json!({ "query": "MCP Streamable HTTP transport" }),
            &invocation,
        )
        .await
        .expect("one allowlisted Cloudflare documentation search through P9");

    assert_eq!(
        result.get("kind").and_then(Value::as_str),
        Some("mcp_untrusted_data")
    );
    assert_eq!(
        result.get("trust").and_then(Value::as_str),
        Some("untrusted")
    );
    assert_eq!(
        result
            .pointer("/provenance/protocolVersion")
            .and_then(Value::as_str),
        Some(MCP_PROTOCOL_VERSION)
    );
    assert_eq!(
        result.pointer("/provenance/server").and_then(Value::as_str),
        Some(SERVER_ALIAS)
    );
    assert_eq!(
        result
            .pointer("/provenance/objectKind")
            .and_then(Value::as_str),
        Some("tool")
    );
    assert_eq!(
        result
            .pointer("/provenance/objectName")
            .and_then(Value::as_str),
        Some(SEARCH_TOOL)
    );
    assert_eq!(
        result
            .pointer("/provenance/catalogGeneration")
            .and_then(Value::as_u64),
        Some(report.generation)
    );
    assert_eq!(
        result
            .pointer("/provenance/catalogDigest")
            .and_then(Value::as_str),
        Some(report.digest.as_str())
    );
    let payload_bytes = result
        .pointer("/provenance/payloadBytes")
        .and_then(Value::as_u64)
        .expect("bounded payload byte count") as usize;
    assert!(payload_bytes > 0 && payload_bytes <= RESPONSE_CAP_BYTES);
    assert!(
        result
            .pointer("/provenance/payloadDigest")
            .and_then(Value::as_str)
            .is_some_and(is_sha256)
    );
    assert!(
        result
            .pointer("/provenance/targetDigest")
            .and_then(Value::as_str)
            .is_some_and(is_sha256)
    );
    let content_count = result
        .pointer("/data/content")
        .and_then(Value::as_array)
        .map(Vec::len)
        .expect("bounded MCP content array");
    assert!((1..=McpBounds::default().max_content_items).contains(&content_count));

    let catalog_audits = catalog_events.snapshot();
    let invocation_audits = invocation_events.snapshot();
    assert_bounded_metadata_only_audits(&catalog_audits);
    assert_bounded_metadata_only_audits(&invocation_audits);
    assert!(
        catalog_audits
            .iter()
            .any(|(kind, _)| kind == "egress_started")
    );
    assert!(
        catalog_audits
            .iter()
            .any(|(kind, _)| kind == "egress_completed")
    );
    assert!(
        invocation_audits
            .iter()
            .any(|(kind, _)| kind == "egress_started")
    );
    assert!(
        invocation_audits
            .iter()
            .any(|(kind, _)| kind == "egress_completed")
    );
    assert!(invocation_audits.iter().any(|(kind, payload)| {
        kind == "mcp_invocation"
            && payload.get("status").and_then(Value::as_str) == Some("requested")
    }));
    let completed = invocation_audits
        .iter()
        .find_map(|(kind, payload)| {
            (kind == "mcp_invocation"
                && payload.get("status").and_then(Value::as_str) == Some("completed"))
            .then_some(payload)
        })
        .expect("completed bounded MCP audit");
    assert_eq!(
        completed.get("resultDigest").and_then(Value::as_str),
        result
            .pointer("/provenance/payloadDigest")
            .and_then(Value::as_str)
    );
    assert_eq!(
        completed.get("resultBytes").and_then(Value::as_u64),
        Some(payload_bytes as u64)
    );
}

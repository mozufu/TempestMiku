use serde_json::json;
use tm_core::{CellBudget, SessionConfig};
use tm_host::P0HostConfig;

use super::{cli::Args, runtime::build_sandbox};

#[tokio::test]
async fn cli_uses_production_egress_catalog_and_exact_configured_grants() {
    let host_config: P0HostConfig = serde_json::from_value(json!({
        "egress": {
            "enabled": true,
            "destinations": [{
                "id": "research",
                "scheme": "https",
                "host": "api.example.com",
                "port": 443,
                "path_prefixes": ["/v1"],
                "methods": ["GET", "POST"]
            }],
            "secrets": [{
                "id": "research_api",
                "env": "RESEARCH_API_TOKEN",
                "destinations": ["research"],
                "injection": {"kind": "authorization_bearer"}
            }]
        }
    }))
    .unwrap();
    host_config.validate().unwrap();
    assert_eq!(
        host_config.egress.turn_capabilities(),
        [
            "egress.destination:research",
            "http.request",
            "secrets.use",
            "secrets.use:research_api",
        ]
    );

    let sandbox = build_sandbox(
        &Args {
            session_id: Some("cli-egress".to_string()),
            ..Args::default()
        },
        &host_config,
        &tm_mcp::McpRuntimeConfig::default(),
        host_config.linked_folders().unwrap(),
    )
    .await
    .unwrap();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    for (tool, summary) in [
        ("http.get", "exact configured destination"),
        ("http.request", "destination-scoped egress boundary"),
        ("secrets.use", "opaque handle"),
    ] {
        let docs = session
            .eval(&format!("@tools.docs \"{tool}\""), CellBudget::default())
            .await
            .unwrap();
        assert!(docs.error.is_none(), "{tool}: {docs:?}");
        let encoded = serde_json::to_string(docs.result.as_ref().unwrap()).unwrap();
        assert!(encoded.contains(summary), "{tool}: {encoded}");
    }
}

#[test]
fn cli_default_config_has_no_production_egress_authority() {
    let host_config: P0HostConfig = serde_json::from_value(json!({})).unwrap();
    assert!(!host_config.egress.enabled);
    assert!(host_config.egress.turn_capabilities().is_empty());
}

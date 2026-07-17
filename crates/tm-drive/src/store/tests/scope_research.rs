use super::*;

#[tokio::test]
async fn drive_host_calls_require_exact_authoritative_project_scope() {
    let (_dir, store) = store();
    for project in ["alpha", "beta"] {
        store
            .put_bytes(
                format!("# {project}").as_bytes(),
                DrivePutOptions {
                    suggested_path: Some(format!("projects/{project}/note.md")),
                    project: Some(project.to_string()),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
    }
    store
        .put_bytes(
            b"# global note",
            DrivePutOptions {
                suggested_path: Some("notes/global.md".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store, None);
    let session_id = Uuid::new_v4().to_string();

    let global = InvocationCtx::new(CapabilityGrants::default().allow("drive.search"))
        .with_session_id(session_id.clone())
        .with_session_scope("global");
    let global_results = host
        .invoke("drive.search", json!({"query": "global"}), &global)
        .await
        .unwrap();
    assert_eq!(global_results.as_array().unwrap().len(), 1);
    assert!(global_results[0]["project"].is_null());
    assert!(matches!(
        host.invoke(
            "drive.search",
            json!({"query": "alpha", "project": "alpha"}),
            &global,
        )
        .await,
        Err(HostError::CapabilityDenied(_))
    ));

    let alpha = InvocationCtx::new(CapabilityGrants::default().allow("drive.search"))
        .with_session_id(session_id)
        .with_session_scope("project:alpha");
    assert!(matches!(
        host.invoke(
            "drive.search",
            json!({"query": "beta", "project": "beta"}),
            &alpha,
        )
        .await,
        Err(HostError::CapabilityDenied(_))
    ));
    let results = host
        .invoke("drive.search", json!({"query": "note"}), &alpha)
        .await
        .unwrap();
    assert_eq!(results.as_array().unwrap().len(), 1);
    assert_eq!(results[0]["project"], json!("alpha"));
}

#[tokio::test]
async fn research_drive_returns_bounded_local_digests_and_citations() {
    let (_dir, store) = store();
    store
        .put_bytes(
            b"# Approval Drop\nManual approval gates drive writes.\nExtra detail.",
            DrivePutOptions {
                suggested_path: Some("projects/tempestmiku/approval.md".to_string()),
                project: Some("tempestmiku".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .unwrap();
    let mut host = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_drive_functions(&mut host, &mut resources, store, None);
    let ctx = InvocationCtx::new(CapabilityGrants::default().allow("research.drive"))
        .with_session_id(Uuid::new_v4().to_string())
        .with_session_scope("project:tempestmiku");

    let result = host
        .invoke(
            "research.drive",
            json!({
                "query": "approval",
                "project": "tempestmiku",
                "maxDocs": 1,
                "maxSnippets": 1,
                "maxWorkers": 0,
                "maxBytesPerDoc": 80,
                "maxDigestBytes": 80
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(result["corpus"].as_array().unwrap().len(), 1);
    assert_eq!(result["digests"].as_array().unwrap().len(), 1);
    assert_eq!(result["citations"].as_array().unwrap().len(), 1);
    assert_eq!(result["citations"][0]["sourceKind"], json!("drive"));
    assert!(result["answer"].as_str().unwrap().contains("drive://"));
    assert_eq!(result["budget"]["maxWorkers"], json!(0));
    assert_eq!(result["budget"]["agentDocs"], json!(0));
}

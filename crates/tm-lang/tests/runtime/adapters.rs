use super::*;

#[tokio::test(flavor = "current_thread")]
async fn resource_adapter_preserves_exact_scheme_grants() {
    let mut resources = ResourceRegistry::new();
    resources.register(Arc::new(WorkspaceResource));
    resources.register(Arc::new(SecretResource));

    let options = TmSandboxOptions {
        grants: CapabilityGrants::default().allow("resources.read:workspace"),
        resource_registry: resources.clone(),
        ..TmSandboxOptions::default()
    };
    let mut session = TmSandbox::new(options)
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = session
        .eval(
            "@resources.read workspace://README.md",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        output.result.as_ref().unwrap()["content"],
        "hello",
        "{output:?}"
    );
    let listed = session
        .eval("@resources.list null", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(
        listed.result,
        Some(json!([{
            "uri": "workspace://",
            "name": "workspace",
            "kind": "scheme",
            "title": null,
            "sizeBytes": null,
            "modifiedAt": null
        }]))
    );

    let denied_options = TmSandboxOptions {
        grants: CapabilityGrants::default(),
        resource_registry: resources,
        ..TmSandboxOptions::default()
    };
    let mut denied = TmSandbox::new(denied_options)
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = denied
        .eval(
            "@resources.read workspace://README.md",
            CellBudget::default(),
        )
        .await
        .unwrap();
    let error = output.error.unwrap();
    assert!(
        error.contains("unknown capability resources.read")
            || error.contains("unknown resource scheme workspace"),
        "{error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn registry_wildcard_grant_exposes_only_matching_effects() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(Patch::new(Arc::clone(&calls))));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("fs.*"),
        approval_policy: Arc::new(Approve),
        approval_timeout: Duration::from_secs(1),
        host_event_sink: events,
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let output = session
        .eval("@fs.patch {patch: \"wildcard\"}", CellBudget::default())
        .await
        .unwrap();
    assert!(output.error.is_none(), "{output:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let docs = session
        .eval("@tools.docs \"fs.patch\"", CellBudget::default())
        .await
        .unwrap();
    let docs = docs.result.unwrap();
    assert_eq!(docs["tmDeclaration"], "eff fs.patch : Json -> Json");
    assert_eq!(docs["approval"], "on-write");
    assert_eq!(docs["resumable"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn artifact_adapter_redacts_and_preserves_read_authority() {
    let temp = tempfile::tempdir().unwrap();
    let events = Arc::new(Events::default());
    let options = TmSandboxOptions {
        artifact_root: temp.path().to_path_buf(),
        session_id: "tm-artifact-test".into(),
        grants: CapabilityGrants::default().allow("resources.read:artifact"),
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    };
    let mut session = TmSandbox::new(options)
        .open(SessionConfig::default())
        .await
        .unwrap();
    let put = session
        .eval(
            "@artifacts.put {data: \"token=secret-token-123456\", title: \"fixture\"}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(put.result.as_ref().unwrap()["uri"], "artifact://0");
    let get = session
        .eval("@artifacts.get artifact://0", CellBudget::default())
        .await
        .unwrap();
    assert!(get.error.is_none(), "{get:?}");
    let content = get.result.as_ref().unwrap()["content"].as_str().unwrap();
    assert!(!content.contains("secret-token-123456"));
    assert!(content.contains("[REDACTED_"));
    let listed = session
        .eval("@artifacts.list null", CellBudget::default())
        .await
        .unwrap();
    let listed = listed.result.unwrap();
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["offset"], 0);
    assert_eq!(listed["hasMore"], false);
    assert!(listed["nextOffset"].is_null());

    for value in ["second", "third"] {
        let output = session
            .eval(
                &format!("@artifacts.put {{data: \"{value}\"}}"),
                CellBudget::default(),
            )
            .await
            .unwrap();
        assert!(output.error.is_none(), "{output:?}");
    }
    let first_page = session
        .eval("@artifacts.list {limit: 1}", CellBudget::default())
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(first_page["items"].as_array().unwrap().len(), 1);
    assert_eq!(first_page["hasMore"], true);
    assert_eq!(first_page["nextOffset"], 1);
    let last_page = session
        .eval(
            "@artifacts.list {offset: 2, limit: 1}",
            CellBudget::default(),
        )
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(last_page["items"][0]["uri"], "artifact://2");
    assert_eq!(last_page["hasMore"], false);
    let invalid_page = session
        .eval("@artifacts.list {limit: 257}", CellBudget::default())
        .await
        .unwrap();
    assert!(
        invalid_page
            .error
            .as_deref()
            .is_some_and(|error| error.contains("1..=256")),
        "{invalid_page:?}"
    );
    for source in [
        "@resources.read artifact://0",
        "@resources.preview artifact://0",
        "@resources.list null",
    ] {
        let output = session.eval(source, CellBudget::default()).await.unwrap();
        assert!(output.error.is_none(), "{source}: {output:?}");
    }
    for capability in [
        "artifacts.get",
        "artifacts.slice",
        "artifacts.list",
        "resources.read",
        "resources.preview",
        "resources.list",
    ] {
        let docs = session
            .eval(
                &format!("@tools.docs \"{capability}\""),
                CellBudget::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            docs.result.as_ref().unwrap()["sensitive"],
            true,
            "{capability}"
        );
        assert_eq!(
            docs.result.as_ref().unwrap()["approval"],
            "none",
            "{capability}"
        );
    }
    let encoded_events = serde_json::to_string(&*events.0.lock().unwrap()).unwrap();
    assert!(
        !encoded_events.contains("secret-token-123456"),
        "{encoded_events}"
    );
    assert!(!encoded_events.contains("artifact://0"), "{encoded_events}");
    assert!(events.0.lock().unwrap().iter().any(|(kind, payload)| {
        kind == "cell_start" && payload["sourcePreview"] == "[redacted]"
    }));
    assert!(events.0.lock().unwrap().iter().any(|(kind, payload)| {
        kind == "effect_result" && payload["resultPreview"] == "[redacted]"
    }));

    let denied_options = TmSandboxOptions {
        artifact_root: temp.path().to_path_buf(),
        session_id: "tm-artifact-test".into(),
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    };
    let mut denied = TmSandbox::new(denied_options)
        .open(SessionConfig::default())
        .await
        .unwrap();
    let denied = denied
        .eval("@artifacts.get artifact://0", CellBudget::default())
        .await
        .unwrap();
    assert!(
        denied
            .error
            .unwrap()
            .contains("unknown capability artifacts.get")
    );
}

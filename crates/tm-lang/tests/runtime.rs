use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tm_artifacts::ResourceContent;
use tm_core::{CancellationToken, CellBudget, Sandbox, SessionConfig, shape_result_capped};
use tm_host::{
    ApprovalDecision, ApprovalPolicy, CapabilityGrants, FsMode, GrantDoc, HostEventSink, HostFn,
    HostRegistry, InvocationCtx, LinkedFolderConfig, LinkedFolders, ResourceHandler,
    ResourceRegistry, ToolDocs,
};
use tm_lang::{RuntimeLimits, TmSandbox, TmSandboxOptions};

#[derive(Default)]
struct Events(Mutex<Vec<(String, Value)>>);

#[async_trait]
impl HostEventSink for Events {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push((event_type.into(), payload_json));
        Ok(())
    }
}

struct Approve;

#[async_trait]
impl ApprovalPolicy for Approve {
    async fn request(
        &self,
        _action: &str,
        _timeout: Duration,
    ) -> tm_host::Result<ApprovalDecision> {
        Ok(ApprovalDecision::Approved)
    }
}

struct Deny;
#[async_trait]
impl ApprovalPolicy for Deny {
    async fn request(
        &self,
        _action: &str,
        _timeout: Duration,
    ) -> tm_host::Result<ApprovalDecision> {
        Ok(ApprovalDecision::Denied)
    }
}

struct Cancelled;
impl CancellationToken for Cancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

struct WorkspaceResource;

#[async_trait]
impl ResourceHandler for WorkspaceResource {
    fn scheme(&self) -> &str {
        "workspace"
    }

    fn capability(&self) -> &str {
        "resources.read:workspace"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> tm_host::Result<ResourceContent> {
        Ok(ResourceContent {
            uri: uri.into(),
            kind: "text".into(),
            mime: "text/plain".into(),
            title: Some("fixture".into()),
            size_bytes: 5,
            selector: selector.map(str::to_owned),
            has_more: false,
            content: "hello".into(),
            preview: "hello".into(),
        })
    }
}

struct SecretResource;

#[async_trait]
impl ResourceHandler for SecretResource {
    fn scheme(&self) -> &str {
        "secret"
    }

    fn capability(&self) -> &str {
        "resources.read:secret"
    }

    async fn read(
        &self,
        _uri: &str,
        _selector: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> tm_host::Result<ResourceContent> {
        unreachable!("ungranted resource handler must not execute")
    }
}

struct Patch {
    calls: Arc<AtomicUsize>,
    docs: ToolDocs,
}

impl Patch {
    fn new(calls: Arc<AtomicUsize>) -> Self {
        Self {
            calls,
            docs: ToolDocs {
                name: "fs.patch".into(),
                namespace: "fs".into(),
                summary: "Apply a patch".into(),
                description: None,
                signature: "fs.patch(args)".into(),
                args_schema: json!({"type":"object"}),
                result_schema: Some(json!({"type":"object"})),
                examples: vec![],
                errors: vec![],
                grants: vec![GrantDoc {
                    kind: "capability".into(),
                    description: "fs.patch".into(),
                }],
                sensitive: true,
                approval: "on-write".into(),
                since: "0.1".into(),
                stability: "stable".into(),
            },
        }
    }
}

#[async_trait]
impl HostFn for Patch {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        ctx.require_approval("fs.patch").await?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        if args.get("patch").and_then(Value::as_str) == Some("denied") {
            return Err(tm_host::HostError::CapabilityDenied("fs.patch".into()));
        }
        if args.get("patch").and_then(Value::as_str) == Some("fail") {
            return Err(tm_host::HostError::InvalidArgs("scripted failure".into()));
        }
        Ok(json!({"applied": true, "args": args}))
    }
}

struct Spill {
    docs: ToolDocs,
}

impl Spill {
    fn new() -> Self {
        Self {
            docs: ToolDocs {
                name: "test.spill".into(),
                namespace: "test".into(),
                summary: "Return a spilled result".into(),
                description: None,
                signature: "test.spill(args)".into(),
                args_schema: json!({"type":"null"}),
                result_schema: Some(json!({"type":"object"})),
                examples: vec![],
                errors: vec![],
                grants: vec![GrantDoc {
                    kind: "capability".into(),
                    description: "test.spill".into(),
                }],
                sensitive: false,
                approval: "none".into(),
                since: "test".into(),
                stability: "experimental".into(),
            },
        }
    }
}

#[async_trait]
impl HostFn for Spill {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, _args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        Ok(json!({
            "stdout": "x".repeat(256),
            "truncated": true,
            "artifact": {"uri": "artifact://7"}
        }))
    }
}

struct ParallelProbe {
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    docs: ToolDocs,
}

impl ParallelProbe {
    fn new(active: Arc<AtomicUsize>, peak: Arc<AtomicUsize>) -> Self {
        Self {
            active,
            peak,
            docs: ToolDocs {
                name: "test.probe".into(),
                namespace: "test".into(),
                summary: "Hold a call open to prove batch overlap".into(),
                description: None,
                signature: "test.probe(args)".into(),
                args_schema: json!({"type":"object"}),
                result_schema: Some(json!({"type":"object"})),
                examples: vec![],
                errors: vec![],
                grants: vec![GrantDoc {
                    kind: "capability".into(),
                    description: "test.probe".into(),
                }],
                sensitive: false,
                approval: "none".into(),
                since: "test".into(),
                stability: "experimental".into(),
            },
        }
    }
}

#[async_trait]
impl HostFn for ParallelProbe {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(40)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(args)
    }
}

fn sandbox(events: Arc<Events>, calls: Arc<AtomicUsize>) -> TmSandbox {
    sandbox_with_policy(events, calls, Arc::new(Approve))
}

fn sandbox_with_policy(
    events: Arc<Events>,
    calls: Arc<AtomicUsize>,
    policy: Arc<dyn ApprovalPolicy>,
) -> TmSandbox {
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(Patch::new(calls)));
    TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("fs.patch"),
        approval_policy: policy,
        approval_timeout: Duration::from_secs(1),
        host_event_sink: events,
        ..TmSandboxOptions::default()
    })
}

#[tokio::test(flavor = "current_thread")]
async fn execute_batch_overlaps_host_calls_and_merges_bindings_in_response_order() {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(ParallelProbe::new(
        Arc::clone(&active),
        Arc::clone(&peak),
    )));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("test.probe"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "let shared = @test.probe {value: 1}".into(),
                "let second = @test.probe {value: 2};\nlet shared = 2".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(
        outputs.iter().all(|output| output.error.is_none()),
        "{outputs:?}"
    );
    assert_eq!(peak.load(Ordering::SeqCst), 2);
    assert_eq!(
        session
            .eval("shared", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(2))
    );
    assert_eq!(
        session
            .eval("second", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!({"value": 2}))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn execute_batch_schedules_forward_declarative_dependencies_in_response_order() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "let first = 1".into(),
                "let second = first + 1".into(),
                "second + 1".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        outputs
            .iter()
            .map(|output| output.result.clone())
            .collect::<Vec<_>>(),
        vec![Some(json!(1)), Some(json!(2)), Some(json!(3))]
    );
    assert!(outputs.iter().all(|output| output.error.is_none()));
    assert_eq!(
        session
            .eval("second", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(2))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn execute_batch_reads_the_latest_earlier_rebinding_and_merges_by_response_order() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    session
        .eval("let shared = 0", CellBudget::default())
        .await
        .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "let shared = 1".into(),
                "let derived = shared + 1".into(),
                "let shared = 3".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert_eq!(outputs[1].result, Some(json!(2)));
    assert_eq!(
        session
            .eval("{shared, derived}", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!({"shared": 3, "derived": 2}))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn execute_batch_schedules_closure_and_interpolation_dependencies() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "let seed = 4".into(),
                "fun add_seed value = value + seed".into(),
                "let rendered = \"value #{add_seed 2}\"".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(outputs.iter().all(|output| output.error.is_none()));
    assert_eq!(outputs[2].result, Some(json!("value 6")));
    assert_eq!(
        session
            .eval("rendered", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!("value 6"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn interpolation_rejects_multiple_forms_before_running_effects() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(events, Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();

    let output = session
        .eval(
            r##""#{1; @fs.patch {patch: \"skipped\"}}""##,
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("interpolation must contain one expression")),
        "{output:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn direct_fs_read_path_is_normalized_to_the_host_argument_schema() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hello.txt"), "hello\n").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".into(),
        path: root.path().to_path_buf(),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let mut session = TmSandbox::new(TmSandboxOptions {
        artifact_root: artifacts.path().to_path_buf(),
        session_id: "direct-fs-read".into(),
        session_scope: Some("project:repo".into()),
        linked_folders: Some(linked),
        grants: CapabilityGrants::default().allow("fs.read"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let output = session
        .eval(
            "let path = \"repo:hello.txt\"; @fs.read path",
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(output.error.is_none(), "{output:?}");
    assert_eq!(output.result.as_ref().unwrap()["content"], "hello\n");
}

#[tokio::test(flavor = "current_thread")]
async fn failed_batch_producer_blocks_dependent_effect_without_falling_back() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();
    session
        .eval("let payload = 7", CellBudget::default())
        .await
        .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "let payload = 1 / 0".into(),
                "@fs.patch {patch: payload}".into(),
                "let unrelated = 9".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(outputs[0].error.is_some());
    assert_eq!(
        outputs[1].error.as_deref(),
        Some(
            "BatchDependencyError: execute call 2 requires binding(s) [payload] from failed execute call 1"
        )
    );
    assert_eq!(outputs[2].result, Some(json!(9)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        session
            .eval("{payload, unrelated}", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!({"payload": 7, "unrelated": 9}))
    );

    let events = events.0.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(event, _)| event == "cell_start")
            .count(),
        5
    );
    assert_eq!(
        events
            .iter()
            .filter(|(event, payload)| { event == "cell_result" && payload["status"] == "failed" })
            .count(),
        2
    );
    assert!(events.iter().all(|(event, _)| event != "effect_start"));
}

#[tokio::test(flavor = "current_thread")]
async fn unparseable_batch_cell_blocks_every_later_cell_fail_closed() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();
    session
        .eval("let payload = 7", CellBudget::default())
        .await
        .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "let payload =".into(),
                "@fs.patch {patch: payload}".into(),
                "let unrelated = 9".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(outputs[0].error.is_some());
    for output in &outputs[1..] {
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains("BatchDependencyError")
                    && error.contains("<unknown bindings>")),
            "{output:?}"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        session
            .eval("payload", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(7))
    );
    assert!(
        session
            .eval("unrelated", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("unbound name unrelated")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn execute_batch_does_not_resolve_backward_declarative_dependencies() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let outputs = session
        .eval_batch(
            &["later".into(), "let later = 1".into()],
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(
        outputs[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unbound name later"))
    );
    assert_eq!(outputs[1].result, Some(json!(1)));
}

#[tokio::test(flavor = "current_thread")]
async fn persistent_bindings_commit_atomically_and_reset() {
    let events = Arc::new(Events::default());
    let mut session = sandbox(events, Arc::new(AtomicUsize::new(0)))
        .open(SessionConfig::default())
        .await
        .unwrap();
    let first = session
        .eval("let x = 1", CellBudget::default())
        .await
        .unwrap();
    assert!(first.error.is_none(), "{first:?}");
    assert_eq!(first.result, Some(json!(1)));
    let failed = session
        .eval("let y = 2;\n1 / 0", CellBudget::default())
        .await
        .unwrap();
    assert!(failed.error.unwrap().contains("division by zero"));
    assert_eq!(
        session
            .eval("x", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(1))
    );
    assert!(
        session
            .eval("y", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("unbound name y")
    );
    session.reset().await.unwrap();
    assert!(
        session
            .eval("x", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("unbound name x")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn approval_resumes_same_effect_node_and_executes_once() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = session
        .eval("@fs.patch {patch: \"secret\"}", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(output.result.unwrap()["applied"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let events = events.0.lock().unwrap();
    let ordered: Vec<_> = events.iter().map(|(kind, _)| kind.as_str()).collect();
    let start = ordered
        .iter()
        .position(|kind| *kind == "effect_start")
        .unwrap();
    let start_payload = events
        .iter()
        .find(|(kind, _)| kind == "effect_start")
        .map(|(_, payload)| payload)
        .unwrap();
    assert_eq!(start_payload["argsPreview"], "[redacted]");
    let suspended = ordered
        .iter()
        .position(|kind| *kind == "effect_suspended")
        .unwrap();
    let resumed = ordered
        .iter()
        .position(|kind| *kind == "effect_resumed")
        .unwrap();
    let result = ordered
        .iter()
        .position(|kind| *kind == "effect_result")
        .unwrap();
    assert!(start < suspended && suspended < resumed && resumed < result);
    let ids: Vec<_> = events
        .iter()
        .filter(|(kind, _)| {
            [
                "effect_start",
                "effect_suspended",
                "effect_resumed",
                "effect_result",
            ]
            .contains(&kind.as_str())
        })
        .map(|(_, payload)| payload["nodeId"].as_str().unwrap())
        .collect();
    assert!(ids.windows(2).all(|pair| pair[0] == pair[1]));
}

#[tokio::test(flavor = "current_thread")]
async fn denial_and_timeout_are_terminal_and_never_execute_effect() {
    for policy in [
        Arc::new(Deny) as Arc<dyn ApprovalPolicy>,
        Arc::new(tm_host::DefaultDenyApprovalPolicy) as Arc<dyn ApprovalPolicy>,
    ] {
        let events = Arc::new(Events::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut session = sandbox_with_policy(Arc::clone(&events), Arc::clone(&calls), policy)
            .open(SessionConfig::default())
            .await
            .unwrap();
        let output = session
            .eval("@fs.patch {patch: \"x\"}", CellBudget::default())
            .await
            .unwrap();
        assert!(output.error.is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let events = events.0.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|(kind, payload)| kind == "effect_result" && payload["status"] == "failed")
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn par_map_owns_effect_children_and_reports_bounded_progress() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = session
        .eval(
            "[{patch: \"a\"}, {patch: \"b\"}] |> par map @fs.patch",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(output.error.is_none(), "{output:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let events = events.0.lock().unwrap();
    let kinds: Vec<_> = events.iter().map(|(kind, _)| kind.as_str()).collect();
    let scope_start = kinds
        .iter()
        .position(|kind| *kind == "scope_start")
        .unwrap();
    let first_effect = kinds
        .iter()
        .position(|kind| *kind == "effect_start")
        .unwrap();
    let scope_result = kinds
        .iter()
        .rposition(|kind| *kind == "scope_result")
        .unwrap();
    assert!(scope_start < first_effect && first_effect < scope_result);
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == "scope_progress")
            .count(),
        2
    );
}

#[tokio::test(flavor = "current_thread")]
async fn par_map_first_failure_cancels_unstarted_siblings() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), Arc::clone(&calls))
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = session
        .eval(
            "[{patch: \"a\"}, {patch: \"fail\"}, {patch: \"never\"}] |> par map @fs.patch",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(output.error.unwrap().contains("scripted failure"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let events = events.0.lock().unwrap();
    let terminal = events
        .iter()
        .find(|(kind, payload)| kind == "scope_result" && payload["status"] == "failed")
        .unwrap();
    assert_eq!(terminal.1["cancelledSiblings"], 1);
}

#[tokio::test(flavor = "current_thread")]
async fn limits_and_structured_scope_fail_closed() {
    let events = Arc::new(Events::default());
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(Patch::new(Arc::new(AtomicUsize::new(0)))));
    let mut options = TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default(),
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    };
    options.limits = RuntimeLimits {
        steps: 50,
        print_bytes: 8,
        preview_bytes: 16,
    };
    let mut session = TmSandbox::new(options)
        .open(SessionConfig::default())
        .await
        .unwrap();
    assert!(
        session
            .eval("print \"0123456789\"", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("budget")
    );
    let output = session
        .eval(
            "par [1, 2]",
            CellBudget {
                wall_ms: 1000,
                output_bytes: 100,
            },
        )
        .await
        .unwrap();
    assert_eq!(output.result, Some(json!([1, 2])));
    let kinds: Vec<_> = events
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|(kind, _)| kind.clone())
        .collect();
    assert!(kinds.contains(&"scope_start".into()));
    assert!(kinds.contains(&"scope_result".into()));
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_and_result_caps_do_not_commit_bindings() {
    let events = Arc::new(Events::default());
    let mut options = TmSandboxOptions {
        grants: CapabilityGrants::default(),
        host_event_sink: events,
        ..TmSandboxOptions::default()
    };
    options.cancellation = Some(Arc::new(Cancelled));
    let mut cancelled = TmSandbox::new(options.clone())
        .open(SessionConfig::default())
        .await
        .unwrap();
    assert!(
        cancelled
            .eval("let x = 1", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("cancelled")
    );

    options.cancellation = None;
    let mut capped = TmSandbox::new(options)
        .open(SessionConfig::default())
        .await
        .unwrap();
    let failed = capped
        .eval(
            "let huge = [\"0123456789\"]",
            CellBudget {
                wall_ms: 1000,
                output_bytes: 4,
            },
        )
        .await
        .unwrap();
    assert!(failed.error.unwrap().contains("result/output budget"));
    assert!(
        capped
            .eval("huge", CellBudget::default())
            .await
            .unwrap()
            .error
            .unwrap()
            .contains("unbound name huge")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn spilled_results_reach_the_core_shaper_with_artifact_references_intact() {
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(Spill::new()));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("test.spill"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let output = session
        .eval(
            "@test.spill null",
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 64,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        output.result.as_ref().unwrap()["artifact"]["uri"],
        json!("artifact://7"),
        "{output:?}"
    );
    let shaped = shape_result_capped(&output, 64);
    assert!(shaped.contains("artifact://7"), "{shaped}");
    assert!(shaped.contains("bytes elided"), "{shaped}");
}

#[tokio::test(flavor = "current_thread")]
async fn integer_overflow_returns_diagnostics_without_poisoning_the_session() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    for (source, operation) in [
        ("9223372036854775807 + 1", "addition"),
        ("(-9223372036854775807 - 1) / -1", "division"),
        ("-(-9223372036854775807 - 1)", "negation"),
    ] {
        let output = session.eval(source, CellBudget::default()).await.unwrap();
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains(&format!("integer overflow in {operation}"))),
            "{source}: {output:?}"
        );
    }

    assert_eq!(
        session
            .eval("40 + 2", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(42))
    );

    assert_eq!(
        session
            .eval("[9223372036854775807, -1] |> sum", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(9223372036854775806_i64))
    );
    let overflow = session
        .eval("[9223372036854775807, 1] |> sum", CellBudget::default())
        .await
        .unwrap();
    assert!(
        overflow
            .error
            .as_deref()
            .is_some_and(|error| error.contains("integer overflow in sum")),
        "{overflow:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dynamic_boolean_operands_are_checked_at_runtime() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    session
        .eval("let dynamic = \"not-a-bool\"", CellBudget::default())
        .await
        .unwrap();

    for source in ["dynamic or true", "false or dynamic"] {
        let output = session.eval(source, CellBudget::default()).await.unwrap();
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains("or requires boolean operands")),
            "{source}: {output:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn top_level_type_constructors_do_not_escape_their_cell() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let local = session
        .eval("type Choice = | No; No", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(local.result, Some(json!({"tag": "No"})), "{local:?}");

    let escaped = session.eval("No", CellBudget::default()).await.unwrap();
    assert!(
        escaped
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unknown constructor No")),
        "{escaped:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn host_error_handlers_receive_structured_payload_fields() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(events, calls)
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = session
        .eval(
            "handle (@fs.patch {patch: \"denied\"}) with error { | CapabilityDeniedError {capability, ...} -> capability }",
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert_eq!(output.result, Some(json!("fs.patch")), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn functions_patterns_and_table_prelude_match_frozen_semantics() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let factorial = session
        .eval(
            "fun fact n = if n == 0 then 1 else n * fact (n - 1);\nfact 5",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(factorial.result, Some(json!(120)), "{factorial:?}");

    let partial_map = session
        .eval(
            "([1, 2] |> map) (fun value -> value + 1)",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(partial_map.result, Some(json!([2, 3])), "{partial_map:?}");

    let partial_display = session
        .eval("([1] |> display) {kind: \"json\"}", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(
        partial_display.result,
        Some(json!([1])),
        "{partial_display:?}"
    );

    let block_scope = session
        .eval(
            "let outer = 7; handle do { let outer = 1; rethrow \"boom\" } with error { | Rethrown _ -> outer }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(block_scope.result, Some(json!(7)), "{block_scope:?}");

    let lexical_closure = session
        .eval(
            "fun helper value = value + 1; fun captured value = helper value; fun helper value = value + 100; captured 1",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        lexical_closure.result,
        Some(json!(2)),
        "{lexical_closure:?}"
    );

    let pattern_error = session
        .eval(
            "let x = 1; let f = fun [a] -> a; let x = 2; handle f 1 with error { | TypeError _ -> x }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(pattern_error.result, Some(json!(2)), "{pattern_error:?}");
    assert_eq!(
        session
            .eval("x", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(2))
    );

    let none = session
        .eval(
            "match None { | Some value -> value | None -> 42 }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(none.result, Some(json!(42)), "{none:?}");

    let local = session
        .eval(
            "do { type Choice = | Yes Int | No; let choice = Yes 3; match choice { | Yes value -> \"value #{value + 1}\" | No -> \"none\" } }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(local.result, Some(json!("value 4")), "{local:?}");
    let escaped = session
        .eval("\"literal \\# hash\"", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(escaped.result, Some(json!("literal # hash")), "{escaped:?}");
    let concatenated = session
        .eval("\"tempest\" + \"miku\"", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(concatenated.result, Some(json!("tempestmiku")));

    let summed = session
        .eval("let values = [1, 2]; values |> sum", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(summed.result, Some(json!(3)), "{summed:?}");

    let lexical_row = session
        .eval(
            "let age = 100; table [{age:10}] |> select {lexical: age, column: row.age}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        lexical_row.result,
        Some(json!([{"column": 10, "lexical": 100}])),
        "{lexical_row:?}"
    );

    let numeric_sort = session
        .eval(
            "table [{n: 2}, {n: 10}, {n: 1}] |> sort_by n asc",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        numeric_sort.result,
        Some(json!([{"n": 1}, {"n": 2}, {"n": 10}])),
        "{numeric_sort:?}"
    );

    let typed_groups = session
        .eval(
            "table [{key: 1}, {key: \"1\"}, {key: true}, {key: \"true\"}] |> group_by key",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        typed_groups.result,
        Some(json!([
            {"key": true, "rows": [{"key": true}]},
            {"key": 1, "rows": [{"key": 1}]},
            {"key": "1", "rows": [{"key": "1"}]},
            {"key": "true", "rows": [{"key": "true"}]}
        ])),
        "{typed_groups:?}"
    );

    let table = session
        .eval(
            "table [{file: \"a\", todos: 2}, {file: \"a\", todos: 3}, {file: \"b\", todos: 1}] |> group_by [file] |> aggregate {file: key, todos: sum todos, hits: count} |> sort_by hits desc",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        table.result,
        Some(json!([
            {"file": ["a"], "hits": 2, "todos": 5},
            {"file": ["b"], "hits": 1, "todos": 1}
        ])),
        "{table:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn display_is_bounded_and_help_returns_capability_docs() {
    let events = Arc::new(Events::default());
    let mut session = sandbox(Arc::clone(&events), Arc::new(AtomicUsize::new(0)))
        .open(SessionConfig::default())
        .await
        .unwrap();

    let help = session
        .eval("help @fs.patch", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(help.result.as_ref().unwrap()["name"], json!("fs.patch"));
    assert_eq!(
        help.result.as_ref().unwrap()["signature"],
        json!("fs.patch(args)")
    );
    assert_eq!(help.result.as_ref().unwrap()["approval"], json!("on-write"));

    let display = session
        .eval(
            "\"0123456789012345678901234567890123456789\" |> display {kind: \"text\"}",
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 32,
            },
        )
        .await
        .unwrap();
    assert!(
        display
            .error
            .as_deref()
            .is_some_and(|error| error.contains("display/output budget exceeded")),
        "{display:?}"
    );
    assert!(
        events
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|(event, _)| event != "display")
    );
}

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
    let options = TmSandboxOptions {
        artifact_root: temp.path().to_path_buf(),
        session_id: "tm-artifact-test".into(),
        grants: CapabilityGrants::default().allow("resources.read:artifact"),
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
    assert_eq!(listed.result.unwrap().as_array().unwrap().len(), 1);

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

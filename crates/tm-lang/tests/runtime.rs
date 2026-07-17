use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use futures::{FutureExt, future::BoxFuture};
use serde_json::{Value, json};
use tm_artifacts::ResourceContent;
use tm_core::{CancellationToken, CellBudget, Sandbox, SessionConfig};
use tm_host::{
    ApprovalDecision, ApprovalPolicy, CapabilityGrants, FsMode, GrantDoc, HostError, HostEventSink,
    HostFn, HostRegistry, InvocationCtx, LinkedFolderConfig, LinkedFolders, ResourceHandler,
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

#[derive(Default)]
struct YieldingStartEvents {
    events: Mutex<Vec<(String, Value)>>,
    effect_start_seen: tokio::sync::Notify,
    paused: AtomicBool,
}

#[derive(Default)]
struct YieldingCellStartEvents {
    events: Mutex<Vec<(String, Value)>>,
    cell_start_seen: tokio::sync::Notify,
    paused: AtomicBool,
}

#[derive(Default)]
struct YieldingBindingEvents {
    events: Mutex<Vec<(String, Value)>>,
    binding_seen: tokio::sync::Notify,
    release: tokio::sync::Notify,
    paused: AtomicBool,
}

#[derive(Default)]
struct StallingSecondCellResult {
    seen: AtomicUsize,
}

struct FailOnceOnEvent {
    event_type: &'static str,
    failed: AtomicBool,
}

impl FailOnceOnEvent {
    fn new(event_type: &'static str) -> Self {
        Self {
            event_type,
            failed: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl HostEventSink for FailOnceOnEvent {
    async fn emit(&self, event_type: &str, _payload_json: Value) -> tm_host::Result<()> {
        if event_type == self.event_type && !self.failed.swap(true, Ordering::SeqCst) {
            return Err(HostError::HostCall(format!(
                "injected {event_type} persistence failure"
            )));
        }
        Ok(())
    }
}

#[derive(Default)]
struct DurableThenStallingBindingEvents {
    events: Mutex<Vec<(String, Value)>>,
    binding_seen: tokio::sync::Notify,
    stalled: AtomicBool,
}

#[async_trait]
impl HostEventSink for StallingSecondCellResult {
    async fn emit(&self, event_type: &str, _payload_json: Value) -> tm_host::Result<()> {
        if event_type == "cell_result" && self.seen.fetch_add(1, Ordering::SeqCst) == 1 {
            std::future::pending::<()>().await;
        }
        Ok(())
    }
}

#[async_trait]
impl HostEventSink for DurableThenStallingBindingEvents {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        self.events
            .lock()
            .unwrap()
            .push((event_type.into(), payload_json));
        if event_type == "binding_committed" && !self.stalled.swap(true, Ordering::SeqCst) {
            self.binding_seen.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(())
    }
}

#[async_trait]
impl HostEventSink for YieldingBindingEvents {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        if event_type == "binding_committed" && !self.paused.swap(true, Ordering::SeqCst) {
            self.binding_seen.notify_one();
            self.release.notified().await;
        }
        self.events
            .lock()
            .unwrap()
            .push((event_type.into(), payload_json));
        Ok(())
    }
}

#[async_trait]
impl HostEventSink for YieldingCellStartEvents {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        if event_type == "cell_start" && !self.paused.swap(true, Ordering::SeqCst) {
            self.cell_start_seen.notify_one();
            std::future::pending::<()>().await;
        }
        self.events
            .lock()
            .unwrap()
            .push((event_type.into(), payload_json));
        Ok(())
    }
}

#[async_trait]
impl HostEventSink for YieldingStartEvents {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        self.events
            .lock()
            .unwrap()
            .push((event_type.into(), payload_json));
        if event_type == "effect_start" && !self.paused.swap(true, Ordering::SeqCst) {
            self.effect_start_seen.notify_one();
            std::future::pending::<()>().await;
        }
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

struct RecordingApprove(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl ApprovalPolicy for RecordingApprove {
    async fn request(&self, action: &str, _timeout: Duration) -> tm_host::Result<ApprovalDecision> {
        self.0.lock().unwrap().push(action.to_string());
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

struct ApprovalFailure;

#[async_trait]
impl ApprovalPolicy for ApprovalFailure {
    async fn request(
        &self,
        _action: &str,
        _timeout: Duration,
    ) -> tm_host::Result<ApprovalDecision> {
        Err(tm_host::HostError::HostCall(
            "approval backend unavailable".into(),
        ))
    }
}

struct Cancelled;
impl CancellationToken for Cancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

#[derive(Default)]
struct TriggerCancellation {
    cancelled: AtomicBool,
    notify: tokio::sync::Notify,
}

impl TriggerCancellation {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }
}

impl CancellationToken for TriggerCancellation {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn cancelled(&self) -> BoxFuture<'_, ()> {
        async move {
            if !self.is_cancelled() {
                self.notify.notified().await;
            }
        }
        .boxed()
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
            "artifact": {
                "uri": "artifact://7",
                "id": "7",
                "kind": "text",
                "mime": "text/plain",
                "title": "spill",
                "size_bytes": 256,
                "preview": "xxxxxxxx"
            }
        }))
    }
}

struct ParallelProbe {
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    docs: ToolDocs,
}

struct BlockingEffect {
    started: Arc<tokio::sync::Notify>,
    docs: ToolDocs,
}

struct FailAfterStart {
    started: Arc<tokio::sync::Notify>,
    docs: ToolDocs,
}

impl BlockingEffect {
    fn new(started: Arc<tokio::sync::Notify>) -> Self {
        Self {
            started,
            docs: ToolDocs {
                name: "test.block".into(),
                namespace: "test".into(),
                summary: "Block until the evaluator is cancelled".into(),
                description: None,
                signature: "test.block(args)".into(),
                args_schema: json!({}),
                result_schema: None,
                examples: vec![],
                errors: vec![],
                grants: vec![GrantDoc {
                    kind: "capability".into(),
                    description: "test.block".into(),
                }],
                sensitive: false,
                approval: "none".into(),
                since: "test".into(),
                stability: "experimental".into(),
            },
        }
    }
}

impl FailAfterStart {
    fn new(started: Arc<tokio::sync::Notify>) -> Self {
        Self {
            started,
            docs: ToolDocs {
                name: "test.fail_after_start".into(),
                namespace: "test".into(),
                summary: "Fail after the paired blocking effect starts".into(),
                description: None,
                signature: "test.fail_after_start(args)".into(),
                args_schema: json!({}),
                result_schema: None,
                examples: vec![],
                errors: vec![],
                grants: vec![GrantDoc {
                    kind: "capability".into(),
                    description: "test.fail_after_start".into(),
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
impl HostFn for BlockingEffect {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, _args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        self.started.notify_one();
        std::future::pending().await
    }
}

#[async_trait]
impl HostFn for FailAfterStart {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, _args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        self.started.notified().await;
        Err(tm_host::HostError::HostCall("nested sibling failed".into()))
    }
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
async fn execute_batch_serializes_state_writes_and_merges_bindings_in_response_order() {
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
    assert_eq!(peak.load(Ordering::SeqCst), 1);
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
async fn execute_batch_honors_configured_parallelism() {
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
        limits: RuntimeLimits {
            parallelism: 1,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let outputs = session
        .eval_batch(
            &[
                "@test.probe {value: 1}".into(),
                "@test.probe {value: 2}".into(),
            ],
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(outputs.iter().all(|output| output.error.is_none()));
    assert_eq!(peak.load(Ordering::SeqCst), 1);
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
async fn authorized_linked_alias_uri_is_normalized_to_the_host_argument_schema() {
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
            "let path = repo:hello.txt; @fs.read path",
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(output.error.is_none(), "{output:?}");
    assert_eq!(output.result.as_ref().unwrap()["content"], "hello\n");
}

#[tokio::test(flavor = "current_thread")]
async fn linked_alias_uri_outside_the_authoritative_scope_is_rejected_before_host_call() {
    let root = tempfile::tempdir().unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".into(),
        path: root.path().to_path_buf(),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let mut session = TmSandbox::new(TmSandboxOptions {
        session_id: "scoped-linked-alias".into(),
        session_scope: Some("project:other".into()),
        linked_folders: Some(linked),
        grants: CapabilityGrants::default().allow("fs.read"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let output = session
        .eval("@fs.read repo:hello.txt", CellBudget::default())
        .await
        .unwrap();

    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unknown resource scheme repo")),
        "{output:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_defaults_do_not_grant_http_or_artifact_reads() {
    let artifacts = tempfile::tempdir().unwrap();
    let mut http_allowlist = std::collections::BTreeMap::new();
    http_allowlist.insert("https://example.test/data".into(), "fixture".into());
    let mut denied = TmSandbox::new(TmSandboxOptions {
        artifact_root: artifacts.path().to_path_buf(),
        session_id: "exact-default-grants".into(),
        http_allowlist: http_allowlist.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    for source in [
        "@http.get {url: \"https://example.test/data\"}",
        "@resources.read artifact://0",
        "@artifacts.get artifact://0",
        "@artifacts.list null",
    ] {
        let output = denied.eval(source, CellBudget::default()).await.unwrap();
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains("unknown capability")
                    || error.contains("unknown resource scheme")),
            "{source}: {output:?}"
        );
    }

    let mut list_only = TmSandbox::new(TmSandboxOptions {
        artifact_root: artifacts.path().to_path_buf(),
        session_id: "artifact-list-without-read-umbrella".into(),
        grants: CapabilityGrants::default().allow("artifacts.list"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let list = list_only
        .eval("@artifacts.list null", CellBudget::default())
        .await
        .unwrap();
    assert!(
        list.error
            .as_deref()
            .is_some_and(|error| error.contains("resources.read:artifact")),
        "{list:?}"
    );

    let mut allowed = TmSandbox::new(TmSandboxOptions {
        artifact_root: artifacts.path().to_path_buf(),
        session_id: "exact-explicit-grants".into(),
        http_allowlist,
        grants: CapabilityGrants::default().allow("http.get"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let output = allowed
        .eval(
            "@http.get {url: \"https://example.test/data\"}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(output.result, Some(json!("fixture")), "{output:?}");
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
async fn stalled_dependency_terminal_persistence_is_bounded() {
    let events = Arc::new(StallingSecondCellResult::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let cells = vec!["let payload = 1 / 0".into(), "payload".into()];

    let error = tokio::time::timeout(
        Duration::from_secs(3),
        session.eval_batch(&cells, CellBudget::default()),
    )
    .await
    .expect("dependency terminal persistence must respect its grace period")
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("terminal event persistence deadline exceeded"),
        "{error:?}"
    );
    assert_eq!(events.seen.load(Ordering::SeqCst), 2);
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
async fn approval_backend_errors_terminalize_suspended_effects_once() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox_with_policy(
        Arc::clone(&events),
        Arc::clone(&calls),
        Arc::new(ApprovalFailure),
    )
    .open(SessionConfig::default())
    .await
    .unwrap();
    let output = session
        .eval("@fs.patch {patch: \"x\"}", CellBudget::default())
        .await
        .unwrap();
    assert!(output.error.is_some(), "{output:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let events = events.0.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "effect_result")
            .count(),
        1,
        "{events:?}"
    );
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
    let scope_id = events
        .iter()
        .find(|(kind, _)| kind == "scope_start")
        .unwrap()
        .1["nodeId"]
        .clone();
    assert!(
        events
            .iter()
            .filter(|(kind, _)| kind == "effect_start")
            .all(|(_, payload)| payload["parentNodeId"] == scope_id)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn par_record_and_map_overlap_effects_and_preserve_result_order() {
    for source in [
        "par {first: @test.probe {value: 1}, second: @test.probe {value: 2}}",
        "[{value: 1}, {value: 2}] |> par map @test.probe",
    ] {
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

        let output = session.eval(source, CellBudget::default()).await.unwrap();
        assert!(output.error.is_none(), "{source}: {output:?}");
        assert_eq!(peak.load(Ordering::SeqCst), 2, "{source}");
        if source.starts_with('[') {
            assert_eq!(
                output.result,
                Some(json!([{"value": 1}, {"value": 2}])),
                "{source}"
            );
        } else {
            assert_eq!(
                output.result,
                Some(json!({"first": {"value": 1}, "second": {"value": 2}})),
                "{source}"
            );
        }
    }
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
async fn outer_parallel_failure_terminalizes_nested_effects_and_scopes_child_first() {
    let events = Arc::new(Events::default());
    let started = Arc::new(tokio::sync::Notify::new());
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(BlockingEffect::new(Arc::clone(&started))));
    registry.register(Arc::new(FailAfterStart::new(started)));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default()
            .allow("test.block")
            .allow("test.fail_after_start"),
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let output = session
        .eval(
            "par {nested: par {blocked: @test.block null}, failing: @test.fail_after_start null}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(output.error.unwrap().contains("nested sibling failed"));

    let events = events.0.lock().unwrap();
    let scopes = events
        .iter()
        .filter(|(kind, _)| kind == "scope_start")
        .collect::<Vec<_>>();
    assert_eq!(scopes.len(), 2, "{events:?}");
    let outer = scopes[0].1["nodeId"].clone();
    let nested = scopes[1].1["nodeId"].clone();
    assert_eq!(scopes[1].1["parentNodeId"], outer);

    let block_start = events
        .iter()
        .find(|(kind, payload)| kind == "effect_start" && payload["capability"] == "test.block")
        .expect("blocking effect starts inside nested scope");
    assert_eq!(block_start.1["parentNodeId"], nested);
    let block_terminal = events
        .iter()
        .find(|(kind, payload)| {
            kind == "effect_result" && payload["nodeId"] == block_start.1["nodeId"]
        })
        .expect("dropped nested effect receives a terminal result");
    assert_eq!(block_terminal.1["status"], "cancelled");
    assert_eq!(block_terminal.1["parentNodeId"], nested);

    let nested_result = events
        .iter()
        .position(|(kind, payload)| {
            kind == "scope_result"
                && payload["nodeId"] == nested
                && payload["status"] == "cancelled"
        })
        .expect("nested scope receives a cancelled terminal");
    let outer_result = events
        .iter()
        .position(|(kind, payload)| {
            kind == "scope_result" && payload["nodeId"] == outer && payload["status"] == "failed"
        })
        .expect("outer scope receives a failed terminal");
    assert!(nested_result < outer_result, "{events:?}");
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
        ..RuntimeLimits::default()
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
        host_event_sink: events.clone(),
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
    let cancelled_batch = cancelled
        .eval_batch(
            &["@artifacts.put {data: \"cancelled-batch-secret\"}".into()],
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(
        cancelled_batch[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cancelled"))
    );

    options.cancellation = None;
    let mut capped = TmSandbox::new(options)
        .open(SessionConfig::default())
        .await
        .unwrap();
    let timed_out = capped
        .eval(
            "@artifacts.put {data: \"preflight-secret\"}",
            CellBudget {
                wall_ms: 0,
                output_bytes: 64,
            },
        )
        .await
        .unwrap();
    assert!(timed_out.error.unwrap().contains("TimeoutError"));
    let timed_out_batch = capped
        .eval_batch(
            &["@artifacts.put {data: \"timed-out-batch-secret\"}".into()],
            CellBudget {
                wall_ms: 0,
                output_bytes: 64,
            },
        )
        .await
        .unwrap();
    assert!(
        timed_out_batch[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("TimeoutError"))
    );
    {
        let events = events.0.lock().unwrap();
        let encoded = serde_json::to_string(&*events).unwrap();
        assert!(!encoded.contains("preflight-secret"), "{encoded}");
        assert!(!encoded.contains("cancelled-batch-secret"), "{encoded}");
        assert!(!encoded.contains("timed-out-batch-secret"), "{encoded}");
        assert_eq!(
            events
                .iter()
                .filter(|(kind, payload)| {
                    kind == "cell_result"
                        && matches!(payload["status"].as_str(), Some("cancelled" | "timed_out"))
                })
                .count(),
            4,
            "{events:#?}"
        );
        assert!(
            events
                .iter()
                .filter(|(kind, _)| kind == "cell_start")
                .all(|(_, payload)| payload["sourcePreview"] == "[redacted]")
        );
    }
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
    let failed_error = failed.error.unwrap();
    assert!(failed_error.len() <= 4, "{failed_error}");
    assert!(failed_error.starts_with("Reso"), "{failed_error}");
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
async fn model_facing_runtime_errors_respect_the_cell_output_budget() {
    let mut session = TmSandbox::new(TmSandboxOptions::default())
        .open(SessionConfig::default())
        .await
        .unwrap();
    let source = format!("unknown_{}", "x".repeat(4_096));
    let budget = CellBudget {
        wall_ms: 1_000,
        output_bytes: 32,
    };

    let single = session.eval(&source, budget).await.unwrap();
    let single_error = single.error.unwrap();
    assert!(single_error.len() <= budget.output_bytes, "{single_error}");

    let batch = session.eval_batch(&[source], budget).await.unwrap();
    let batch_error = batch[0].error.as_deref().unwrap();
    assert!(batch_error.len() <= budget.output_bytes, "{batch_error}");
}

#[tokio::test(flavor = "current_thread")]
async fn source_parse_value_and_cumulative_output_budgets_fail_closed() {
    let options = TmSandboxOptions {
        limits: RuntimeLimits {
            source_bytes: 1_024,
            syntax_nodes: 64,
            parse_depth: 16,
            value_bytes: 256,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    };
    let mut session = TmSandbox::new(options)
        .open(SessionConfig::default())
        .await
        .unwrap();

    let oversized_source = format!("\"{}\"", "x".repeat(1_025));
    let source_error = session
        .eval(&oversized_source, CellBudget::default())
        .await
        .unwrap();
    assert!(source_error.error.unwrap().contains("source budget"));

    let token_heavy = std::iter::repeat_n("x", 100).collect::<Vec<_>>().join(" ");
    let token_error = session
        .eval(&token_heavy, CellBudget::default())
        .await
        .unwrap();
    assert!(token_error.error.unwrap().contains("syntax budget"));

    let nested = format!("{}1{}", "(".repeat(24), ")".repeat(24));
    let depth_error = session.eval(&nested, CellBudget::default()).await.unwrap();
    assert!(depth_error.error.unwrap().contains("nesting budget"));

    let interpolated = format!("\"#{{{}1{}}}\"", "(".repeat(24), ")".repeat(24));
    let interpolation_error = session
        .eval(&interpolated, CellBudget::default())
        .await
        .unwrap();
    assert!(
        interpolation_error
            .error
            .unwrap()
            .contains("nesting budget")
    );

    let growth = session
        .eval(
            "let x = \"12345678\"; let x = x + x; let x = x + x; let x = x + x; let x = x + x; let x = x + x; let x = x + x",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(growth.error.unwrap().contains("intermediate value budget"));

    let cumulative = session
        .eval(
            "print \"123456\"; \"abcdef\"",
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 10,
            },
        )
        .await
        .unwrap();
    let cumulative_error = cumulative.error.as_deref().unwrap();
    assert!(cumulative_error.len() <= 10, "{cumulative_error}");
    assert!(
        cumulative_error.starts_with("Resource"),
        "{cumulative_error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn retained_environment_and_high_cardinality_values_are_bounded_atomically() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            // Keep one candidate value below the per-value ceiling so this specifically exercises
            // the lower aggregate retained-environment ceiling.
            value_bytes: 256 * 1024,
            environment_bytes: 32 * 1024,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let kept = session
        .eval("let kept = 7", CellBudget::default())
        .await
        .unwrap();
    assert!(kept.error.is_none(), "{kept:?}");
    let oversized = format!(
        "let rejected = {}",
        serde_json::to_string(&"x".repeat(40_000)).unwrap()
    );
    let rejected = session
        .eval(&oversized, CellBudget::default())
        .await
        .unwrap();
    assert!(
        rejected
            .error
            .as_deref()
            .is_some_and(|error| error.contains("environment budget")),
        "{rejected:?}"
    );
    let prior = session.eval("kept", CellBudget::default()).await.unwrap();
    assert_eq!(prior.result, Some(json!(7)), "{prior:?}");
    let absent = session
        .eval("rejected", CellBudget::default())
        .await
        .unwrap();
    assert!(absent.error.unwrap().contains("unbound name rejected"));

    let mut value_session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            value_bytes: 8 * 1024,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let high_cardinality_source = serde_json::to_string(&"\n".repeat(256)).unwrap();
    let high_cardinality = value_session
        .eval(
            &format!("{high_cardinality_source} |> lines"),
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(
        high_cardinality
            .error
            .unwrap()
            .contains("intermediate value budget")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cumulative_closure_capture_budget_rejects_quadratic_environment_growth() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            environment_bytes: 32 * 1024,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let source = (0..128)
        .map(|index| format!("fun f{index} x = x"))
        .collect::<Vec<_>>()
        .join("; ");
    let output = session.eval(&source, CellBudget::default()).await.unwrap();
    assert!(output.error.unwrap().contains("environment budget"));
}

#[tokio::test(flavor = "current_thread")]
async fn repeated_large_lambda_bodies_count_toward_value_budget() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            value_bytes: 256 * 1024,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let body = serde_json::to_string(&"x".repeat(16 * 1024)).unwrap();
    let inputs = (0..24)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!("let make = fun _ -> fun _ -> {body}; [{inputs}] |> map make");

    let output = session.eval(&source, CellBudget::default()).await.unwrap();

    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("intermediate value budget")),
        "{output:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn many_short_bindings_are_checked_once_at_commit_within_the_wall_budget() {
    let mut options = TmSandboxOptions::default();
    options.limits.source_bytes = 512 * 1024;
    let mut session = TmSandbox::new(options)
        .open(SessionConfig::default())
        .await
        .unwrap();
    let mut source = String::new();
    for index in 0..6_000 {
        source.push_str(&format!("let value_{index} = {index};"));
    }
    source.push_str("value_5999");

    let output = session
        .eval(
            &source,
            CellBudget {
                wall_ms: 3_000,
                output_bytes: CellBudget::default().output_bytes,
            },
        )
        .await
        .unwrap();

    assert_eq!(output.result, Some(json!(5_999)), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn failed_function_pattern_restores_the_callers_environment_before_handle() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            steps: 200,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let pattern = (0..128)
        .map(|index| format!("item_{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let values = serde_json::to_string(&"item\n".repeat(128)).unwrap();
    let source = format!(
        "fun fail [{pattern}] = 0; let caller = 41; let values = {values} |> lines; handle fail values with error {{ | ResourceLimitError _ -> caller }}"
    );

    let output = session.eval(&source, CellBudget::default()).await.unwrap();

    assert_eq!(output.result, Some(json!(41)), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_event_persistence_failures_are_fatal_sandbox_errors() {
    for (event_type, source) in [
        ("cell_result", "1"),
        ("cell_start", "("),
        ("binding_committed", "let persisted = 1"),
    ] {
        let mut session = TmSandbox::new(TmSandboxOptions {
            host_event_sink: Arc::new(FailOnceOnEvent::new(event_type)),
            ..TmSandboxOptions::default()
        })
        .open(SessionConfig::default())
        .await
        .unwrap();

        let error = session
            .eval(source, CellBudget::default())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("persistence failure"),
            "{event_type}: {error:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn deep_pure_recursion_returns_a_resource_limit_error() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        limits: RuntimeLimits {
            runtime_depth: 32,
            ..RuntimeLimits::default()
        },
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let output = session
        .eval(
            "fun recurse value = recurse value; recurse 0",
            CellBudget::default(),
        )
        .await
        .unwrap();
    let error = output.error.as_deref().unwrap_or_default();

    assert!(error.contains("ResourceLimitError"), "{output:?}");
    assert!(error.contains("runtime nesting budget"), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn parse_and_check_failures_emit_redacted_paired_cell_events() {
    let events = Arc::new(Events::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    for source in ["let = \"parse-secret\"", "unknown_check_secret"] {
        let output = session.eval(source, CellBudget::default()).await.unwrap();
        assert!(output.error.is_some(), "{source}: {output:?}");
    }
    let events = events.0.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_start")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_result")
            .count(),
        2
    );
    let encoded = serde_json::to_string(&*events).unwrap();
    assert!(!encoded.contains("parse-secret"), "{encoded}");
    assert!(!encoded.contains("unknown_check_secret"), "{encoded}");
}

#[tokio::test(flavor = "current_thread")]
async fn collection_builders_enforce_value_budget_incrementally() {
    let repeated = "x".repeat(50);
    let strings = ["a".repeat(30), "b".repeat(30), "c".repeat(30)];
    let controls = "\\u0001".repeat(40);
    let sources = [
        format!("let key = \"{repeated}\"; [1, 2, 3] |> map (fun value -> key)"),
        format!(
            "[\"{}\", \"{}\", \"{}\"] |> sort_by (fun value -> value) asc",
            strings[0], strings[1], strings[2]
        ),
        format!(
            "[\"{}\", \"{}\", \"{}\"] |> group_by (fun value -> value)",
            strings[0], strings[1], strings[2]
        ),
        format!("[\"{controls}\"] |> group_by (fun value -> value)"),
    ];
    for source in sources {
        let mut session = TmSandbox::new(TmSandboxOptions {
            limits: RuntimeLimits {
                value_bytes: 128,
                ..RuntimeLimits::default()
            },
            ..TmSandboxOptions::default()
        })
        .open(SessionConfig::default())
        .await
        .unwrap();
        let output = session.eval(&source, CellBudget::default()).await.unwrap();
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains("intermediate value budget")),
            "{source}: {output:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fabricated_artifact_envelopes_do_not_bypass_result_budget() {
    let fake = format!(
        "{{truncated: true, artifact: {{uri: \"artifact://7\", id: \"7\", kind: \"text\", mime: \"text/plain\", title: null, size_bytes: 256, preview: \"x\"}}, payload: \"{}\"}}",
        "x".repeat(256)
    );
    let mut session = TmSandbox::new(TmSandboxOptions::default())
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = session
        .eval(
            &fake,
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 64,
            },
        )
        .await
        .unwrap();
    assert!(output.error.unwrap().contains("result/output budget"));
}

#[tokio::test(flavor = "current_thread")]
async fn effect_previews_share_the_cell_output_budget() {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(ParallelProbe::new(active, peak)));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("test.probe"),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let output = session
        .eval(
            "@test.probe {value: 1}; @test.probe {value: 2}; @test.probe {value: 3}",
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 50,
            },
        )
        .await
        .unwrap();
    assert!(output.error.unwrap().contains("effect/output budget"));
}

#[tokio::test(flavor = "current_thread")]
async fn sensitive_cells_redact_source_arguments_results_and_errors() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let approval_actions = Arc::new(Mutex::new(Vec::new()));
    let mut session = sandbox_with_policy(
        Arc::clone(&events),
        calls,
        Arc::new(RecordingApprove(Arc::clone(&approval_actions))),
    )
    .open(SessionConfig::default())
    .await
    .unwrap();
    let output = session
        .eval(
            "@fs.patch {patch: \"secret-source-value\"} |> display {kind: \"json\"}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(output.error.is_none(), "{output:?}");
    assert_eq!(&*approval_actions.lock().unwrap(), &["fs.patch"]);

    let events = events.0.lock().unwrap();
    let encoded = serde_json::to_string(&*events).unwrap();
    assert!(!encoded.contains("secret-source-value"), "{encoded}");
    assert!(events.iter().any(|(kind, payload)| {
        kind == "cell_start" && payload["sourcePreview"] == "[redacted]"
    }));
    assert!(events.iter().any(|(kind, payload)| {
        kind == "effect_result" && payload["resultPreview"] == "[redacted]"
    }));
    assert!(events.iter().any(|(kind, payload)| {
        kind == "effect_suspended" && payload["action"] == "[redacted]"
    }));
    assert!(events.iter().any(|(kind, payload)| {
        kind == "display" && payload["spec"] == "[redacted]" && payload["value"] == "[redacted]"
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn durable_previews_stay_content_blind_across_persistent_bindings() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(Arc::clone(&events), calls)
        .open(SessionConfig::default())
        .await
        .unwrap();

    let authority = session
        .eval(
            "let persisted = @fs.patch {patch: \"cross-cell-secret\"}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(authority.error.is_none(), "{authority:?}");
    let pure = session
        .eval(
            "persisted |> display {kind: \"json\"}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(pure.error.is_none(), "{pure:?}");

    let events = events.0.lock().unwrap();
    let encoded = serde_json::to_string(&*events).unwrap();
    assert!(!encoded.contains("cross-cell-secret"), "{encoded}");
    assert!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_start")
            .all(|(_, payload)| payload["sourcePreview"] == "[redacted]")
    );
    assert!(events.iter().any(|(kind, payload)| {
        kind == "display" && payload["spec"] == "[redacted]" && payload["value"] == "[redacted]"
    }));
    let binding = events
        .iter()
        .find(|(kind, _)| kind == "binding_committed")
        .expect("authority cell commits its binding");
    assert_eq!(binding.1["bindingCount"], 1);
    assert_eq!(binding.1["namesRedacted"], true);
    assert!(binding.1.get("names").is_none());
    assert!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_result")
            .all(|(_, payload)| {
                payload
                    .get("resultPreview")
                    .is_none_or(|value| value == "[redacted]")
            })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn division_by_zero_and_rethrow_preserve_error_identity() {
    let mut session = TmSandbox::new(TmSandboxOptions::default())
        .open(SessionConfig::default())
        .await
        .unwrap();
    let direct = session
        .eval(
            "handle 1 / 0 with error { | DivisionByZero _ -> 41 }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(direct.result, Some(json!(41)), "{direct:?}");

    let rethrown = session
        .eval(
            "handle do { handle 1 / 0 with error { | DivisionByZero _ -> rethrow null } } with error { | DivisionByZero _ -> 42 }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(rethrown.result, Some(json!(42)), "{rethrown:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn mid_effect_cancellation_emits_one_terminal_effect_and_cell_result() {
    let events = Arc::new(Events::default());
    let started = Arc::new(tokio::sync::Notify::new());
    let cancellation = Arc::new(TriggerCancellation::default());
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(BlockingEffect::new(Arc::clone(&started))));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("test.block"),
        host_event_sink: events.clone(),
        cancellation: Some(cancellation.clone()),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let cancel = async {
        started.notified().await;
        cancellation.cancel();
    };
    let (output, ()) = tokio::join!(
        session.eval("@test.block null", CellBudget::default()),
        cancel
    );
    let output = output.unwrap();
    assert!(output.error.unwrap().contains("cancelled"));

    let events = events.0.lock().unwrap();
    let effect_results = events
        .iter()
        .filter(|(kind, _)| kind == "effect_result")
        .collect::<Vec<_>>();
    assert_eq!(effect_results.len(), 1, "{events:?}");
    assert_eq!(effect_results[0].1["status"], "cancelled");
    let cell_results = events
        .iter()
        .filter(|(kind, _)| kind == "cell_result")
        .collect::<Vec<_>>();
    assert_eq!(cell_results.len(), 1, "{events:?}");
    assert_eq!(cell_results[0].1["status"], "cancelled");
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_effect_start_sink_still_emits_one_terminal_result() {
    let events = Arc::new(YieldingStartEvents::default());
    let cancellation = Arc::new(TriggerCancellation::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(Patch::new(Arc::clone(&calls))));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("fs.patch"),
        host_event_sink: events.clone(),
        cancellation: Some(cancellation.clone()),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let cancel = async {
        events.effect_start_seen.notified().await;
        cancellation.cancel();
    };
    let (output, ()) = tokio::join!(
        session.eval("@fs.patch {patch: \"x\"}", CellBudget::default()),
        cancel
    );
    let output = output.unwrap();
    assert!(output.error.unwrap().contains("cancelled"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let events = events.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "effect_result")
            .count(),
        1,
        "{events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_result")
            .count(),
        1,
        "{events:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_cell_start_sink_still_emits_one_paired_terminal() {
    let events = Arc::new(YieldingCellStartEvents::default());
    let cancellation = Arc::new(TriggerCancellation::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        cancellation: Some(cancellation.clone()),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let cancel = async {
        events.cell_start_seen.notified().await;
        cancellation.cancel();
    };
    let (output, ()) = tokio::join!(session.eval("1", CellBudget::default()), cancel);
    let output = output.unwrap();
    assert!(output.error.unwrap().contains("cancelled"));

    let events = events.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_start")
            .count(),
        1,
        "{events:?}"
    );
    let cell_results = events
        .iter()
        .filter(|(kind, _)| kind == "cell_result")
        .collect::<Vec<_>>();
    assert_eq!(cell_results.len(), 1, "{events:?}");
    assert_eq!(cell_results[0].1["status"], "cancelled");
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_binding_persistence_finishes_the_selected_commit() {
    let events = Arc::new(YieldingBindingEvents::default());
    let cancellation = Arc::new(TriggerCancellation::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        cancellation: Some(cancellation.clone()),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let cancel = async {
        events.binding_seen.notified().await;
        cancellation.cancel();
        events.release.notify_one();
    };
    let (output, ()) = tokio::join!(
        session.eval("let committed_during_cancel = 7", CellBudget::default()),
        cancel
    );
    let output = output.unwrap();
    assert!(output.error.is_none(), "{output:?}");
    assert_eq!(output.result, Some(json!(7)));

    cancellation.reset();
    let read = session
        .eval("committed_during_cancel", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(read.result, Some(json!(7)), "{read:?}");
    let events = events.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(kind, _)| kind == "binding_committed")
            .count(),
        2,
        "one structural binding event is emitted for each successful cell: {events:?}"
    );
    assert!(
        events
            .iter()
            .filter(|(kind, _)| kind == "cell_result")
            .all(|(_, payload)| payload["status"] == "completed")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_after_durable_binding_event_keeps_the_installed_binding() {
    let events = Arc::new(DurableThenStallingBindingEvents::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    {
        let evaluation = session.eval("let installed_before_await = 7", CellBudget::default());
        tokio::pin!(evaluation);
        tokio::select! {
            _ = events.binding_seen.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                panic!("binding event was not durably recorded")
            }
            result = &mut evaluation => {
                panic!("binding persistence unexpectedly completed: {result:?}")
            }
        }
    }

    let read = session
        .eval("installed_before_await", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(read.result, Some(json!(7)), "{read:?}");
    assert!(
        events
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|(kind, payload)| { kind == "binding_committed" && payload["bindingCount"] == 1 })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn state_writing_batch_cancellation_cannot_split_event_from_base_commit() {
    let events = Arc::new(YieldingBindingEvents::default());
    let cancellation = Arc::new(TriggerCancellation::default());
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_event_sink: events.clone(),
        cancellation: Some(cancellation.clone()),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let cancel = async {
        events.binding_seen.notified().await;
        cancellation.cancel();
        events.release.notify_one();
    };
    let cells = vec!["let batch_commit_during_cancel = 9".into()];
    let (outputs, ()) = tokio::join!(session.eval_batch(&cells, CellBudget::default()), cancel);
    let outputs = outputs.unwrap();
    assert!(outputs[0].error.is_none(), "{outputs:?}");

    cancellation.reset();
    let read = session
        .eval("batch_commit_during_cancel", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(read.result, Some(json!(9)), "{read:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn mid_effect_timeout_emits_one_terminal_effect_and_cell_result() {
    let events = Arc::new(Events::default());
    let started = Arc::new(tokio::sync::Notify::new());
    let mut registry = HostRegistry::new();
    registry.register(Arc::new(BlockingEffect::new(started)));
    let mut session = TmSandbox::new(TmSandboxOptions {
        host_registry: registry,
        grants: CapabilityGrants::default().allow("test.block"),
        host_event_sink: events.clone(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    let output = session
        .eval(
            "@test.block null",
            CellBudget {
                wall_ms: 10,
                output_bytes: 1024,
            },
        )
        .await
        .unwrap();
    assert!(output.error.unwrap().contains("wall-clock budget"));

    let events = events.0.lock().unwrap();
    let effect_results = events
        .iter()
        .filter(|(kind, _)| kind == "effect_result")
        .collect::<Vec<_>>();
    assert_eq!(effect_results.len(), 1, "{events:?}");
    assert_eq!(effect_results[0].1["status"], "timed_out");
    let cell_results = events
        .iter()
        .filter(|(kind, _)| kind == "cell_result")
        .collect::<Vec<_>>();
    assert_eq!(cell_results.len(), 1, "{events:?}");
    assert_eq!(cell_results[0].1["status"], "timed_out");
}

#[tokio::test(flavor = "current_thread")]
async fn host_spill_envelopes_must_fit_the_cell_budget() {
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

    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("output budget")),
        "{output:?}"
    );

    let compact = session
        .eval(
            "@test.spill null",
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 2_048,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        compact.result.as_ref().unwrap()["artifact"]["uri"],
        json!("artifact://7"),
        "{compact:?}"
    );
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
    let display_error = display.error.as_deref().unwrap();
    assert!(display_error.len() <= 32, "{display_error}");
    assert!(
        display_error.starts_with("ResourceLimitError"),
        "{display_error}"
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

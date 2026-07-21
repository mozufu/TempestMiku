use std::{
    collections::BTreeMap,
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
pub(super) struct Events(pub(super) Mutex<Vec<(String, Value)>>);

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
pub(super) struct YieldingStartEvents {
    events: Mutex<Vec<(String, Value)>>,
    effect_start_seen: tokio::sync::Notify,
    paused: AtomicBool,
}

#[derive(Default)]
pub(super) struct YieldingCellStartEvents {
    events: Mutex<Vec<(String, Value)>>,
    cell_start_seen: tokio::sync::Notify,
    paused: AtomicBool,
}

#[derive(Default)]
pub(super) struct YieldingBindingEvents {
    events: Mutex<Vec<(String, Value)>>,
    binding_seen: tokio::sync::Notify,
    release: tokio::sync::Notify,
    paused: AtomicBool,
}

#[derive(Default)]
pub(super) struct StallingSecondCellResult {
    seen: AtomicUsize,
}

pub(super) struct FailOnceOnEvent {
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
pub(super) struct DurableThenStallingBindingEvents {
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

pub(super) struct Approve;

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

pub(super) struct RecordingApprove(pub(super) Arc<Mutex<Vec<String>>>);

#[async_trait]
impl ApprovalPolicy for RecordingApprove {
    async fn request(&self, action: &str, _timeout: Duration) -> tm_host::Result<ApprovalDecision> {
        self.0.lock().unwrap().push(action.to_string());
        Ok(ApprovalDecision::Approved)
    }
}

pub(super) struct Deny;
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

pub(super) struct ApprovalFailure;

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

pub(super) struct Cancelled;
impl CancellationToken for Cancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

#[derive(Default)]
pub(super) struct TriggerCancellation {
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

pub(super) struct WorkspaceResource;

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

pub(super) struct SecretResource;

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

pub(super) struct Patch {
    calls: Arc<AtomicUsize>,
    docs: ToolDocs,
}

pub(super) struct ProductionHttpRequest {
    docs: ToolDocs,
}

impl ProductionHttpRequest {
    pub(super) fn new() -> Self {
        Self {
            docs: ToolDocs {
                name: "http.request".into(),
                namespace: "http".into(),
                summary: "Production-bound HTTP test handler".into(),
                description: None,
                signature: "http.request(args)".into(),
                args_schema: json!({"type":"object"}),
                result_schema: Some(json!({"type":"object"})),
                examples: vec![],
                errors: vec![],
                grants: vec![GrantDoc {
                    kind: "capability".into(),
                    description: "http.request".into(),
                }],
                sensitive: true,
                approval: "none".into(),
                since: "test".into(),
                stability: "stable".into(),
            },
        }
    }
}

#[async_trait]
impl HostFn for ProductionHttpRequest {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        Ok(json!({"source": "production", "args": args}))
    }
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

pub(super) struct Spill {
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

pub(super) struct ParallelProbe {
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    docs: ToolDocs,
}

pub(super) struct BlockingEffect {
    started: Arc<tokio::sync::Notify>,
    docs: ToolDocs,
}

pub(super) struct FailAfterStart {
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

pub(super) fn sandbox(events: Arc<Events>, calls: Arc<AtomicUsize>) -> TmSandbox {
    sandbox_with_policy(events, calls, Arc::new(Approve))
}

pub(super) fn sandbox_with_policy(
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

#[path = "adapters.rs"]
mod adapters;
#[path = "batch.rs"]
mod batch;
#[path = "cancellation.rs"]
mod cancellation;
#[path = "limits.rs"]
mod limits;
#[path = "parallel.rs"]
mod parallel;
#[path = "persistence.rs"]
mod persistence;
#[path = "semantics.rs"]
mod semantics;
#[path = "state.rs"]
mod state;

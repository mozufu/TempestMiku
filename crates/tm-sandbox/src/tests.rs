use std::{collections::BTreeMap, fs, sync::Arc};

use serde_json::Value;
use tm_artifacts::ArtifactStore;
use tm_core::{CellBudget, Sandbox, SessionConfig};
use tm_drive::InMemoryDriveStore;
use tm_host::{ApprovalDecision, ApprovalPolicy, FsMode, LinkedFolderConfig, LinkedFolders};

use crate::{DenoSandbox, DenoSandboxOptions, StubSandbox};

fn p0_sandbox(root: &std::path::Path, artifact_root: &std::path::Path) -> DenoSandbox {
    p0_sandbox_with_approval(
        root,
        artifact_root,
        DenoSandboxOptions::default().approval_policy,
    )
}

fn p0_sandbox_with_approval(
    root: &std::path::Path,
    artifact_root: &std::path::Path,
    approval_policy: Arc<dyn ApprovalPolicy>,
) -> DenoSandbox {
    let drive_artifacts = ArtifactStore::open(artifact_root, "drive").unwrap();
    DenoSandbox::new(DenoSandboxOptions {
        artifact_root: artifact_root.to_path_buf(),
        linked_folders: Some(
            LinkedFolders::from_configs(vec![LinkedFolderConfig {
                name: "tempestmiku".to_string(),
                path: root.to_path_buf(),
                mode: FsMode::Rw,
                commands: vec!["cargo".to_string()],
                safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
            }])
            .unwrap(),
        ),
        drive_store: Some(InMemoryDriveStore::new(drive_artifacts)),
        approval_policy,
        ..DenoSandboxOptions::default()
    })
}

struct StaticApproval(ApprovalDecision);

#[async_trait::async_trait]
impl ApprovalPolicy for StaticApproval {
    async fn request(
        &self,
        _action: &str,
        _timeout: std::time::Duration,
    ) -> tm_host::Result<ApprovalDecision> {
        Ok(self.0)
    }
}

const SDK_CATALOG_NAMESPACE_METHODS: &[(&str, &str)] = &[
    ("tools.search", "tools"),
    ("tools.docs", "tools"),
    ("tools.call", "tools"),
    ("resources.read", "resources"),
    ("resources.preview", "resources"),
    ("resources.list", "resources"),
    ("artifacts.put", "artifacts"),
    ("artifacts.get", "artifacts"),
    ("artifacts.slice", "artifacts"),
    ("artifacts.list", "artifacts"),
    ("fs.read", "fs"),
    ("fs.write", "fs"),
    ("fs.ls", "fs"),
    ("fs.find", "fs"),
    ("code.search", "code"),
    ("code.edit", "code"),
    ("proc.run", "proc"),
    ("http.get", "http"),
    ("drive.put", "drive"),
    ("drive.get", "drive"),
    ("drive.ls", "drive"),
    ("drive.move", "drive"),
    ("drive.search", "drive"),
    ("drive.tag", "drive"),
    ("drive.link", "drive"),
    ("drive.unlink", "drive"),
    ("drive.organize", "drive"),
];

#[tokio::test]
async fn stub_echoes_code_and_persists_cell_count() {
    let sandbox = StubSandbox;
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();

    let out = session.eval("1 + 1", CellBudget::default()).await.unwrap();
    assert_eq!(out.result, Some(Value::String("1 + 1".into())));
    assert!(out.stdout.contains("cell #1"));

    let out2 = session.eval("2 + 2", CellBudget::default()).await.unwrap();
    assert!(out2.stdout.contains("cell #2"));

    session.reset().await.unwrap();
    let out3 = session.eval("3", CellBudget::default()).await.unwrap();
    assert!(out3.stdout.contains("cell #1"));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_executes_typescript_cell() {
    let sandbox = DenoSandbox::default();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval(
            "interface Box<T> { value: T }\n\
                 type Label = string;\n\
                 const box: Box<number> = { value: 41 };\n\
                 const label = 'x' as Label;\n\
                 box.value + label.length",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.result, Some(Value::Number(42.into())));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_parse_errors_are_cell_errors() {
    let sandbox = DenoSandbox::default();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval("const broken: = ;", CellBudget::default())
        .await
        .unwrap();
    assert!(out.error.is_some());

    let after = session.eval("1 + 1", CellBudget::default()).await.unwrap();
    assert_eq!(after.result, Some(Value::Number(2.into())));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_executes_multiline_cells() {
    let sandbox = DenoSandbox::default();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval(
            "const x: number = 1;\nconst y: number = 2;\nx + y",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.result, Some(Value::Number(3.into())));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_persists_state_and_resets() {
    let sandbox = DenoSandbox::default();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    session
        .eval(
            "let count: number = 1;\n\
                 function add_one(n: number): number { return n + 1; }\n\
                 0",
            CellBudget::default(),
        )
        .await
        .unwrap();
    let out = session
        .eval("add_one(count)", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(out.result, Some(Value::Number(2.into())));
    session.reset().await.unwrap();
    let out = session
        .eval("add_one(1)", CellBudget::default())
        .await
        .unwrap();
    assert!(out.error.is_some());
    let out = session.eval("count", CellBudget::default()).await.unwrap();
    assert!(out.error.is_some());
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_timeout_is_structured_error() {
    let sandbox = DenoSandbox::default();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval(
            "while (true) {}",
            CellBudget {
                wall_ms: 10,
                ..CellBudget::default()
            },
        )
        .await
        .unwrap();
    assert!(out.error.unwrap().contains("TimeoutError"));
    let after = session.eval("1 + 1", CellBudget::default()).await.unwrap();
    assert_eq!(after.result, Some(Value::Number(2.into())));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_captures_print_and_display() {
    let sandbox = DenoSandbox::default();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval(
            "print('hello', 1); display({ ok: true }); 7",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(out.stdout.contains("hello 1"));
    assert!(out.stdout.contains("display"));
    assert_eq!(out.result, Some(Value::Number(7.into())));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_blocks_ambient_raw_apis() {
    let sandbox = DenoSandbox::default();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval(
            "({ deno: typeof Deno, fetch: typeof fetch, process: typeof process })",
            CellBudget::default(),
        )
        .await
        .unwrap();
    let result = out.result.unwrap();
    assert_eq!(result["deno"], Value::String("undefined".into()));
    assert_eq!(result["fetch"], Value::String("undefined".into()));
    assert_eq!(result["process"], Value::String("undefined".into()));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_spills_large_output_to_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let sandbox = DenoSandbox::new(DenoSandboxOptions {
        artifact_root: dir.path().to_path_buf(),
        ..DenoSandboxOptions::default()
    });
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval(
            "print('x'.repeat(100));",
            CellBudget {
                output_bytes: 20,
                ..CellBudget::default()
            },
        )
        .await
        .unwrap();
    assert!(out.stdout.contains("artifact://"));
    assert!(out.stdout.contains("output truncated to 20 bytes"));
    assert!(!out.stdout.contains(&"x".repeat(100)));
    let fetched = session
        .eval(
            "const first = artifacts.list()[0].uri; await artifacts.get(first)",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        fetched.result.unwrap()["content"].as_str().unwrap().len(),
        100
    );
    let listed = session
        .eval("artifacts.list()[0].sizeBytes", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(listed.result, Some(Value::Number(100.into())));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_drive_put_read_and_resource_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    fs::write(root.path().join("README.md"), "TempestMiku\n").unwrap();
    let sandbox = p0_sandbox_with_approval(
        root.path(),
        artifacts.path(),
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
    );
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();

    let out = session
        .eval(
            "const filed = await drive.put('# P5 Note\\nhello', { auto: true, project: 'TempestMiku' });\n\
             const viaDrive = await drive.get(filed.uri, { selector: '2-2' });\n\
             const viaResource = await resources.read(filed.uri, '1-1');\n\
             ({ uri: filed.uri, path: filed.entry.path, viaDrive: viaDrive.content, viaResource: viaResource.content })",
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(out.error.is_none(), "drive eval failed: {:?}", out.error);
    let result = out.result.unwrap();
    assert_eq!(result["viaDrive"], Value::String("hello".to_string()));
    assert_eq!(
        result["viaResource"],
        Value::String("# P5 Note".to_string())
    );
    assert!(
        result["uri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with("drive://"))
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_drive_link_registers_linked_resource_after_approval() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("README.md"),
        "TempestMiku linked project\n",
    )
    .unwrap();
    let sandbox = p0_sandbox_with_approval(
        root.path(),
        artifacts.path(),
        Arc::new(StaticApproval(ApprovalDecision::Approved)),
    );
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let host_path = serde_json::to_string(root.path().to_str().unwrap()).unwrap();
    let code = format!(
        "const plan = await drive.link({host_path}, 'rw', {{ project: 'Approved Project' }});\n\
         const read = await resources.read('linked://approved-project/README.md');\n\
         await fs.write('approved-project:tmp.txt', 'allowed before attenuation');\n\
         const narrowed = await drive.link({host_path}, 'ro', {{ project: 'Approved Project' }});\n\
         const writeAfterNarrow = await fs.write('approved-project:after.txt', 'blocked').catch(err => ({{ name: err.name }}));\n\
         const revoked = await drive.unlink('linked://approved-project/');\n\
         const readAfterRevoke = await resources.read('linked://approved-project/README.md').catch(err => ({{ name: err.name }}));\n\
         ({{ linkedUri: plan.linkedUri, memoryScope: plan.memoryScope, narrowedMode: narrowed.mode, revokedAt: typeof revoked.revokedAt, content: read.content, writeAfterNarrow, readAfterRevoke }})"
    );

    let out = session
        .eval(
            &code,
            CellBudget {
                wall_ms: 30_000,
                output_bytes: 50_000,
            },
        )
        .await
        .unwrap();

    assert!(
        out.error.is_none(),
        "drive link eval failed: {:?}",
        out.error
    );
    let result = out.result.unwrap();
    assert_eq!(
        result["linkedUri"],
        Value::String("linked://approved-project/".to_string())
    );
    assert_eq!(
        result["memoryScope"],
        Value::String("project:approved-project".to_string())
    );
    assert_eq!(
        result["content"],
        Value::String("TempestMiku linked project\n".to_string())
    );
    assert_eq!(result["narrowedMode"], Value::String("ro".to_string()));
    assert_eq!(result["revokedAt"], Value::String("string".to_string()));
    assert_eq!(
        result["writeAfterNarrow"]["name"],
        Value::String("CapabilityDeniedError".to_string())
    );
    assert_eq!(
        result["readAfterRevoke"]["name"],
        Value::String("InvalidPathError".to_string())
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_research_drive_summarizes_bounded_local_corpus() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    fs::write(root.path().join("README.md"), "TempestMiku\n").unwrap();
    let sandbox = p0_sandbox(root.path(), artifacts.path());
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();

    let out = session
        .eval(
            "await drive.put('# Approval Notes\\nManual approval gates drive writes.\\nExtra private body that should stay out of corpus.', { project: 'TempestMiku', suggestedPath: 'projects/tempestmiku/notes/approval.md' });\n\
             await drive.put('# Approval Checklist\\nUse bounded citations for every local drive digest.', { project: 'TempestMiku', suggestedPath: 'projects/tempestmiku/notes/checklist.md' });\n\
             await drive.put('# Other Notes\\nUnrelated project material.', { project: 'Other', suggestedPath: 'projects/other/notes/other.md' });\n\
             const result = await research.drive('approval', { project: 'TempestMiku', maxDocs: 2, maxSnippets: 1, maxWorkers: 0, maxBytesPerDoc: 80, maxDigestBytes: 80 });\n\
             let seenTasks = [];\n\
             globalThis.agents = { parallel: async (tasks) => { seenTasks = tasks; return tasks.map((task, index) => ({ actorId: `Scripted-${index}`, summary: `scripted ${index}: ${task.task.includes('Approval')}`, artifactUri: `artifact://${index}`, historyUri: `history://Scripted-${index}` })); } };\n\
             const agentResult = await research.drive('approval', { project: 'TempestMiku', maxDocs: 2, maxSnippets: 2, maxWorkers: 1, maxBytesPerDoc: 80, maxDigestBytes: 80, workerTimeoutMs: 250, totalTimeoutMs: 200 });\n\
             ({ corpus: result.corpus.length, digests: result.digests.length, citations: result.citations.length, answer: result.answer, leakedContent: Object.prototype.hasOwnProperty.call(result.corpus[0], 'content'), uri: result.corpus[0].uri, sourceKind: result.corpus[0].sourceKind, citationKind: result.citations[0].sourceKind, hash: result.corpus[0].contentHash, budget: result.budget, workerFailures: result.workerFailures.length, agentDigests: agentResult.digests.length, agentDocs: agentResult.budget.agentDocs, agentDocsCompleted: agentResult.budget.agentDocsCompleted, agentFailures: agentResult.workerFailures.length, firstActor: agentResult.digests[0].actorId, secondActor: agentResult.digests[1].actorId, firstHistory: agentResult.digests[0].historyUri, agentCitationKind: agentResult.citations[0].sourceKind, agentWorkerTimeout: agentResult.budget.workerTimeoutMs, agentTotalTimeout: agentResult.budget.totalTimeoutMs, childTimeout: seenTasks[0]?.timeoutMs, childWallMs: seenTasks[0]?.budget?.wallMs })",
            CellBudget {
                wall_ms: 30_000,
                output_bytes: 50_000,
            },
        )
        .await
        .unwrap();

    assert!(out.error.is_none(), "research eval failed: {:?}", out.error);
    let result = out.result.unwrap();
    assert_eq!(result["corpus"], Value::Number(1.into()));
    assert_eq!(result["digests"], Value::Number(1.into()));
    assert_eq!(result["citations"], Value::Number(1.into()));
    assert_eq!(result["leakedContent"], Value::Bool(false));
    assert!(result["answer"].as_str().unwrap().contains("drive://"));
    assert!(result["uri"].as_str().unwrap().contains("approval.md"));
    assert_eq!(result["sourceKind"], Value::String("drive".to_string()));
    assert_eq!(result["citationKind"], Value::String("drive".to_string()));
    assert!(!result["hash"].as_str().unwrap().is_empty());
    assert_eq!(result["budget"]["maxDocs"], Value::Number(2.into()));
    assert_eq!(result["budget"]["maxSnippets"], Value::Number(1.into()));
    assert_eq!(result["budget"]["maxWorkers"], Value::Number(0.into()));
    assert_eq!(result["budget"]["maxBytesPerDoc"], Value::Number(80.into()));
    assert_eq!(result["budget"]["maxDigestBytes"], Value::Number(80.into()));
    assert_eq!(result["budget"]["selectedDocs"], Value::Number(1.into()));
    assert_eq!(result["budget"]["agentDocs"], Value::Number(0.into()));
    assert_eq!(result["budget"]["workerFailures"], Value::Number(0.into()));
    assert_eq!(result["workerFailures"], Value::Number(0.into()));
    assert_eq!(result["agentDigests"], Value::Number(2.into()));
    assert_eq!(result["agentDocs"], Value::Number(1.into()));
    assert_eq!(result["agentDocsCompleted"], Value::Number(1.into()));
    assert_eq!(result["agentFailures"], Value::Number(0.into()));
    assert_eq!(result["agentWorkerTimeout"], Value::Number(200.into()));
    assert_eq!(result["agentTotalTimeout"], Value::Number(200.into()));
    assert_eq!(result["childTimeout"], Value::Number(200.into()));
    assert_eq!(result["childWallMs"], Value::Number(200.into()));
    assert_eq!(
        result["firstActor"],
        Value::String("Scripted-0".to_string())
    );
    assert_eq!(result["secondActor"], Value::Null);
    assert_eq!(
        result["firstHistory"],
        Value::String("history://Scripted-0".to_string())
    );
    assert_eq!(
        result["agentCitationKind"],
        Value::String("drive".to_string())
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_research_drive_isolates_child_failure_and_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    fs::write(root.path().join("README.md"), "TempestMiku\n").unwrap();
    let sandbox = p0_sandbox(root.path(), artifacts.path());
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();

    let out = session
        .eval(
            "await drive.put('# Approval Notes\\nManual approval gates drive writes.', { project: 'TempestMiku', suggestedPath: 'projects/tempestmiku/notes/approval.md' });\n\
             await drive.put('# Approval Checklist\\nUse bounded citations for every local drive digest.', { project: 'TempestMiku', suggestedPath: 'projects/tempestmiku/notes/checklist.md' });\n\
             const before = (await drive.ls('projects/tempestmiku/notes', { recursive: true })).map((entry) => entry.uri).sort().join('|');\n\
             globalThis.agents = { parallel: async () => { const err = new Error('actor CancelledOne failed: actor cancelled'); err.name = 'HostCallError'; throw err; } };\n\
             const cancelled = await research.drive('approval', { project: 'TempestMiku', maxDocs: 2, maxSnippets: 2, maxWorkers: 2, maxBytesPerDoc: 80, maxDigestBytes: 80 });\n\
             globalThis.agents = { parallel: async (tasks) => tasks.map((task, index) => index === 0 ? { status: 'failed', actorId: 'FailedOne', reason: 'model timeout' } : { actorId: 'OkTwo', summary: `ok ${index}`, artifactUri: null, historyUri: 'history://OkTwo' }) };\n\
             const partial = await research.drive('approval', { project: 'TempestMiku', maxDocs: 2, maxSnippets: 2, maxWorkers: 2, maxBytesPerDoc: 80, maxDigestBytes: 80 });\n\
             const after = (await drive.ls('projects/tempestmiku/notes', { recursive: true })).map((entry) => entry.uri).sort().join('|');\n\
             ({ unchanged: before === after, cancelledDigests: cancelled.digests.length, cancelledFailures: cancelled.workerFailures.length, cancelledKind: cancelled.workerFailures[0].kind, cancelledActor: cancelled.workerFailures[0].actorId, cancelledPhase: cancelled.workerFailures[0].phase, cancelledCompleted: cancelled.budget.agentDocsCompleted, cancelledBudgetFailures: cancelled.budget.workerFailures, cancelledAnswer: cancelled.answer, partialDigests: partial.digests.length, partialFailures: partial.workerFailures.length, partialKind: partial.workerFailures[0].kind, partialActor: partial.workerFailures[0].actorId, partialUriPresent: partial.workerFailures[0].uri != null, partialOkActor: partial.digests.some((digest) => digest.actorId === 'OkTwo'), partialCompleted: partial.budget.agentDocsCompleted, partialBudgetFailures: partial.budget.workerFailures })",
            CellBudget {
                wall_ms: 30_000,
                output_bytes: 50_000,
            },
        )
        .await
        .unwrap();

    assert!(
        out.error.is_none(),
        "research failure isolation eval failed: {:?}",
        out.error
    );
    let result = out.result.unwrap();
    assert_eq!(result["unchanged"], Value::Bool(true));
    assert_eq!(result["cancelledDigests"], Value::Number(2.into()));
    assert_eq!(result["cancelledFailures"], Value::Number(1.into()));
    assert_eq!(
        result["cancelledKind"],
        Value::String("cancelled".to_string())
    );
    assert_eq!(
        result["cancelledActor"],
        Value::String("CancelledOne".to_string())
    );
    assert_eq!(
        result["cancelledPhase"],
        Value::String("agents.parallel".to_string())
    );
    assert_eq!(result["cancelledCompleted"], Value::Number(0.into()));
    assert_eq!(result["cancelledBudgetFailures"], Value::Number(1.into()));
    assert!(
        result["cancelledAnswer"]
            .as_str()
            .unwrap()
            .contains("drive://")
    );
    assert_eq!(result["partialDigests"], Value::Number(2.into()));
    assert_eq!(result["partialFailures"], Value::Number(1.into()));
    assert_eq!(result["partialKind"], Value::String("timeout".to_string()));
    assert_eq!(
        result["partialActor"],
        Value::String("FailedOne".to_string())
    );
    assert_eq!(result["partialUriPresent"], Value::Bool(true));
    assert_eq!(result["partialOkActor"], Value::Bool(true));
    assert_eq!(result["partialCompleted"], Value::Number(1.into()));
    assert_eq!(result["partialBudgetFailures"], Value::Number(1.into()));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_drive_fail_closed_denial_paths() {
    let no_drive = DenoSandbox::default();
    let mut no_drive_session = no_drive.open(SessionConfig::default()).await.unwrap();
    let missing = no_drive_session
        .eval(
            "await drive.put('x').catch(err => ({ name: err.name, capability: err.capability }))",
            CellBudget::default(),
        )
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        missing["name"],
        Value::String("CapabilityDeniedError".to_string())
    );

    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    fs::write(root.path().join("README.md"), "TempestMiku\n").unwrap();
    let sandbox = p0_sandbox(root.path(), artifacts.path());
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval(
            "const unknown = await tools.call('drive.nope', {}).catch(err => ({ name: err.name, capability: err.capability }));\n\
             const rawPath = await drive.put('x', { suggestedPath: '/Users/brian/secret.txt' }).catch(err => ({ name: err.name, path: err.path }));\n\
             const oversized = await drive.put('x'.repeat(5 * 1024 * 1024 + 1)).catch(err => ({ name: err.name }));\n\
             const autoTimeout = await drive.put('blocked', { auto: true, suggestedPath: 'notes/blocked.txt' }).catch(err => ({ name: err.name, retryable: err.retryable }));\n\
             const missingAfterTimeout = await drive.get('notes/blocked.txt').catch(err => ({ name: err.name }));\n\
             const filed = await drive.put('hello', { suggestedPath: 'notes/a.txt' });\n\
             const timeout = await drive.move(filed.uri, 'notes/b.txt').catch(err => ({ name: err.name, retryable: err.retryable }));\n\
             ({ unknown, rawPath, oversized, autoTimeout, missingAfterTimeout, timeout })",
            CellBudget {
                wall_ms: 30_000,
                output_bytes: 50_000,
            },
        )
        .await
        .unwrap();

    assert!(
        out.error.is_none(),
        "drive denial eval failed: {:?}",
        out.error
    );
    let result = out.result.unwrap();
    assert_eq!(
        result["unknown"]["name"],
        Value::String("CapabilityDeniedError".to_string())
    );
    assert_eq!(
        result["rawPath"]["name"],
        Value::String("InvalidPathError".to_string())
    );
    assert_eq!(
        result["oversized"]["name"],
        Value::String("InvalidArgsError".to_string())
    );
    assert_eq!(
        result["autoTimeout"]["name"],
        Value::String("ApprovalTimeoutError".to_string())
    );
    assert_eq!(result["autoTimeout"]["retryable"], Value::Bool(true));
    assert_eq!(
        result["missingAfterTimeout"]["name"],
        Value::String("NotFoundError".to_string())
    );
    assert_eq!(
        result["timeout"]["name"],
        Value::String("ApprovalTimeoutError".to_string())
    );
    assert_eq!(result["timeout"]["retryable"], Value::Bool(true));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_tools_docs_match_sdk_declarations_for_exposed_namespace_methods() {
    let sdk_types = include_str!("../../../docs/sdk/tm-runtime.d.ts");
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    fs::write(root.path().join("README.md"), "TempestMiku\n").unwrap();
    let sandbox = p0_sandbox(root.path(), artifacts.path());
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let expected_names: Vec<&str> = SDK_CATALOG_NAMESPACE_METHODS
        .iter()
        .map(|(name, _)| *name)
        .collect();
    let js_expected = serde_json::to_string(&expected_names).unwrap();
    let out = session
        .eval(
            &format!(
                r#"
                (async () => {{
                const expected = {js_expected}
                const namespaces = ["tools", "resources", "artifacts", "fs", "code", "proc", "http", "drive"]
                const runtimeMethods = namespaces.flatMap((namespace) =>
                  Object.keys(globalThis[namespace] ?? {{}}).map((method) => `${{namespace}}.${{method}}`)
                ).sort()
                const docs = {{}}
                for (const name of expected) {{
                  const doc = await tools.docs(name)
                  const found = await tools.search(name, {{ namespace: doc.namespace, limit: 50 }})
                  docs[name] = {{
                    namespace: doc.namespace,
                    signature: doc.signature,
                    description: doc.description,
                    argsSchemaType: doc.argsSchema?.type,
                    resultSchemaPresent: doc.resultSchema != null,
                    examples: doc.examples.length,
                    errors: doc.errors.length,
                    grants: doc.grants.length,
                    approval: doc.approval,
                    since: doc.since,
                    stability: doc.stability,
                    searchHit: found.some((item) => item.name === name)
                  }}
                }}
                return {{ runtimeMethods, docs }}
                }})()
                "#
            ),
            CellBudget {
                output_bytes: 50_000,
                ..CellBudget::default()
            },
        )
        .await
        .unwrap();
    assert!(out.error.is_none(), "parity eval failed: {:?}", out.error);
    let result = out.result.unwrap();
    let runtime_methods: Vec<String> = result["runtimeMethods"]
        .as_array()
        .unwrap_or_else(|| panic!("runtimeMethods missing from parity result: {result:?}"))
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    let mut expected_sorted: Vec<String> =
        expected_names.iter().map(|name| name.to_string()).collect();
    expected_sorted.sort();
    assert_eq!(
        runtime_methods, expected_sorted,
        "update SDK_CATALOG_NAMESPACE_METHODS when a direct namespace method is exposed"
    );

    for (name, namespace) in SDK_CATALOG_NAMESPACE_METHODS {
        let docs = &result["docs"][*name];
        assert_eq!(
            docs["namespace"],
            Value::String((*namespace).to_string()),
            "{name} should report its SDK namespace"
        );
        let signature = docs["signature"].as_str().unwrap();
        assert!(
            sdk_types.contains(signature),
            "docs/sdk/tm-runtime.d.ts is missing the tools.docs signature for {name}: {signature}"
        );
        assert_eq!(
            docs["argsSchemaType"],
            Value::String("object".into()),
            "{name} should document an object args schema"
        );
        assert!(
            docs["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "{name} should include a description"
        );
        assert!(
            docs["resultSchemaPresent"].as_bool().unwrap() || *name == "tools.call",
            "{name} should document a result schema unless the generic tools.call result is target-dependent"
        );
        assert!(
            docs["examples"].as_u64().unwrap() > 0,
            "{name} should include at least one example"
        );
        assert!(
            docs["errors"].as_u64().unwrap() > 0,
            "{name} should document fail-closed errors"
        );
        assert!(
            docs["grants"].as_u64().unwrap() > 0,
            "{name} should document grant behavior"
        );
        assert!(
            docs["approval"].as_str().is_some_and(|approval| matches!(
                approval,
                "none" | "on-write" | "on-external" | "always" | "policy"
            )),
            "{name} should use a declared approval policy"
        );
        assert!(
            docs["since"]
                .as_str()
                .is_some_and(|since| !since.is_empty()),
            "{name} should include since metadata"
        );
        assert!(
            docs["stability"].as_str().is_some_and(|stability| matches!(
                stability,
                "stable" | "experimental" | "reserved" | "deprecated"
            )),
            "{name} should use a declared stability value"
        );
        assert_eq!(
            docs["searchHit"],
            Value::Bool(true),
            "{name} should be discoverable through tools.search"
        );
    }
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_artifacts_resolve_through_resource_registry() {
    let sdk_types = include_str!("../../../docs/sdk/tm-runtime.d.ts");
    let dir = tempfile::tempdir().unwrap();
    let sandbox = DenoSandbox::new(DenoSandboxOptions {
        artifact_root: dir.path().to_path_buf(),
        ..DenoSandboxOptions::default()
    });
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval(
            "const ref = artifacts.put('one\\ntwo', { title: 'manual' });\n\
                 await resources.read(ref.uri, '2-2')",
            CellBudget::default(),
        )
        .await
        .unwrap();
    let result = out.result.unwrap();
    assert_eq!(result["content"], Value::String("two".into()));
    assert_eq!(result["sizeBytes"], Value::Number(7.into()));
    assert_eq!(result["hasMore"], Value::Bool(false));

    let denied = session
        .eval("await resources.read('memory://x')", CellBudget::default())
        .await
        .unwrap();
    let error = denied.error.unwrap();
    assert!(error.contains("CapabilityDeniedError"));
    assert!(error.contains("unknown resource scheme"));

    let skill_denied = session
        .eval(
            "await resources.read('skill://miku-voice').catch((err) => ({ name: err.name, uri: err.uri, scheme: err.details.scheme, registered: err.details.registered }))",
            CellBudget::default(),
        )
        .await
        .unwrap();
    let skill_result = skill_denied.result.unwrap();
    assert_eq!(
        skill_result["name"],
        Value::String("CapabilityDeniedError".into())
    );
    assert_eq!(skill_result["uri"], Value::String("skill://".into()));
    assert_eq!(skill_result["scheme"], Value::String("skill".into()));
    assert!(
        skill_result["registered"]
            .as_array()
            .unwrap()
            .iter()
            .any(|scheme| scheme.as_str() == Some("artifact"))
    );
    assert!(
        !skill_result["registered"]
            .as_array()
            .unwrap()
            .iter()
            .any(|scheme| scheme.as_str() == Some("skill"))
    );

    let docs = session
            .eval(
                "const artifactDocs = await tools.docs('artifacts.put');\n\
                 const resourceDocs = await tools.docs('resources.read');\n\
                 const found = await tools.search('artifact', { namespace: 'artifacts', limit: 10 });\n\
                 const resourceFound = await tools.search('read', { namespace: 'resources', limit: 10 });\n\
                 const schemes = await resources.list();\n\
                 ({ artifactSignature: artifactDocs.signature, resourceSignature: resourceDocs.signature, resourceDescription: resourceDocs.description, resourceGrantKinds: resourceDocs.grants.map(grant => grant.kind), resourceGrantDescriptions: resourceDocs.grants.map(grant => grant.description), resourceErrors: resourceDocs.errors.map(err => err.name), artifactResultRequired: artifactDocs.resultSchema.required[0], resourceContentType: resourceDocs.resultSchema.properties.content.type, foundNames: found.map(item => item.name), resourceFoundNames: resourceFound.map(item => item.name), resourceReadGranted: resourceFound.find(item => item.name === 'resources.read').granted, putGranted: found.find(item => item.name === 'artifacts.put').granted, schemeNames: schemes.map(item => item.name) })",
                CellBudget::default(),
            )
            .await
            .unwrap();
    let result = docs.result.unwrap();
    assert_eq!(
        result["artifactSignature"],
        Value::String(
            "artifacts.put(data: ArtifactInput, opts?: ArtifactPutOptions): ArtifactRef".into()
        )
    );
    assert_eq!(
            result["resourceSignature"],
            Value::String(
                "resources.read(uri: ResourceUri, selector?: ResourceSelector): Promise<ResourceContent>"
                    .into()
            )
        );
    assert!(
        sdk_types.contains(result["artifactSignature"].as_str().unwrap()),
        "docs/sdk/tm-runtime.d.ts is missing the artifacts.put signature"
    );
    assert!(
        sdk_types.contains(result["resourceSignature"].as_str().unwrap()),
        "docs/sdk/tm-runtime.d.ts is missing the resources.read signature"
    );
    assert!(
        sdk_types.contains("type MemoryResourceUri"),
        "docs/sdk/tm-runtime.d.ts should declare the P2 memory:// resource surface"
    );
    assert!(
        sdk_types.contains("type SkillPromptLabel = `skill://${string}`;"),
        "docs/sdk/tm-runtime.d.ts should keep skill:// as a prompt-composition label"
    );
    let resource_uri_decl = sdk_types
        .split("type ResourceUri =")
        .nth(1)
        .unwrap()
        .split("type SdkPath")
        .next()
        .unwrap();
    assert!(
        !resource_uri_decl.contains("skill://"),
        "skill:// is prompt-composition-only until P4/P7 and must not be part of ResourceUri"
    );
    let resource_description = result["resourceDescription"].as_str().unwrap();
    for needle in [
        "artifact://",
        "linked://",
        "memory://",
        "resources.read:memory",
        "skill://",
        "prompt-composition-only",
        "unknown scheme",
    ] {
        assert!(
            resource_description.contains(needle),
            "resources.read docs should mention {needle}: {resource_description}"
        );
    }
    for grant_kind in ["artifact", "linked-folder", "memory"] {
        assert!(
            result["resourceGrantKinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|kind| kind.as_str() == Some(grant_kind)),
            "resources.read docs should include a {grant_kind} grant"
        );
    }
    assert!(
        result["resourceGrantDescriptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|description| description
                .as_str()
                .is_some_and(|text| text.contains("resources.read:memory"))),
        "resources.read docs should name the memory grant"
    );
    assert!(
        result["resourceErrors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name.as_str() == Some("CapabilityDeniedError"))
    );
    assert_eq!(
        result["artifactResultRequired"],
        Value::String("uri".into())
    );
    assert_eq!(
        result["resourceContentType"],
        Value::String("string".into())
    );
    assert_eq!(result["putGranted"], Value::Bool(true));
    assert_eq!(result["resourceReadGranted"], Value::Bool(true));
    assert!(
        result["resourceFoundNames"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name.as_str() == Some("resources.read"))
    );
    assert!(
        result["schemeNames"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name.as_str() == Some("artifact"))
    );
    assert!(
        result["foundNames"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name.as_str() == Some("artifacts.get"))
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_unknown_host_capability_fails_closed() {
    let sandbox = DenoSandbox::default();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval(
            "await tools.call('missing.capability', {})",
            CellBudget::default(),
        )
        .await
        .unwrap();
    let error = out.error.unwrap();
    assert!(error.contains("CapabilityDeniedError"));
    assert!(error.contains("missing.capability"));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_host_errors_are_structured_js_errors() {
    let sandbox = DenoSandbox::default();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
            .eval(
                "const err = await tools.call('missing.capability', {}).catch((err) => ({ name: err.name, message: err.message, capability: err.capability, retryable: err.retryable, details: err.details }));\n\
                 err",
                CellBudget::default(),
            )
            .await
            .unwrap();
    let result = out.result.unwrap();
    assert_eq!(
        result["name"],
        Value::String("CapabilityDeniedError".into())
    );
    assert_eq!(
        result["capability"],
        Value::String("missing.capability".into())
    );
    assert_eq!(result["retryable"], Value::Bool(false));
    assert_eq!(
        result["details"]["capability"],
        Value::String("missing.capability".into())
    );
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains("capability denied")
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_http_get_is_default_deny_and_allowlisted() {
    let sdk_types = include_str!("../../../docs/sdk/tm-runtime.d.ts");
    let mut http_allowlist = BTreeMap::new();
    http_allowlist.insert("https://local.test/ok".to_string(), "ok".to_string());
    let sandbox = DenoSandbox::new(DenoSandboxOptions {
        http_allowlist,
        ..DenoSandboxOptions::default()
    });
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let denied = session
        .eval(
            "await http.get('https://evil.test/')",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(denied.error.unwrap().contains("CapabilityDeniedError"));
    let allowed = session
        .eval(
            "await http.get('https://local.test/ok')",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.result, Some(Value::String("ok".into())));
    let composed = session
        .eval(
            "const body = await http.get('https://local.test/ok'); display(body)",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert!(composed.stdout.contains("display: ok"));

    let docs = session
            .eval(
                "const found = await tools.search('http', { namespace: 'http' });\n\
                 const docs = await tools.docs('http.get');\n\
                 const unknown = await tools.call('http.post', {}).catch(err => ({ name: err.name, capability: err.capability, retryable: err.retryable }));\n\
                 ({ found: found.map(item => ({ name: item.name, granted: item.granted, sensitive: item.sensitive })), signature: docs.signature, description: docs.description, grantKind: docs.grants[0].kind, grantDescription: docs.grants[0].description, errorWhen: docs.errors[0].when, exampleNotes: docs.examples[0].notes, deniedName: unknown.name, deniedCapability: unknown.capability, deniedRetryable: unknown.retryable })",
                CellBudget::default(),
            )
            .await
            .unwrap();
    let result = docs.result.unwrap();
    assert_eq!(result["found"][0]["name"], Value::String("http.get".into()));
    assert_eq!(result["found"][0]["granted"], Value::Bool(true));
    assert_eq!(result["found"][0]["sensitive"], Value::Bool(true));
    assert_eq!(
        result["signature"],
        Value::String("http.get(url: string): Promise<string>".into())
    );
    assert!(
        sdk_types.contains(result["signature"].as_str().unwrap()),
        "docs/sdk/tm-runtime.d.ts is missing the http.get signature"
    );
    let http_description = result["description"].as_str().unwrap();
    for needle in [
        "default-deny deterministic allowlist helper",
        "not ambient network egress",
        "not fetch()",
        "not a production egress policy",
        "production egress hardening remains deferred",
    ] {
        assert!(
            http_description.contains(needle),
            "http.get docs should mention {needle}: {http_description}"
        );
    }
    assert!(
        sdk_types.contains("production egress hardening remains deferred"),
        "docs/sdk/tm-runtime.d.ts should preserve deferred egress wording"
    );
    assert_eq!(result["grantKind"], Value::String("network".into()));
    assert!(
        result["grantDescription"]
            .as_str()
            .unwrap()
            .contains("no open egress")
    );
    assert!(
        result["errorWhen"]
            .as_str()
            .unwrap()
            .contains("deterministic allowlist")
    );
    assert!(
        result["exampleNotes"]
            .as_str()
            .unwrap()
            .contains("does not grant open network egress")
    );
    assert_eq!(
        result["deniedName"],
        Value::String("CapabilityDeniedError".into())
    );
    assert_eq!(
        result["deniedCapability"],
        Value::String("http.post".into())
    );
    assert_eq!(result["deniedRetryable"], Value::Bool(false));
}

#[test]
fn agents_namespace_types_declared_in_sdk() {
    let sdk_types = include_str!("../../../docs/sdk/tm-runtime.d.ts");
    // Interface and type declarations
    for needle in [
        "interface AgentsNamespace",
        "interface AgentDigest",
        "interface AgentHandle",
        "interface AgentTask",
        "type AgentPipelineTaskFn",
        "interface AgentPipelineStage",
        "interface AgentMessage",
        "interface AgentReceipt",
        "interface AgentBroadcastReceipt",
        "interface AgentCancelReceipt",
        "interface AgentRosterEntry",
        "interface AgentSpawnOpts",
        "interface AgentSupervisionOpts",
        "interface MsgOpts",
        "interface SendOpts",
        "agents: AgentsNamespace | undefined",
        // agents.* HostFn signatures — must match tools.docs output exactly
        "agents.run(role: string, task: string, opts?: AgentRunOpts): Promise<AgentDigest>",
        "agents.spawn(role: string, task: string, opts?: AgentSpawnOpts): Promise<AgentHandle>",
        "agents.parallel(tasks: AgentTask[]): Promise<AgentDigest[]>",
        "agents.pipeline(items: JsonValue[], ...stages: AgentPipelineStage[]): Promise<AgentDigest[][]>",
        "agents.msg(handle: AgentHandle, text: string, opts?: MsgOpts): Promise<AgentReceipt | string | null>",
        "agents.send(to: AgentHandle | string, text: string, opts?: SendOpts): Promise<AgentReceipt | AgentMessage | null>",
        "agents.broadcast(text: string): Promise<AgentBroadcastReceipt[]>",
        "agents.cancel(target: AgentHandle | string): Promise<AgentCancelReceipt>",
        "agents.wait(from?: AgentHandle | string, timeoutMs?: number): Promise<AgentMessage | null>",
        "agents.inbox(): Promise<AgentMessage[]>",
        "agents.list(): Promise<AgentRosterEntry[]>",
        // Protocol invariants documented
        "plain prose",
        "retry-loop",
        // GrantDoc kind extended
        r#"| "agents""#,
        // ResourceKind extended
        r#"| "actor""#,
        r#"| "history""#,
    ] {
        assert!(
            sdk_types.contains(needle),
            "docs/sdk/tm-runtime.d.ts is missing: {needle}"
        );
    }
    // agents must NOT appear as an always-defined global (it's undefined in ungranted sessions)
    assert!(
        !sdk_types.contains("const agents: AgentsNamespace;"),
        "agents must be AgentsNamespace | undefined, not unconditionally typed"
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_agents_namespace_wired_when_granted() {
    use tm_host::CapabilityGrants;
    let sandbox = DenoSandbox::new(DenoSandboxOptions {
        grants: CapabilityGrants::default()
            .allow("http.get")
            .allow("resources.read:artifact")
            .allow("agents.*"),
        ..DenoSandboxOptions::default()
    });
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval(
            "({ agentsType: typeof agents, runType: typeof agents?.run, spawnType: typeof agents?.spawn, parallelType: typeof agents?.parallel, pipelineType: typeof agents?.pipeline, msgType: typeof agents?.msg, sendType: typeof agents?.send, broadcastType: typeof agents?.broadcast, cancelType: typeof agents?.cancel, waitType: typeof agents?.wait, inboxType: typeof agents?.inbox, listType: typeof agents?.list })",
            CellBudget::default(),
        )
        .await
        .unwrap();
    let result = out.result.unwrap();
    assert_eq!(result["agentsType"], Value::String("object".into()));
    assert_eq!(result["runType"], Value::String("function".into()));
    assert_eq!(result["spawnType"], Value::String("function".into()));
    assert_eq!(result["parallelType"], Value::String("function".into()));
    assert_eq!(result["pipelineType"], Value::String("function".into()));
    assert_eq!(result["msgType"], Value::String("function".into()));
    assert_eq!(result["sendType"], Value::String("function".into()));
    assert_eq!(result["broadcastType"], Value::String("function".into()));
    assert_eq!(result["cancelType"], Value::String("function".into()));
    assert_eq!(result["waitType"], Value::String("function".into()));
    assert_eq!(result["inboxType"], Value::String("function".into()));
    assert_eq!(result["listType"], Value::String("function".into()));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_agents_namespace_undefined_without_grant() {
    let sandbox = DenoSandbox::default();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
        .eval("typeof agents", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(out.result, Some(Value::String("undefined".into())));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_p0_sdk_exposes_linked_repo_functions() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("src/lib.rs"),
        "pub fn edit() -> i32 { 1 }\n",
    )
    .unwrap();
    let sandbox = p0_sandbox(root.path(), artifacts.path());
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
            .eval(
                "const found = await tools.search('edit');\n\
                 const docs = await tools.docs('code.edit');\n\
                 const fsDocs = await tools.docs('fs.read');\n\
                 const read = await fs.read('tempestmiku:src/lib.rs');\n\
                 const listed = await fs.ls('tempestmiku:src');\n\
                 const hits = await code.search({ pattern: 'edit', paths: ['tempestmiku:src/lib.rs'], regex: false });\n\
                 const linked = await resources.read('linked://tempestmiku/src/lib.rs');\n\
                 ({ found: found.length, docName: docs.name, fsSignature: fsDocs.signature, fsRequired: fsDocs.argsSchema.required[0], fsResultContent: fsDocs.resultSchema.properties.content.type, fsExamples: fsDocs.examples.length, fsApproval: fsDocs.approval, readHasMore: read.hasMore, sizeBytes: listed[0].sizeBytes, hits: hits.length, linked: linked.content.includes('edit'), fsType: typeof fs, codeType: typeof code, procType: typeof proc, memoryType: typeof memory, skillsType: typeof skills, agentsType: typeof agents })",
                CellBudget::default(),
            )
            .await
            .unwrap();
    let result = out.result.unwrap();
    assert_eq!(result["docName"], Value::String("code.edit".into()));
    assert_eq!(
        result["fsSignature"],
        Value::String(
            "fs.read(path: SdkPath, opts?: FsReadOptions): Promise<ResourceContent>".into()
        )
    );
    assert_eq!(result["fsRequired"], Value::String("path".into()));
    assert_eq!(result["fsResultContent"], Value::String("string".into()));
    assert!(result["fsExamples"].as_u64().unwrap() > 0);
    assert_eq!(result["fsApproval"], Value::String("none".into()));
    assert_eq!(result["readHasMore"], Value::Bool(false));
    assert!(result["sizeBytes"].as_u64().unwrap() > 0);
    assert_eq!(result["hits"], Value::Number(1.into()));
    assert_eq!(result["linked"], Value::Bool(true));
    assert_eq!(result["fsType"], Value::String("object".into()));
    assert_eq!(result["codeType"], Value::String("object".into()));
    assert_eq!(result["procType"], Value::String("object".into()));
    assert_eq!(result["memoryType"], Value::String("undefined".into()));
    assert_eq!(result["skillsType"], Value::String("undefined".into()));
    assert_eq!(result["agentsType"], Value::String("undefined".into()));
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn deno_p0_linked_repo_patch_and_proc_run_through_sdk() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"p0-sdk-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(
            root.path().join("src/lib.rs"),
            "pub fn answer() -> i32 { 1 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn answer_is_two() {\n        assert_eq!(super::answer(), 2);\n    }\n}\n",
        )
        .unwrap();
    let sandbox = p0_sandbox(root.path(), artifacts.path());
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
    let out = session
            .eval(
                "const hits = await code.search({ pattern: '1', paths: ['tempestmiku:src/lib.rs'], regex: false });\n\
                 const tag = hits[0].tag;\n\
                 await code.edit({ path: 'tempestmiku:src/lib.rs', tag, hunks: [{ op: 'replace', startLine: 1, endLine: 1, lines: ['pub fn answer() -> i32 { 2 }'] }] });\n\
                 const invalid = await proc.run('cargo test', [], { cwd: 'tempestmiku:' }).catch(err => String(err));\n\
                 const run = await proc.run('cargo', ['test'], { cwd: 'tempestmiku:' });\n\
                 ({ exitCode: run.exitCode, invalid })",
                CellBudget {
                    wall_ms: 240_000,
                    output_bytes: 50_000,
                },
            )
            .await
            .unwrap();
    let result = out.result.unwrap();
    assert_eq!(result["exitCode"], Value::Number(0.into()));
    assert!(
        result["invalid"]
            .as_str()
            .unwrap()
            .contains("InvalidArgsError")
    );
    let changed = fs::read_to_string(root.path().join("src/lib.rs")).unwrap();
    assert!(changed.contains("pub fn answer() -> i32 { 2 }"));
}

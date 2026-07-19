use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, Path as AxumPath, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tm_artifacts::{ArtifactRef, ArtifactStore};
use tm_host::{
    ApprovalDecision, ApprovalPolicy, CapabilityGrants, HostError, HostEventSink, HostRegistry,
    InvocationCtx, P0HostConfig, ResourceRegistry,
    register_p0_linked_folder_functions_with_isolation,
};
use tm_worker_protocol::{
    ApprovalResolution, HealthResponse, JobRequest, JobState, JobStatus, MAX_CLOCK_SKEW_SECONDS,
    MAX_REQUEST_BODY_BYTES, PROTOCOL_VERSION, RequestAuth, ResolveApprovalRequest, SigningKey,
    WorkerEvent, WorkerOperation, current_unix_seconds, validate_worker_id,
};
use tokio::sync::{Mutex, RwLock, Semaphore, oneshot};
use uuid::Uuid;

const HEADER_TIMESTAMP: &str = "x-tm-worker-timestamp";
const HEADER_NONCE: &str = "x-tm-worker-nonce";
const HEADER_SIGNATURE: &str = "x-tm-worker-signature";
const MAX_NONCES: usize = 4096;
const MAX_EVENTS: usize = 128;
const MAX_EVENT_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerConfig {
    worker_id: String,
    listen_addr: SocketAddr,
    signing_key_file: PathBuf,
    host_config_file: PathBuf,
    ledger_root: PathBuf,
    #[serde(default = "default_approval_timeout_ms")]
    approval_timeout_ms: u64,
    #[serde(default = "default_max_concurrent_jobs")]
    max_concurrent_jobs: usize,
    #[serde(default = "default_max_concurrent_proc_runs")]
    max_concurrent_proc_runs: usize,
    #[serde(default = "default_retention_seconds")]
    retention_seconds: i64,
}

fn default_approval_timeout_ms() -> u64 {
    60_000
}

fn default_max_concurrent_jobs() -> usize {
    4
}
fn default_max_concurrent_proc_runs() -> usize {
    1
}
fn default_retention_seconds() -> i64 {
    86_400
}

impl WorkerConfig {
    fn load() -> anyhow::Result<Self> {
        let path = std::env::var_os("TM_WORKER_CONFIG")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("TM_WORKER_CONFIG is required"))?;
        let config: Self = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
        )
        .with_context(|| format!("parsing {}", path.display()))?;
        validate_worker_id(&config.worker_id).map_err(|error| anyhow!(error))?;
        if !(1_000..=300_000).contains(&config.approval_timeout_ms) {
            return Err(anyhow!("approvalTimeoutMs must be in 1000..=300000"));
        }
        if !(1..=32).contains(&config.max_concurrent_jobs)
            || !(1..=config.max_concurrent_jobs).contains(&config.max_concurrent_proc_runs)
        {
            return Err(anyhow!(
                "worker concurrency must be bounded and proc concurrency cannot exceed total concurrency"
            ));
        }
        if !(3600..=2_592_000).contains(&config.retention_seconds) {
            return Err(anyhow!("retentionSeconds must be in 3600..=2592000"));
        }
        Ok(config)
    }
}

#[derive(Clone)]
struct AppState {
    worker_id: Arc<str>,
    signing_key: SigningKey,
    registry: Arc<HostRegistry>,
    resources: Arc<ResourceRegistry>,
    artifacts: ArtifactStore,
    jobs: Arc<RwLock<HashMap<Uuid, Arc<JobEntry>>>>,
    ledger: JobLedger,
    nonces: Arc<Mutex<BTreeMap<String, i64>>>,
    approval_timeout: Duration,
    job_slots: Arc<Semaphore>,
    proc_slots: Arc<Semaphore>,
}

struct JobEntry {
    status: RwLock<JobStatus>,
    approval: Mutex<Option<oneshot::Sender<ApprovalDecision>>>,
    task: StdMutex<Option<tokio::task::AbortHandle>>,
}

#[derive(Clone)]
struct JobLedger {
    root: Arc<PathBuf>,
}

impl JobLedger {
    fn open(root: PathBuf) -> anyhow::Result<Self> {
        fs::create_dir_all(&root)?;
        let root = fs::canonicalize(root)?;
        Ok(Self {
            root: Arc::new(root),
        })
    }

    fn path(&self, job_id: Uuid) -> PathBuf {
        self.root.join(format!("{job_id}.json"))
    }

    fn persist(&self, status: &JobStatus) -> anyhow::Result<()> {
        let encoded = serde_json::to_vec_pretty(status)?;
        let path = self.path(status.job_id);
        let temporary = self
            .root
            .join(format!(".{}.{}.tmp", status.job_id, Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(self.root.as_ref())?.sync_all()?;
        Ok(())
    }

    fn recover(&self, worker_id: &str) -> anyhow::Result<Vec<JobStatus>> {
        let mut recovered = Vec::new();
        for entry in fs::read_dir(self.root.as_ref())? {
            let entry = entry?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let mut status: JobStatus = serde_json::from_slice(&fs::read(entry.path())?)?;
            if status.worker_id != worker_id {
                return Err(anyhow!("ledger contains a record for another worker"));
            }
            if !status.state.is_terminal() {
                status.state = JobState::Indeterminate;
                status.action = None;
                status.action_sha256 = None;
                status.result = None;
                status.error = Some(HostError::HostCall(
                    "worker restarted before the operation reached a durable terminal state; it was not replayed"
                        .to_string(),
                ).to_payload());
                status.updated_at = Utc::now().to_rfc3339();
                self.persist(&status)?;
            }
            recovered.push(status);
        }
        Ok(recovered)
    }

    fn prune(&self, retention_seconds: i64) -> anyhow::Result<()> {
        let cutoff = Utc::now() - chrono::Duration::seconds(retention_seconds);
        for entry in fs::read_dir(self.root.as_ref())? {
            let entry = entry?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let status: JobStatus = match serde_json::from_slice(&fs::read(entry.path())?) {
                Ok(status) => status,
                Err(_) => continue,
            };
            let updated = chrono::DateTime::parse_from_rfc3339(&status.updated_at)
                .map(|value| value.with_timezone(&Utc));
            if status.state.is_terminal() && updated.is_ok_and(|updated| updated < cutoff) {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct CapturingEventSink {
    events: Arc<Mutex<Vec<WorkerEvent>>>,
}

#[async_trait]
impl HostEventSink for CapturingEventSink {
    async fn emit(&self, event_type: &str, payload: Value) -> tm_host::Result<()> {
        let encoded =
            serde_json::to_vec(&payload).map_err(|error| HostError::HostCall(error.to_string()))?;
        if encoded.len() > MAX_EVENT_BYTES {
            return Err(HostError::OutputTruncated(
                "worker audit event exceeds the bounded wire envelope".to_string(),
            ));
        }
        let mut events = self.events.lock().await;
        if events.len() >= MAX_EVENTS {
            return Err(HostError::OutputTruncated(
                "worker audit event count exceeds the bounded wire envelope".to_string(),
            ));
        }
        events.push(WorkerEvent {
            event_type: event_type.to_string(),
            payload,
        });
        Ok(())
    }
}

struct JobApprovalPolicy {
    entry: Arc<JobEntry>,
    ledger: JobLedger,
}

#[async_trait]
impl ApprovalPolicy for JobApprovalPolicy {
    async fn request(&self, action: &str, timeout: Duration) -> tm_host::Result<ApprovalDecision> {
        let digest = hex::encode(Sha256::digest(action.as_bytes()));
        let (sender, receiver) = oneshot::channel();
        {
            let mut approval = self.entry.approval.lock().await;
            if approval.is_some() {
                return Err(HostError::HostCall(
                    "worker job attempted overlapping approval requests".to_string(),
                ));
            }
            *approval = Some(sender);
        }
        update_status(&self.entry, &self.ledger, |status| {
            status.state = JobState::AwaitingApproval;
            status.action = Some(action.to_string());
            status.action_sha256 = Some(digest);
        })
        .await?;
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(decision)) => {
                update_status(&self.entry, &self.ledger, |status| {
                    status.state = JobState::Running;
                    status.action = None;
                    status.action_sha256 = None;
                })
                .await?;
                Ok(decision)
            }
            Ok(Err(_)) | Err(_) => {
                let _ = self.entry.approval.lock().await.take();
                Err(HostError::ApprovalTimeout(action.to_string()))
            }
        }
    }
}

pub async fn run() -> anyhow::Result<()> {
    let config = WorkerConfig::load()?;
    let signing_key = SigningKey::from_hex_file(&config.signing_key_file)?;
    let host_config = P0HostConfig::from_json_file(&config.host_config_file)?;
    host_config.proc_isolation.recover_orphans_at_startup()?;
    let linked = host_config.linked_folders()?;
    let artifacts = ArtifactStore::open(
        host_config
            .artifact_root
            .clone()
            .unwrap_or_else(|| config.ledger_root.join("artifacts")),
        "worker",
    )?;
    let mut registry = HostRegistry::new();
    let mut resources = ResourceRegistry::new();
    register_p0_linked_folder_functions_with_isolation(
        &mut registry,
        &mut resources,
        linked,
        artifacts.clone(),
        Duration::from_millis(host_config.proc_run_timeout_ms),
        host_config.proc_isolation,
    );
    let ledger = JobLedger::open(config.ledger_root.join("jobs"))?;
    ledger.prune(config.retention_seconds)?;
    let jobs = Arc::new(RwLock::new(HashMap::new()));
    for status in ledger.recover(&config.worker_id)? {
        jobs.write().await.insert(
            status.job_id,
            Arc::new(JobEntry {
                status: RwLock::new(status),
                approval: Mutex::new(None),
                task: StdMutex::new(None),
            }),
        );
    }
    let state = AppState {
        worker_id: Arc::from(config.worker_id),
        signing_key,
        registry: Arc::new(registry),
        resources: Arc::new(resources),
        artifacts,
        jobs,
        ledger,
        nonces: Arc::new(Mutex::new(BTreeMap::new())),
        approval_timeout: Duration::from_millis(config.approval_timeout_ms),
        job_slots: Arc::new(Semaphore::new(config.max_concurrent_jobs)),
        proc_slots: Arc::new(Semaphore::new(config.max_concurrent_proc_runs)),
    };
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(config.listen_addr).await?;
    tracing::info!(addr = %config.listen_addr, "tm-worker listening");
    axum::serve(listener, app).await?;
    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/jobs", post(create_job))
        .route("/v1/jobs/:job_id", get(get_job).delete(cancel_job))
        .route("/v1/jobs/:job_id/approval", post(resolve_approval))
        .route("/v1/artifacts/:artifact_id", get(read_artifact))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        protocol_version: PROTOCOL_VERSION,
        worker_id: state.worker_id.to_string(),
        ready: true,
    })
}

async fn create_job(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = authenticate(&state, &Method::POST, uri.path(), &headers, &body).await {
        return response;
    }
    let request: JobRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, error.to_string()),
    };
    if request.protocol_version != PROTOCOL_VERSION || request.worker_id != state.worker_id.as_ref()
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "worker protocol or identity mismatch",
        );
    }
    if let Some(entry) = state.jobs.read().await.get(&request.job_id).cloned() {
        return Json(entry.status.read().await.clone()).into_response();
    }
    let status = JobStatus {
        protocol_version: PROTOCOL_VERSION,
        job_id: request.job_id,
        worker_id: state.worker_id.to_string(),
        state: JobState::Queued,
        action: None,
        action_sha256: None,
        result: None,
        error: None,
        events: Vec::new(),
        updated_at: Utc::now().to_rfc3339(),
    };
    if let Err(error) = state.ledger.persist(&status) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    let entry = Arc::new(JobEntry {
        status: RwLock::new(status),
        approval: Mutex::new(None),
        task: StdMutex::new(None),
    });
    state
        .jobs
        .write()
        .await
        .insert(request.job_id, Arc::clone(&entry));
    let task = tokio::spawn(run_job(state.clone(), Arc::clone(&entry), request));
    *entry.task.lock().expect("job task lock poisoned") = Some(task.abort_handle());
    Json(entry.status.read().await.clone()).into_response()
}

async fn run_job(state: AppState, entry: Arc<JobEntry>, request: JobRequest) {
    let Ok(_job_slot) = Arc::clone(&state.job_slots).acquire_owned().await else {
        return;
    };
    let is_proc_run = matches!(
        &request.operation,
        WorkerOperation::Invoke { capability, .. } if capability == "proc.run"
    );
    let _proc_slot = if is_proc_run {
        Arc::clone(&state.proc_slots).acquire_owned().await.ok()
    } else {
        None
    };
    if let Err(error) = update_status(&entry, &state.ledger, |status| {
        status.state = JobState::Running
    })
    .await
    {
        tracing::error!(%error, job_id = %request.job_id, "failed to persist running worker job");
        return;
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let approvals: Arc<dyn ApprovalPolicy> = Arc::new(JobApprovalPolicy {
        entry: Arc::clone(&entry),
        ledger: state.ledger.clone(),
    });
    let mut ctx = InvocationCtx::with_approvals(
        CapabilityGrants::default().allow_many(request.authority.grants.clone()),
        approvals,
        state.approval_timeout,
    )
    .with_session_id(request.authority.session_id)
    .with_actor_id(request.authority.actor_id)
    .with_event_sink(Arc::new(CapturingEventSink {
        events: Arc::clone(&events),
    }));
    if let Some(scope) = request.authority.session_scope {
        ctx = ctx.with_session_scope(scope);
    }
    let result = match request.operation {
        WorkerOperation::Invoke { capability, args } => {
            state.registry.invoke(&capability, args, &ctx).await
        }
        WorkerOperation::ResourceRead { uri, selector } => state
            .resources
            .read(&uri, selector.as_deref(), &ctx)
            .await
            .and_then(|value| {
                serde_json::to_value(value).map_err(|error| HostError::HostCall(error.to_string()))
            }),
        WorkerOperation::ResourcePreview { uri } => {
            state.resources.preview(&uri, &ctx).await.and_then(|value| {
                serde_json::to_value(value).map_err(|error| HostError::HostCall(error.to_string()))
            })
        }
        WorkerOperation::ResourceList { uri } => state
            .resources
            .list(uri.as_deref(), &ctx)
            .await
            .and_then(|value| {
                serde_json::to_value(value).map_err(|error| HostError::HostCall(error.to_string()))
            }),
    };
    let captured = events.lock().await.clone();
    let persist = update_status(&entry, &state.ledger, |status| {
        status.action = None;
        status.action_sha256 = None;
        status.events = captured;
        match result {
            Ok(value) => {
                status.state = JobState::Succeeded;
                status.result = Some(value);
                status.error = None;
            }
            Err(error) => {
                status.state = JobState::Failed;
                status.result = None;
                status.error = Some(error.to_payload());
            }
        }
    })
    .await;
    if let Err(error) = persist {
        tracing::error!(%error, job_id = %request.job_id, "failed to persist terminal worker job");
    }
}

async fn get_job(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    AxumPath(job_id): AxumPath<Uuid>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authenticate(&state, &Method::GET, uri.path(), &headers, &[]).await {
        return response;
    }
    match state.jobs.read().await.get(&job_id).cloned() {
        Some(entry) => Json(entry.status.read().await.clone()).into_response(),
        None => api_error(StatusCode::NOT_FOUND, "worker job not found"),
    }
}

async fn resolve_approval(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    AxumPath(job_id): AxumPath<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = authenticate(&state, &Method::POST, uri.path(), &headers, &body).await {
        return response;
    }
    let request: ResolveApprovalRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, error.to_string()),
    };
    if request.protocol_version != PROTOCOL_VERSION {
        return api_error(StatusCode::BAD_REQUEST, "worker protocol mismatch");
    }
    let Some(entry) = state.jobs.read().await.get(&job_id).cloned() else {
        return api_error(StatusCode::NOT_FOUND, "worker job not found");
    };
    let status = entry.status.read().await.clone();
    if status.state != JobState::AwaitingApproval
        || status.action_sha256.as_deref() != Some(request.action_sha256.as_str())
    {
        return api_error(
            StatusCode::CONFLICT,
            "approval action does not match the waiting job",
        );
    }
    let Some(sender) = entry.approval.lock().await.take() else {
        return api_error(
            StatusCode::CONFLICT,
            "worker job has no live approval waiter",
        );
    };
    let decision = match request.resolution {
        ApprovalResolution::Approved => ApprovalDecision::Approved,
        ApprovalResolution::Denied => ApprovalDecision::Denied,
    };
    if sender.send(decision).is_err() {
        return api_error(
            StatusCode::CONFLICT,
            "worker approval waiter already closed",
        );
    }
    Json(entry.status.read().await.clone()).into_response()
}

async fn cancel_job(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    AxumPath(job_id): AxumPath<Uuid>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authenticate(&state, &Method::DELETE, uri.path(), &headers, &[]).await {
        return response;
    }
    let Some(entry) = state.jobs.read().await.get(&job_id).cloned() else {
        return api_error(StatusCode::NOT_FOUND, "worker job not found");
    };
    if !entry.status.read().await.state.is_terminal() {
        if let Some(task) = entry.task.lock().expect("job task lock poisoned").take() {
            task.abort();
        }
        let _ = entry.approval.lock().await.take();
        if let Err(error) = update_status(&entry, &state.ledger, |status| {
            status.state = JobState::Cancelled;
            status.action = None;
            status.action_sha256 = None;
            status.result = None;
            status.error = None;
        })
        .await
        {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
    }
    Json(entry.status.read().await.clone()).into_response()
}

async fn read_artifact(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    AxumPath(artifact_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authenticate(&state, &Method::GET, uri.path(), &headers, &[]).await {
        return response;
    }
    let artifact_uri = format!("artifact://{artifact_id}");
    match state.artifacts.read_all_text(&artifact_uri) {
        Ok((artifact, content)) => Json(json!({
            "artifact": ArtifactRef {
                uri: artifact.uri,
                id: artifact_id,
                kind: artifact.kind,
                mime: artifact.mime,
                title: artifact.title,
                size_bytes: artifact.size_bytes,
                preview: artifact.preview,
            },
            "sha256": hex::encode(Sha256::digest(content.as_bytes())),
            "content": content,
        }))
        .into_response(),
        Err(error) => api_error(StatusCode::NOT_FOUND, error.to_string()),
    }
}

async fn update_status(
    entry: &Arc<JobEntry>,
    ledger: &JobLedger,
    update: impl FnOnce(&mut JobStatus),
) -> tm_host::Result<()> {
    let snapshot = {
        let mut status = entry.status.write().await;
        update(&mut status);
        status.updated_at = Utc::now().to_rfc3339();
        status.clone()
    };
    ledger
        .persist(&snapshot)
        .map_err(|error| HostError::HostCall(format!("persisting worker job: {error}")))
}

async fn authenticate(
    state: &AppState,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), Response> {
    if body.len() > MAX_REQUEST_BODY_BYTES {
        return Err(api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body is too large",
        ));
    }
    let timestamp = required_header(headers, HEADER_TIMESTAMP)
        .map_err(|message| api_error(StatusCode::UNAUTHORIZED, message))?
        .parse::<i64>()
        .map_err(|_| api_error(StatusCode::UNAUTHORIZED, "invalid request timestamp"))?;
    let nonce = required_header(headers, HEADER_NONCE)
        .map_err(|message| api_error(StatusCode::UNAUTHORIZED, message))?;
    if nonce.is_empty() || nonce.len() > 128 || !nonce.is_ascii() {
        return Err(api_error(StatusCode::UNAUTHORIZED, "invalid request nonce"));
    }
    let auth = RequestAuth {
        timestamp,
        nonce: nonce.to_string(),
        signature: required_header(headers, HEADER_SIGNATURE)
            .map_err(|message| api_error(StatusCode::UNAUTHORIZED, message))?
            .to_string(),
    };
    auth.verify(
        &state.signing_key,
        method.as_str(),
        path,
        body,
        current_unix_seconds(),
        MAX_CLOCK_SKEW_SECONDS,
    )
    .map_err(|error| api_error(StatusCode::UNAUTHORIZED, error.to_string()))?;
    let mut nonces = state.nonces.lock().await;
    let cutoff = current_unix_seconds() - MAX_CLOCK_SKEW_SECONDS * 2;
    nonces.retain(|_, seen| *seen >= cutoff);
    if nonces.contains_key(nonce) {
        return Err(api_error(
            StatusCode::CONFLICT,
            "request nonce was already used",
        ));
    }
    if nonces.len() >= MAX_NONCES {
        let oldest = nonces
            .iter()
            .min_by_key(|(_, timestamp)| *timestamp)
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            nonces.remove(&oldest);
        }
    }
    nonces.insert(nonce.to_string(), timestamp);
    Ok(())
}

fn required_header<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<&'a str, String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| format!("missing {name}"))
}

fn api_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_host::{ApprovalDecision, SessionHostConnector};
    use tm_worker_protocol::{RemoteWorkerConfig, RemoteWorkerConnector};

    #[derive(Debug)]
    struct ApproveAll;

    #[async_trait]
    impl ApprovalPolicy for ApproveAll {
        async fn request(
            &self,
            _action: &str,
            _timeout: Duration,
        ) -> tm_host::Result<ApprovalDecision> {
            Ok(ApprovalDecision::Approved)
        }
    }

    fn test_state(root: &std::path::Path, key: SigningKey) -> AppState {
        let linked_root = root.join("linked");
        fs::create_dir_all(&linked_root).unwrap();
        fs::write(linked_root.join("hello.txt"), "hello worker\n").unwrap();
        fs::write(linked_root.join("large.txt"), "x".repeat(300 * 1024)).unwrap();
        let host_config_path = root.join("host.json");
        fs::write(
            &host_config_path,
            serde_json::to_vec(&json!({
                "linked_folders": [{
                    "name": "repo",
                    "path": linked_root,
                    "mode": "rw",
                    "commands": ["cat"]
                }],
                "approvals": { "mode": "deny", "timeout_ms": 60000 },
                "artifact_root": root.join("worker-artifacts"),
                "proc_run_timeout_ms": 10000,
                "proc_isolation": { "provider": "disabled" }
            }))
            .unwrap(),
        )
        .unwrap();
        let host_config = P0HostConfig::from_json_file(host_config_path).unwrap();
        let linked = host_config.linked_folders().unwrap();
        let artifacts = ArtifactStore::open(root.join("worker-artifacts"), "worker").unwrap();
        let mut registry = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        register_p0_linked_folder_functions_with_isolation(
            &mut registry,
            &mut resources,
            linked,
            artifacts.clone(),
            Duration::from_secs(10),
            host_config.proc_isolation,
        );
        let ledger = JobLedger::open(root.join("ledger/jobs")).unwrap();
        AppState {
            worker_id: Arc::from("homolab"),
            signing_key: key,
            registry: Arc::new(registry),
            resources: Arc::new(resources),
            artifacts,
            jobs: Arc::new(RwLock::new(HashMap::new())),
            ledger,
            nonces: Arc::new(Mutex::new(BTreeMap::new())),
            approval_timeout: Duration::from_secs(2),
            job_slots: Arc::new(Semaphore::new(4)),
            proc_slots: Arc::new(Semaphore::new(1)),
        }
    }

    #[test]
    fn ledger_marks_interrupted_jobs_indeterminate() {
        let temp = tempfile::tempdir().unwrap();
        let ledger = JobLedger::open(temp.path().join("jobs")).unwrap();
        let status = JobStatus {
            protocol_version: PROTOCOL_VERSION,
            job_id: Uuid::new_v4(),
            worker_id: "homolab".to_string(),
            state: JobState::Running,
            action: None,
            action_sha256: None,
            result: None,
            error: None,
            events: Vec::new(),
            updated_at: Utc::now().to_rfc3339(),
        };
        ledger.persist(&status).unwrap();
        let recovered = ledger.recover("homolab").unwrap();
        assert_eq!(recovered[0].state, JobState::Indeterminate);
        assert!(recovered[0].error.is_some());
    }

    #[tokio::test]
    async fn signed_remote_connector_reads_and_runs_through_worker() {
        let temp = tempfile::tempdir().unwrap();
        let key_hex = "22".repeat(32);
        let key = SigningKey::from_hex(&key_hex).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = router(test_state(temp.path(), key));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let key_path = temp.path().join("key");
        fs::write(&key_path, &key_hex).unwrap();
        let connector = RemoteWorkerConnector::from_config(RemoteWorkerConfig {
            worker_id: "homolab".to_string(),
            endpoint: format!("http://{address}"),
            signing_key_file: key_path,
            linked_aliases: vec!["repo".to_string()],
        })
        .unwrap();
        let local_artifacts =
            ArtifactStore::open(temp.path().join("local-artifacts"), "session").unwrap();
        let mut registry = HostRegistry::new();
        let mut resources = ResourceRegistry::new();
        connector
            .register(&mut registry, &mut resources, local_artifacts.clone())
            .unwrap();
        let ctx = InvocationCtx::with_approvals(
            CapabilityGrants::default().allow_many(["fs.read", "proc.run"]),
            Arc::new(ApproveAll),
            Duration::from_secs(2),
        )
        .with_session_id(Uuid::new_v4().to_string())
        .with_session_scope("project:repo");

        let read = registry
            .invoke("fs.read", json!({ "path": "repo:hello.txt" }), &ctx)
            .await
            .unwrap();
        assert_eq!(read["content"], json!("hello worker\n"));

        let run = registry
            .invoke(
                "proc.run",
                json!({ "cmd": "cat", "args": ["hello.txt"], "cwd": "repo:" }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(run["exitCode"], json!(0));
        assert_eq!(run["stdout"], json!("hello worker\n"));

        let spill = registry
            .invoke(
                "proc.run",
                json!({
                    "cmd": "cat",
                    "args": ["large.txt"],
                    "cwd": "repo:",
                    "outputBytes": 1024
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(spill["truncated"], json!(true));
        let localized_uri = spill["artifact"]["uri"].as_str().unwrap();
        let (_, localized) = local_artifacts.read_all_text(localized_uri).unwrap();
        assert_eq!(localized.len(), 300 * 1024);
        assert!(localized.bytes().all(|byte| byte == b'x'));

        server.abort();
    }
}

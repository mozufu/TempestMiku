use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};

use async_trait::async_trait;
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tm_artifacts::{ArtifactRef, ArtifactStore, ResourceContent};
use tm_host::{
    HostError, HostFn, HostRegistry, InvocationCtx, ResourceEntry, ResourceHandler,
    ResourceRegistry, SessionHostConnector, ToolDocs, linked_tool_docs,
};
use url::Host;
use uuid::Uuid;

use crate::{
    ApprovalResolution, JobRequest, JobState, JobStatus, PROTOCOL_VERSION, RequestAuth,
    ResolveApprovalRequest, SigningKey, WorkerAuthority, WorkerOperation, current_unix_seconds,
    validate_worker_id,
};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const REMOTE_JOB_TIMEOUT: Duration = Duration::from_secs(300);
const LINKED_CAPABILITIES: &[&str] = &[
    "fs.read",
    "fs.write",
    "fs.patch",
    "fs.move",
    "fs.remove",
    "fs.ls",
    "fs.find",
    "fs.grep",
    "git.status",
    "git.diff",
    "git.log",
    "git.commit",
    "git.push",
    "git.pull",
    "git.clone",
    "git.init",
    "git.add",
    "git.mv",
    "git.restore",
    "git.rm",
    "git.bisect",
    "git.grep",
    "git.show",
    "proc.run",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteWorkerConfig {
    pub worker_id: String,
    pub endpoint: String,
    pub signing_key_file: PathBuf,
    pub linked_aliases: Vec<String>,
}

impl RemoteWorkerConfig {
    pub fn validate(&self) -> Result<(), HostError> {
        validate_worker_id(&self.worker_id)
            .map_err(|error| HostError::InvalidArgs(error.into()))?;
        if self.linked_aliases.is_empty() {
            return Err(HostError::InvalidArgs(
                "remote worker must expose at least one linked alias".to_string(),
            ));
        }
        let mut sorted = self.linked_aliases.clone();
        sorted.sort();
        sorted.dedup();
        if sorted.len() != self.linked_aliases.len()
            || sorted.iter().any(|alias| {
                alias.is_empty()
                    || alias.len() > 64
                    || !alias.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'-' | b'_')
                    })
            })
        {
            return Err(HostError::InvalidArgs(
                "remote linked aliases must be unique lowercase identifiers".to_string(),
            ));
        }
        let endpoint = Url::parse(&self.endpoint)
            .map_err(|error| HostError::InvalidArgs(format!("invalid worker endpoint: {error}")))?;
        if endpoint.scheme() != "http" && endpoint.scheme() != "https" {
            return Err(HostError::InvalidArgs(
                "worker endpoint must use http or https".to_string(),
            ));
        }
        if endpoint.scheme() == "http" && !is_private_worker_host(&endpoint) {
            return Err(HostError::InvalidArgs(
                "plain HTTP worker endpoints must use loopback or a Tailnet IPv4 address"
                    .to_string(),
            ));
        }
        if endpoint.query().is_some() || endpoint.fragment().is_some() || endpoint.path() != "/" {
            return Err(HostError::InvalidArgs(
                "worker endpoint must be an origin without path, query, or fragment".to_string(),
            ));
        }
        Ok(())
    }
}

fn is_private_worker_host(endpoint: &Url) -> bool {
    match endpoint.host() {
        Some(Host::Ipv4(address)) => {
            address.is_loopback()
                || (u32::from(address) & 0xffc0_0000) == u32::from_be_bytes([100, 64, 0, 0])
        }
        Some(Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    }
}

#[derive(Clone)]
pub struct RemoteWorkerConnector {
    config: RemoteWorkerConfig,
    key: SigningKey,
    client: reqwest::Client,
}

impl RemoteWorkerConnector {
    pub fn from_config(config: RemoteWorkerConfig) -> Result<Self, HostError> {
        config.validate()?;
        let key = SigningKey::from_hex_file(&config.signing_key_file)
            .map_err(|error| HostError::HostCall(error.to_string()))?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| HostError::HostCall(error.to_string()))?;
        Ok(Self {
            config,
            key,
            client,
        })
    }

    pub fn linked_aliases(&self) -> &[String] {
        &self.config.linked_aliases
    }

    pub fn linked_resource_handler(&self, artifacts: ArtifactStore) -> Arc<dyn ResourceHandler> {
        Arc::new(RemoteLinkedResourceHandler {
            client: Arc::new(RemoteWorkerClient {
                connector: self.clone(),
                artifacts,
            }),
        })
    }
}

impl SessionHostConnector for RemoteWorkerConnector {
    fn register(
        &self,
        host: &mut HostRegistry,
        resources: &mut ResourceRegistry,
        artifacts: ArtifactStore,
    ) -> tm_host::Result<()> {
        let client = Arc::new(RemoteWorkerClient {
            connector: self.clone(),
            artifacts,
        });
        for name in LINKED_CAPABILITIES {
            host.register(Arc::new(RemoteHostFn {
                docs: linked_tool_docs(name).ok_or_else(|| {
                    HostError::HostCall(format!("missing linked tool docs for {name}"))
                })?,
                client: Arc::clone(&client),
            }));
        }
        resources.register(self.linked_resource_handler(client.artifacts.clone()));
        Ok(())
    }
}

#[derive(Clone)]
struct RemoteWorkerClient {
    connector: RemoteWorkerConnector,
    artifacts: ArtifactStore,
}

impl RemoteWorkerClient {
    async fn execute(
        &self,
        operation: WorkerOperation,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Value> {
        let request = JobRequest {
            protocol_version: PROTOCOL_VERSION,
            job_id: Uuid::new_v4(),
            worker_id: self.connector.config.worker_id.clone(),
            operation,
            authority: WorkerAuthority {
                session_id: ctx.session_id.clone(),
                actor_id: ctx.actor_id.clone(),
                session_scope: ctx.session_scope.clone(),
                grants: ctx.grants.names().map(str::to_string).collect(),
            },
        };
        let body =
            serde_json::to_vec(&request).map_err(|error| HostError::HostCall(error.to_string()))?;
        let mut status: JobStatus = self.request_json(Method::POST, "/v1/jobs", body).await?;
        let mut guard = RemoteJobGuard {
            client: self.clone(),
            job_id: request.job_id,
            armed: true,
        };
        let deadline = tokio::time::Instant::now() + REMOTE_JOB_TIMEOUT;
        let mut resolved_action = None;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(HostError::Timeout(format!(
                    "remote worker job {}",
                    request.job_id
                )));
            }
            match status.state {
                JobState::Queued | JobState::Running => {}
                JobState::AwaitingApproval => {
                    let action = status.action.clone().ok_or_else(|| {
                        HostError::HostCall(
                            "worker omitted an awaiting approval action".to_string(),
                        )
                    })?;
                    let digest = status.action_sha256.clone().ok_or_else(|| {
                        HostError::HostCall(
                            "worker omitted an awaiting approval digest".to_string(),
                        )
                    })?;
                    if resolved_action.as_deref() != Some(digest.as_str()) {
                        let resolution = match ctx.require_approval(&action).await {
                            Ok(()) => ApprovalResolution::Approved,
                            Err(HostError::ApprovalDenied(_)) => ApprovalResolution::Denied,
                            Err(error) => return Err(error),
                        };
                        let approval = ResolveApprovalRequest {
                            protocol_version: PROTOCOL_VERSION,
                            action_sha256: digest.clone(),
                            resolution,
                        };
                        let body = serde_json::to_vec(&approval)
                            .map_err(|error| HostError::HostCall(error.to_string()))?;
                        let path = format!("/v1/jobs/{}/approval", request.job_id);
                        status = self.request_json(Method::POST, &path, body).await?;
                        resolved_action = Some(digest);
                        continue;
                    }
                }
                JobState::Succeeded => {
                    for event in &status.events {
                        ctx.emit_event(
                            &event.event_type,
                            serde_json::json!({
                                "workerId": self.connector.config.worker_id,
                                "jobId": request.job_id,
                                "payload": event.payload,
                            }),
                        )
                        .await?;
                    }
                    let mut value = status.result.take().ok_or_else(|| {
                        HostError::HostCall("worker succeeded without a result".to_string())
                    })?;
                    self.localize_artifacts(&mut value).await?;
                    guard.armed = false;
                    return Ok(value);
                }
                JobState::Failed => {
                    guard.armed = false;
                    return Err(status
                        .error
                        .as_ref()
                        .map(payload_to_error)
                        .unwrap_or_else(|| {
                            HostError::HostCall("worker failed without an error".to_string())
                        }));
                }
                JobState::Cancelled => {
                    guard.armed = false;
                    return Err(HostError::HostCall(
                        "remote worker job was cancelled".to_string(),
                    ));
                }
                JobState::Indeterminate => {
                    guard.armed = false;
                    return Err(HostError::HostCall(
                        "remote worker job is indeterminate and was not replayed".to_string(),
                    ));
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
            let path = format!("/v1/jobs/{}", request.job_id);
            status = self.request_json(Method::GET, &path, Vec::new()).await?;
        }
    }

    async fn cancel(&self, job_id: Uuid) {
        let path = format!("/v1/jobs/{job_id}");
        let _ = self
            .request_json::<JobStatus>(Method::DELETE, &path, Vec::new())
            .await;
    }

    async fn request_json<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Vec<u8>,
    ) -> tm_host::Result<T> {
        let endpoint = self.connector.config.endpoint.trim_end_matches('/');
        let url = format!("{endpoint}{path}");
        let auth = RequestAuth::new(
            &self.connector.key,
            method.as_str(),
            path,
            &body,
            current_unix_seconds(),
            Uuid::new_v4().to_string(),
        );
        let response = self
            .connector
            .client
            .request(method, url)
            .header("x-tm-worker-timestamp", auth.timestamp.to_string())
            .header("x-tm-worker-nonce", auth.nonce)
            .header("x-tm-worker-signature", auth.signature)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|error| HostError::HostCall(format!("remote worker unavailable: {error}")))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| HostError::HostCall(error.to_string()))?;
        if !status.is_success() {
            let message = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned());
            return Err(
                if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                    HostError::CapabilityDenied(format!(
                        "remote worker rejected request: {message}"
                    ))
                } else {
                    HostError::HostCall(format!("remote worker returned {status}: {message}"))
                },
            );
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            HostError::HostCall(format!("invalid remote worker response: {error}"))
        })
    }

    fn localize_artifacts<'a>(
        &'a self,
        value: &'a mut Value,
    ) -> Pin<Box<dyn Future<Output = tm_host::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let artifact_id = value.as_object().and_then(|object| {
                let uri = object.get("uri")?.as_str()?;
                let id = uri.strip_prefix("artifact://")?;
                (object.contains_key("size_bytes") || object.contains_key("sizeBytes"))
                    .then(|| id.to_string())
            });
            if let Some(id) = artifact_id {
                let remote: ArtifactRef =
                    serde_json::from_value(value.clone()).map_err(|error| {
                        HostError::HostCall(format!("invalid worker artifact: {error}"))
                    })?;
                let path = format!("/v1/artifacts/{id}");
                let payload: Value = self.request_json(Method::GET, &path, Vec::new()).await?;
                let content = payload
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        HostError::HostCall("worker artifact response omitted content".to_string())
                    })?;
                let expected = payload
                    .get("sha256")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        HostError::HostCall("worker artifact response omitted digest".to_string())
                    })?;
                let actual = hex::encode(Sha256::digest(content.as_bytes()));
                if actual != expected || content.len() != remote.size_bytes {
                    return Err(HostError::HostCall(
                        "worker artifact failed digest or size verification".to_string(),
                    ));
                }
                let local = self
                    .artifacts
                    .put_text(content, remote.title, &remote.mime)
                    .map_err(|error| HostError::HostCall(error.to_string()))?;
                *value = serde_json::to_value(local)
                    .map_err(|error| HostError::HostCall(error.to_string()))?;
                return Ok(());
            }
            match value {
                Value::Array(values) => {
                    for value in values {
                        self.localize_artifacts(value).await?;
                    }
                }
                Value::Object(values) => {
                    for value in values.values_mut() {
                        self.localize_artifacts(value).await?;
                    }
                }
                _ => {}
            }
            Ok(())
        })
    }
}

struct RemoteJobGuard {
    client: RemoteWorkerClient,
    job_id: Uuid,
    armed: bool,
}

impl Drop for RemoteJobGuard {
    fn drop(&mut self) {
        if self.armed
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let client = self.client.clone();
            let job_id = self.job_id;
            runtime.spawn(async move { client.cancel(job_id).await });
        }
    }
}

struct RemoteHostFn {
    docs: ToolDocs,
    client: Arc<RemoteWorkerClient>,
}

#[async_trait]
impl HostFn for RemoteHostFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        self.client
            .execute(
                WorkerOperation::Invoke {
                    capability: self.docs.name.clone(),
                    args,
                },
                ctx,
            )
            .await
    }
}

struct RemoteLinkedResourceHandler {
    client: Arc<RemoteWorkerClient>,
}

#[async_trait]
impl ResourceHandler for RemoteLinkedResourceHandler {
    fn scheme(&self) -> &str {
        "linked"
    }

    fn capability(&self) -> &str {
        "resources.read:linked"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<ResourceContent> {
        let value = self
            .client
            .execute(
                WorkerOperation::ResourceRead {
                    uri: uri.to_string(),
                    selector: selector.map(str::to_string),
                },
                ctx,
            )
            .await?;
        serde_json::from_value(value).map_err(|error| HostError::HostCall(error.to_string()))
    }

    async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> tm_host::Result<ResourceContent> {
        let value = self
            .client
            .execute(
                WorkerOperation::ResourcePreview {
                    uri: uri.to_string(),
                },
                ctx,
            )
            .await?;
        serde_json::from_value(value).map_err(|error| HostError::HostCall(error.to_string()))
    }

    async fn list(
        &self,
        uri: Option<&str>,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<Vec<ResourceEntry>> {
        let value = self
            .client
            .execute(
                WorkerOperation::ResourceList {
                    uri: uri.map(str::to_string),
                },
                ctx,
            )
            .await?;
        serde_json::from_value(value).map_err(|error| HostError::HostCall(error.to_string()))
    }
}

fn payload_to_error(payload: &tm_host::HostErrorPayload) -> HostError {
    match payload.name.as_str() {
        "CapabilityDeniedError" => HostError::CapabilityDenied(payload.message.clone()),
        "ApprovalDeniedError" => HostError::ApprovalDenied(payload.message.clone()),
        "ApprovalTimeoutError" => HostError::ApprovalTimeout(payload.message.clone()),
        "NotFoundError" => HostError::NotFound(payload.message.clone()),
        "InvalidArgsError" => HostError::InvalidArgs(payload.message.clone()),
        "InvalidPathError" => HostError::InvalidPath(payload.message.clone()),
        "NotImplementedError" => HostError::NotImplemented(payload.message.clone()),
        "QuotaExceededError" => HostError::QuotaExceeded(payload.message.clone()),
        "TimeoutError" => HostError::Timeout(payload.message.clone()),
        "OutputTruncatedError" => HostError::OutputTruncated(payload.message.clone()),
        _ => HostError::HostCall(payload.message.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(endpoint: &str) -> RemoteWorkerConfig {
        RemoteWorkerConfig {
            worker_id: "homolab".to_string(),
            endpoint: endpoint.to_string(),
            signing_key_file: PathBuf::from("/not/read-during-validation"),
            linked_aliases: vec!["repo".to_string()],
        }
    }

    #[test]
    fn every_remote_linked_capability_has_docs() {
        for name in LINKED_CAPABILITIES {
            assert!(
                linked_tool_docs(name).is_some(),
                "remote linked capability {name} must resolve exact docs"
            );
        }
    }

    #[test]
    fn plain_http_is_confined_to_loopback_or_tailnet() {
        config("http://127.0.0.1:18787").validate().unwrap();
        config("http://100.110.95.111:18787").validate().unwrap();
        assert!(config("http://192.168.1.8:18787").validate().is_err());
        assert!(
            config("http://worker.example.test:18787")
                .validate()
                .is_err()
        );
        config("https://worker.example.test").validate().unwrap();
    }
}

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    net::IpAddr,
    sync::{Arc, Mutex, RwLock as StdRwLock},
    time::{Duration, Instant},
};

use futures::StreamExt;
use reqwest::{
    Method,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tm_host::{
    CapabilityGrants, EgressConfig, EgressDestinationConfig, EgressSecretConfig,
    EgressSecretInjection, InvocationCtx, SecretHandle,
};
use tokio::sync::RwLock;
use zeroize::Zeroizing;

use crate::{
    DnsResolver, EgressBudgetRequest, EgressError, EgressMutationIntent, EgressMutationRecord,
    EgressMutationStatus, EgressStateStore, EgressUsageLimits, Result, SystemDnsResolver,
    VolatileEgressStateStore,
    policy::{
        AuthorizedDestination, MAX_SECRET_BYTES, ValidatedConfig, validate_config,
        validate_request_headers,
    },
    validate_resolved_addresses,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub auth: Option<SecretHandle>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    pub content_type: Option<String>,
    /// A deliberately tiny response-header view. The runtime never exposes cookies,
    /// authentication challenges, routing headers, or an unbounded peer header map.
    /// It is host-adapter metadata and is intentionally omitted from model-visible serialization.
    #[serde(skip)]
    pub headers: BTreeMap<String, String>,
    pub destination_id: String,
    pub response_bytes: usize,
    pub redirects: u8,
    pub secret_redactions: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EgressRuntimeOptions {
    /// Production must leave this false. It exists for deterministic loopback tests and explicit
    /// private deployments whose operator accepts the SSRF boundary.
    pub allow_http: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct TargetVersion {
    id: String,
    version: u64,
}

#[derive(Clone)]
struct RuntimeState {
    config: ValidatedConfig,
    revoked_destinations: BTreeSet<TargetVersion>,
    revoked_secrets: BTreeSet<TargetVersion>,
    generation: u64,
}

#[derive(Debug, Clone)]
struct HandleRecord {
    session_id: String,
    actor_id: Option<String>,
    secret_id: String,
    secret_version: u64,
    expires_at: Instant,
}

struct ResolvedSecret {
    config: EgressSecretConfig,
    value: Zeroizing<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SecretSnapshot {
    pub(crate) handle_digest: String,
    pub(crate) secret_id: String,
    pub(crate) secret_version: u64,
    expires_at: Instant,
}

#[derive(Clone)]
pub(crate) struct PreparedMutation {
    state: RuntimeState,
    destination: AuthorizedDestination,
    pub(crate) policy_generation: u64,
    pub(crate) destination_id: String,
    pub(crate) destination_version: u64,
    pub(crate) canonical_url: String,
    pub(crate) target_digest: String,
    pub(crate) request_digest: String,
    pub(crate) query_digest: String,
    pub(crate) request_bytes: usize,
    pub(crate) secret: Option<SecretSnapshot>,
}

impl PartialEq for PreparedMutation {
    fn eq(&self, other: &Self) -> bool {
        self.policy_generation == other.policy_generation
            && self.destination_id == other.destination_id
            && self.destination_version == other.destination_version
            && self.canonical_url == other.canonical_url
            && self.target_digest == other.target_digest
            && self.request_digest == other.request_digest
            && self.query_digest == other.query_digest
            && self.request_bytes == other.request_bytes
            && self.secret == other.secret
    }
}

impl Eq for PreparedMutation {}

pub(crate) enum MutationExecution {
    Execute(EgressMutationRecord),
    Replay(EgressMutationRecord),
}

#[derive(Clone)]
pub struct EgressRuntime {
    state: Arc<RwLock<RuntimeState>>,
    resolver: Arc<dyn DnsResolver>,
    handles: Arc<Mutex<HashMap<String, HandleRecord>>>,
    durable: Arc<StdRwLock<Arc<dyn EgressStateStore>>>,
    options: EgressRuntimeOptions,
}

/// A transport-free operator handle for atomic policy replacement, emergency revocation, and
/// session teardown. Keeping this separate from [`EgressRuntime`] lets production supervision
/// retain administrative control without exposing an HTTP request primitive.
#[derive(Clone)]
pub struct EgressAdmin {
    runtime: EgressRuntime,
}

impl std::fmt::Debug for EgressAdmin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EgressAdmin")
            .finish_non_exhaustive()
    }
}

/// One session can create handles for multiple delegated actors, but cannot grow the broker
/// without bound. Repeated use by the same session/actor/secret tuple reuses one opaque handle.
pub const MAX_SECRET_HANDLES_PER_SESSION: usize = 64;
const MAX_SECRET_HANDLES_TOTAL: usize = 4_096;
const SECRET_HANDLE_TTL: Duration = Duration::from_secs(30 * 60);

impl std::fmt::Debug for EgressRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EgressRuntime")
            .finish_non_exhaustive()
    }
}

impl EgressRuntime {
    pub fn new(config: EgressConfig) -> Result<Self> {
        Self::with_resolver_and_options(
            config,
            Arc::new(SystemDnsResolver),
            EgressRuntimeOptions::default(),
        )
    }

    pub fn with_resolver(config: EgressConfig, resolver: Arc<dyn DnsResolver>) -> Result<Self> {
        Self::with_resolver_and_options(config, resolver, EgressRuntimeOptions::default())
    }

    /// Deterministic loopback constructor for downstream crate tests. Production configurations
    /// must use [`Self::new`] or [`Self::with_resolver`], both of which require HTTPS.
    #[cfg(feature = "test-http")]
    #[doc(hidden)]
    pub fn with_test_http_resolver(
        config: EgressConfig,
        resolver: Arc<dyn DnsResolver>,
    ) -> Result<Self> {
        Self::with_resolver_and_options(config, resolver, EgressRuntimeOptions { allow_http: true })
    }

    pub(crate) fn with_resolver_and_options(
        config: EgressConfig,
        resolver: Arc<dyn DnsResolver>,
        options: EgressRuntimeOptions,
    ) -> Result<Self> {
        let config = validate_config(config, options.allow_http)?;
        Ok(Self {
            state: Arc::new(RwLock::new(RuntimeState {
                config,
                revoked_destinations: BTreeSet::new(),
                revoked_secrets: BTreeSet::new(),
                generation: 1,
            })),
            resolver,
            handles: Arc::new(Mutex::new(HashMap::new())),
            durable: Arc::new(StdRwLock::new(
                Arc::new(VolatileEgressStateStore::default()),
            )),
            options,
        })
    }

    pub fn admin_handle(&self) -> EgressAdmin {
        EgressAdmin {
            runtime: self.clone(),
        }
    }

    /// Atomically replaces policy. Revocations survive replacement for unchanged id/version
    /// pairs; rotating to a new explicit version is the only way replacement re-authorizes one.
    pub async fn replace(&self, config: EgressConfig) -> Result<()> {
        let config = validate_config(config, self.options.allow_http)?;
        let mut state = self.state.write().await;
        state.revoked_destinations.retain(|target| {
            config
                .destinations
                .get(&target.id)
                .is_some_and(|item| item.version == target.version)
        });
        state.revoked_secrets.retain(|target| {
            config
                .secrets
                .get(&target.id)
                .is_some_and(|item| item.version == target.version)
        });
        state.config = config;
        state.generation = state.generation.saturating_add(1);
        let current_secrets = &state.config.secrets;
        self.handles
            .lock()
            .expect("secret handle lock poisoned")
            .retain(|_, handle| {
                handle.expires_at > Instant::now()
                    && current_secrets
                        .get(&handle.secret_id)
                        .is_some_and(|secret| secret.version == handle.secret_version)
            });
        Ok(())
    }

    pub async fn revoke_destination(&self, id: &str) -> Result<()> {
        let mut state = self.state.write().await;
        let destination = state
            .config
            .destinations
            .get(id)
            .ok_or_else(|| EgressError::InvalidRequest("unknown destination".into()))?;
        let target = TargetVersion {
            id: id.to_string(),
            version: destination.version,
        };
        state.revoked_destinations.insert(target);
        Ok(())
    }

    pub async fn revoke_secret(&self, id: &str) -> Result<()> {
        let mut state = self.state.write().await;
        let secret = state
            .config
            .secrets
            .get(id)
            .ok_or_else(|| EgressError::InvalidRequest("unknown secret".into()))?;
        let target = TargetVersion {
            id: id.to_string(),
            version: secret.version,
        };
        state.revoked_secrets.insert(target);
        Ok(())
    }

    pub async fn policy_generation(&self) -> u64 {
        self.state.read().await.generation
    }

    pub async fn issue_secret_handle(
        &self,
        ctx: &InvocationCtx,
        secret_id: &str,
    ) -> Result<SecretHandle> {
        require_session(ctx)?;
        let state = {
            let guard = self.state.read().await;
            (*guard).clone()
        };
        if !state.config.enabled {
            return Err(EgressError::Disabled);
        }
        require_exact_grant(&ctx.grants, &format!("secrets.use:{secret_id}"))?;
        let secret = state
            .config
            .secrets
            .get(secret_id)
            .ok_or(EgressError::SecretUnavailable)?;
        ensure_secret_not_revoked(&state, secret)?;
        let now = Instant::now();
        let (token, reused) = {
            let mut handles = self.handles.lock().expect("secret handle lock poisoned");
            handles.retain(|_, handle| handle.expires_at > now);
            if let Some((token, _)) = handles.iter().find(|(_, handle)| {
                handle.session_id == ctx.session_id
                    && handle.actor_id == ctx.actor_id
                    && handle.secret_id == secret.id
                    && handle.secret_version == secret.version
            }) {
                (token.clone(), true)
            } else {
                let session_handles = handles
                    .values()
                    .filter(|handle| handle.session_id == ctx.session_id)
                    .count();
                if session_handles >= MAX_SECRET_HANDLES_PER_SESSION {
                    return Err(EgressError::Budget("per-session secret handle cap".into()));
                }
                if handles.len() >= MAX_SECRET_HANDLES_TOTAL {
                    return Err(EgressError::Budget("process secret handle cap".into()));
                }
                let token = random_token(32)?;
                handles.insert(
                    token.clone(),
                    HandleRecord {
                        session_id: ctx.session_id.clone(),
                        actor_id: ctx.actor_id.clone(),
                        secret_id: secret.id.clone(),
                        secret_version: secret.version,
                        expires_at: now + SECRET_HANDLE_TTL,
                    },
                );
                (token, false)
            }
        };
        ctx.emit_event(
            "secret_handle_issued",
            json!({
                "secretId": secret.id,
                "secretVersion": secret.version,
                "destinationCount": secret.destinations.len(),
                "opaque": true,
                "reused": reused,
            }),
        )
        .await
        .map_err(|_| EgressError::Audit)?;
        Ok(SecretHandle { token })
    }

    pub async fn execute(&self, ctx: &InvocationCtx, request: HttpRequest) -> Result<HttpResponse> {
        self.execute_scoped(ctx, None, request).await
    }

    /// Execute only if the URL resolves to the exact configured destination id. This is used by
    /// host-owned protocol adapters so a configuration mistake cannot silently route a named
    /// server through some other destination that the same invocation also happens to hold.
    pub async fn execute_for_destination(
        &self,
        ctx: &InvocationCtx,
        destination_id: &str,
        request: HttpRequest,
    ) -> Result<HttpResponse> {
        self.execute_scoped(ctx, Some(destination_id), request)
            .await
    }

    /// Resolve and bind all policy and opaque-handle metadata needed for a mutation approval.
    /// This deliberately does not read the secret value. Callers compare two snapshots around the
    /// manual approval boundary, then claim the durable effect before any DNS or transport work.
    pub(crate) async fn prepare_mutation(
        &self,
        ctx: &InvocationCtx,
        request: &HttpRequest,
    ) -> Result<PreparedMutation> {
        let result = self.prepare_mutation_snapshot(ctx, request).await;
        if let Err(error) = &result {
            let audit_id = random_token(16)?;
            let generation = self.state.read().await.generation;
            emit_denial(
                ctx,
                &audit_id,
                &request.method.to_ascii_uppercase(),
                None,
                generation,
                error,
            )
            .await?;
        }
        result
    }

    async fn prepare_mutation_snapshot(
        &self,
        ctx: &InvocationCtx,
        request: &HttpRequest,
    ) -> Result<PreparedMutation> {
        require_session(ctx)?;
        let method = request.method.to_ascii_uppercase();
        if method == "GET" {
            return Err(EgressError::InvalidRequest(
                "GET does not use the mutation boundary".into(),
            ));
        }
        let state = {
            let guard = self.state.read().await;
            (*guard).clone()
        };
        let destination = state.config.authorize(&request.url, &method)?;
        authorize_destination(ctx, &state, &destination.policy)?;
        validate_request_headers(&destination.policy, &request.headers)?;
        let canonical_headers = canonical_request_headers(&request.headers)?;
        let secret_config = match &request.auth {
            Some(handle) => Some(resolve_secret_snapshot(
                ctx,
                &state,
                &self.handles,
                handle,
                &destination.policy,
            )?),
            None => None,
        };
        let secret = secret_config.as_ref().map(|(snapshot, _)| snapshot.clone());
        let target_digest = sha256_json(&json!({
            "destination": destination.policy,
            "secret": secret_config.as_ref().map(|(_, config)| config),
        }))?;
        let canonical_url = destination.url.as_str().to_string();
        let query_digest = sha256_domain(
            "tm.egress.query.v1",
            destination.url.query().unwrap_or("").as_bytes(),
        );
        let body = request.body.as_deref().unwrap_or("");
        let body_digest = sha256_domain("tm.egress.body.v1", body.as_bytes());
        let request_digest = sha256_json(&json!({
            "version": 1,
            "method": method,
            "url": canonical_url,
            "headers": canonical_headers,
            "bodyBytes": body.len(),
            "bodyDigest": body_digest,
            "timeoutMs": request.timeout_ms,
            "secretId": secret.as_ref().map(|item| item.secret_id.as_str()),
            "secretVersion": secret.as_ref().map(|item| item.secret_version),
            "secretHandleDigest": secret.as_ref().map(|item| item.handle_digest.as_str()),
            "targetDigest": target_digest,
        }))?;
        let request_bytes = request_size_without_secret_value(request, &canonical_url)?;
        Ok(PreparedMutation {
            policy_generation: state.generation,
            destination_id: destination.policy.id.clone(),
            destination_version: destination.policy.version,
            canonical_url,
            target_digest,
            request_digest,
            query_digest,
            request_bytes,
            secret,
            state,
            destination,
        })
    }

    pub(crate) async fn begin_mutation(
        &self,
        ctx: &InvocationCtx,
        prepared: &PreparedMutation,
    ) -> Result<MutationExecution> {
        let effect_scope_id = ctx.events.effect_scope_id().ok_or_else(|| {
            EgressError::Durability(
                "HTTP mutation requires a host-owned durable effect scope".into(),
            )
        })?;
        let session_digest = sha256_domain("tm.egress.session.v1", ctx.session_id.as_bytes());
        let actor_digest = sha256_domain(
            "tm.egress.actor.v1",
            ctx.actor_id.as_deref().unwrap_or("root").as_bytes(),
        );
        let effect_id = sha256_json(&json!({
            "version": 1,
            "effectScopeId": effect_scope_id,
            "sessionDigest": session_digest,
            "actorDigest": actor_digest,
            "destinationId": prepared.destination_id,
            "destinationVersion": prepared.destination_version,
            "targetDigest": prepared.target_digest,
            "requestDigest": prepared.request_digest,
        }))?;
        let intent = EgressMutationIntent {
            effect_id,
            session_id: ctx.session_id.clone(),
            effect_scope_id,
            session_digest,
            actor_digest,
            destination_id: prepared.destination_id.clone(),
            destination_version: prepared.destination_version,
            target_digest: prepared.target_digest.clone(),
            request_digest: prepared.request_digest.clone(),
            request_bytes: prepared.request_bytes,
        };
        let durable = self
            .durable
            .read()
            .expect("egress durable state lock poisoned")
            .clone();
        let claim = durable.begin_mutation(intent).await?;
        if claim.created {
            return Ok(MutationExecution::Execute(claim.record));
        }
        let record = if claim.record.status == EgressMutationStatus::Started {
            durable
                .finish_mutation(
                    &claim.record.intent.effect_id,
                    EgressMutationStatus::Uncertain,
                    None,
                    None,
                    Some("interrupted_before_terminal_persistence"),
                    None,
                )
                .await?
        } else {
            claim.record
        };
        Ok(MutationExecution::Replay(record))
    }

    pub(crate) async fn execute_prepared_mutation(
        &self,
        ctx: &InvocationCtx,
        request: HttpRequest,
        prepared: PreparedMutation,
        effect: &EgressMutationRecord,
    ) -> Result<HttpResponse> {
        let audit_id = effect.intent.effect_id.chars().take(32).collect::<String>();
        let started = Instant::now();
        let method = request.method.to_ascii_uppercase();
        let secret = match (&request.auth, &prepared.secret) {
            (Some(handle), Some(expected)) => resolve_secret_snapshot(
                ctx,
                &prepared.state,
                &self.handles,
                handle,
                &prepared.destination.policy,
            )
            .and_then(|(current, _)| {
                if &current != expected {
                    return Err(EgressError::InvalidSecretHandle);
                }
                resolve_secret(
                    ctx,
                    &prepared.state,
                    &self.handles,
                    handle,
                    &prepared.destination.policy,
                )
            })
            .map(Some),
            (None, None) => Ok(None),
            _ => Err(EgressError::InvalidSecretHandle),
        };
        let secret = match secret {
            Ok(secret) => secret,
            Err(error) => {
                self.finish_mutation_error(effect, &error).await?;
                return Err(error);
            }
        };
        let request_bytes =
            request_size_for_url(&request, prepared.destination.url.as_str(), secret.as_ref());
        let request_bytes = match request_bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                self.finish_mutation_error(effect, &error).await?;
                return Err(error);
            }
        };
        if ctx
            .emit_event(
                "egress_started",
                json!({
                    "auditId": audit_id,
                    "effectId": effect.intent.effect_id,
                    "destinationId": prepared.destination_id,
                    "destinationVersion": prepared.destination_version,
                    "policyGeneration": prepared.policy_generation,
                    "method": method,
                    "requestBytes": request_bytes,
                    "usesSecret": secret.is_some(),
                }),
            )
            .await
            .is_err()
        {
            let error = EgressError::Audit;
            self.finish_mutation_error(effect, &error).await?;
            return Err(error);
        }
        let result = self
            .execute_hops(
                ctx,
                &prepared.state,
                Some(&prepared.destination_id),
                request,
                prepared.destination,
                secret.as_ref(),
            )
            .await;
        let elapsed_ms = duration_ms(started.elapsed());
        match result {
            Ok(response) => {
                self.finish_mutation_response(effect, &response).await?;
                ctx.emit_event(
                    "egress_completed",
                    json!({
                        "auditId": audit_id,
                        "effectId": effect.intent.effect_id,
                        "destinationId": response.destination_id,
                        "policyGeneration": prepared.policy_generation,
                        "method": method,
                        "status": response.status,
                        "responseBytes": response.response_bytes,
                        "redirects": response.redirects,
                        "secretRedactions": response.secret_redactions,
                        "elapsedMs": elapsed_ms,
                    }),
                )
                .await
                .map_err(|_| EgressError::Audit)?;
                Ok(response)
            }
            Err(error) => {
                self.finish_mutation_error(effect, &error).await?;
                ctx.emit_event(
                    "egress_failed",
                    json!({
                        "auditId": audit_id,
                        "effectId": effect.intent.effect_id,
                        "destinationId": prepared.destination_id,
                        "policyGeneration": prepared.policy_generation,
                        "method": method,
                        "errorCode": error.code(),
                        "elapsedMs": elapsed_ms,
                    }),
                )
                .await
                .map_err(|_| EgressError::Audit)?;
                Err(error)
            }
        }
    }

    async fn finish_mutation_response(
        &self,
        effect: &EgressMutationRecord,
        response: &HttpResponse,
    ) -> Result<()> {
        let result_digest = sha256_json(&json!({
            "status": response.status,
            "bodyDigest": sha256_domain("tm.egress.response-body.v1", response.body.as_bytes()),
            "contentType": response.content_type,
            "destinationId": response.destination_id,
            "responseBytes": response.response_bytes,
            "redirects": response.redirects,
            "secretRedactions": response.secret_redactions,
        }))?;
        let (status, error_code, error_digest) = if response.status < 400 {
            (EgressMutationStatus::Succeeded, None, None)
        } else {
            (
                EgressMutationStatus::Failed,
                Some("http_error_status"),
                Some(sha256_domain(
                    "tm.egress.http-status.v1",
                    response.status.to_string().as_bytes(),
                )),
            )
        };
        let durable = self
            .durable
            .read()
            .expect("egress durable state lock poisoned")
            .clone();
        durable
            .finish_mutation(
                &effect.intent.effect_id,
                status,
                Some(&result_digest),
                Some(response.response_bytes),
                error_code,
                error_digest.as_deref(),
            )
            .await?;
        Ok(())
    }

    async fn finish_mutation_error(
        &self,
        effect: &EgressMutationRecord,
        error: &EgressError,
    ) -> Result<()> {
        let status = if matches!(
            error,
            EgressError::Transport
                | EgressError::Timeout
                | EgressError::Audit
                | EgressError::Durability(_)
        ) {
            EgressMutationStatus::Uncertain
        } else {
            EgressMutationStatus::Failed
        };
        let error_digest = sha256_domain("tm.egress.error.v1", error.code().as_bytes());
        let durable = self
            .durable
            .read()
            .expect("egress durable state lock poisoned")
            .clone();
        durable
            .finish_mutation(
                &effect.intent.effect_id,
                status,
                None,
                None,
                Some(error.code()),
                Some(&error_digest),
            )
            .await?;
        Ok(())
    }

    async fn execute_scoped(
        &self,
        ctx: &InvocationCtx,
        expected_destination_id: Option<&str>,
        request: HttpRequest,
    ) -> Result<HttpResponse> {
        let audit_id = random_token(16)?;
        let started = Instant::now();
        let method = request.method.to_ascii_uppercase();
        if let Err(error) = require_session(ctx) {
            emit_denial(ctx, &audit_id, &method, None, 0, &error).await?;
            return Err(error);
        }
        // Take an immutable policy snapshot before DNS or transport work. This keeps one request
        // internally consistent while allowing an operator revocation/reload to complete without
        // waiting for a slow peer. The next request observes the new generation immediately.
        let state = {
            let guard = self.state.read().await;
            (*guard).clone()
        };
        let generation = state.generation;
        let initial = match state.config.authorize(&request.url, &method) {
            Ok(destination) => destination,
            Err(error) => {
                emit_denial(ctx, &audit_id, &method, None, generation, &error).await?;
                return Err(error);
            }
        };
        if expected_destination_id.is_some_and(|expected| initial.policy.id != expected) {
            let error = EgressError::Denied("unexpected destination for scoped request".into());
            emit_denial(
                ctx,
                &audit_id,
                &method,
                Some(&initial.policy.id),
                generation,
                &error,
            )
            .await?;
            return Err(error);
        }
        if let Err(error) = authorize_destination(ctx, &state, &initial.policy) {
            emit_denial(
                ctx,
                &audit_id,
                &method,
                Some(&initial.policy.id),
                generation,
                &error,
            )
            .await?;
            return Err(error);
        }
        if let Err(error) = validate_request_headers(&initial.policy, &request.headers) {
            emit_denial(
                ctx,
                &audit_id,
                &method,
                Some(&initial.policy.id),
                generation,
                &error,
            )
            .await?;
            return Err(error);
        }
        if method == "GET" && request.body.as_ref().is_some_and(|body| !body.is_empty()) {
            let error = EgressError::InvalidRequest("GET requests cannot carry a body".into());
            emit_denial(
                ctx,
                &audit_id,
                &method,
                Some(&initial.policy.id),
                generation,
                &error,
            )
            .await?;
            return Err(error);
        }
        let resolved_secret = match &request.auth {
            Some(handle) => {
                match resolve_secret(ctx, &state, &self.handles, handle, &initial.policy) {
                    Ok(secret) => Some(secret),
                    Err(error) => {
                        emit_denial(
                            ctx,
                            &audit_id,
                            &method,
                            Some(&initial.policy.id),
                            generation,
                            &error,
                        )
                        .await?;
                        return Err(error);
                    }
                }
            }
            None => None,
        };
        let request_bytes =
            match request_size_for_url(&request, initial.url.as_str(), resolved_secret.as_ref()) {
                Ok(bytes) => bytes,
                Err(error) => {
                    emit_denial(
                        ctx,
                        &audit_id,
                        &method,
                        Some(&initial.policy.id),
                        generation,
                        &error,
                    )
                    .await?;
                    return Err(error);
                }
            };
        let initial_destination_id = initial.policy.id.clone();
        ctx.emit_event(
            "egress_started",
            json!({
                "auditId": audit_id,
                "destinationId": initial.policy.id,
                "destinationVersion": initial.policy.version,
                "policyGeneration": generation,
                "method": method,
                "requestBytes": request_bytes,
                "usesSecret": resolved_secret.is_some(),
            }),
        )
        .await
        .map_err(|_| EgressError::Audit)?;

        let result = self
            .execute_hops(
                ctx,
                &state,
                expected_destination_id,
                request,
                initial,
                resolved_secret.as_ref(),
            )
            .await;
        let elapsed_ms = duration_ms(started.elapsed());
        match result {
            Ok(response) => {
                ctx.emit_event(
                    "egress_completed",
                    json!({
                        "auditId": audit_id,
                        "destinationId": response.destination_id,
                        "policyGeneration": generation,
                        "method": method,
                        "status": response.status,
                        "responseBytes": response.response_bytes,
                        "redirects": response.redirects,
                        "secretRedactions": response.secret_redactions,
                        "elapsedMs": elapsed_ms,
                    }),
                )
                .await
                .map_err(|_| EgressError::Audit)?;
                Ok(response)
            }
            Err(error) => {
                ctx.emit_event(
                    "egress_failed",
                    json!({
                        "auditId": audit_id,
                        "destinationId": initial_destination_id,
                        "policyGeneration": generation,
                        "method": method,
                        "errorCode": error.code(),
                        "elapsedMs": elapsed_ms,
                    }),
                )
                .await
                .map_err(|_| EgressError::Audit)?;
                Err(error)
            }
        }
    }

    pub async fn clear_session(&self, session_id: &str) -> Result<()> {
        self.handles
            .lock()
            .expect("secret handle lock poisoned")
            .retain(|_, handle| handle.session_id != session_id);
        let durable = self
            .durable
            .read()
            .expect("egress durable state lock poisoned")
            .clone();
        durable.clear_session(session_id).await
    }

    async fn execute_hops(
        &self,
        ctx: &InvocationCtx,
        state: &RuntimeState,
        expected_destination_id: Option<&str>,
        request: HttpRequest,
        mut destination: AuthorizedDestination,
        secret: Option<&ResolvedSecret>,
    ) -> Result<HttpResponse> {
        let method = Method::from_bytes(request.method.to_ascii_uppercase().as_bytes())
            .map_err(|_| EgressError::InvalidRequest("invalid HTTP method".into()))?;
        let redirect_cap = destination.policy.max_redirects;
        let mut redirects = 0u8;
        loop {
            authorize_destination(ctx, state, &destination.policy)?;
            if let Some(secret) = secret {
                authorize_secret_for_destination(ctx, state, secret, &destination.policy)?;
            }
            let request_bytes = request_size_for_url(&request, destination.url.as_str(), secret)?;
            if request_bytes > destination.policy.max_request_bytes {
                return Err(EgressError::Budget("request byte cap".into()));
            }
            let timeout_ms = request
                .timeout_ms
                .unwrap_or(destination.policy.request_timeout_ms);
            if timeout_ms == 0 || timeout_ms > destination.policy.request_timeout_ms {
                return Err(EgressError::Budget("request timeout cap".into()));
            }
            let durable = self
                .durable
                .read()
                .expect("egress durable state lock poisoned")
                .clone();
            let reservation = durable
                .reserve_budget(budget_request(
                    ctx,
                    &state.config.session_limits,
                    &destination.policy,
                    request_bytes,
                    timeout_ms,
                )?)
                .await?;
            let hop_started = Instant::now();
            let hop = tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                self.perform_hop(&destination, &method, &request, secret),
            )
            .await;
            let elapsed_ms = duration_ms(hop_started.elapsed());
            let hop = match hop {
                Ok(result) => result,
                Err(_) => Err(EgressError::Timeout),
            };
            match hop {
                Ok(HopResponse::Redirect { location }) => {
                    durable.settle_budget(reservation, 0, elapsed_ms).await?;
                    if method != Method::GET && method != Method::HEAD {
                        return Err(EgressError::Denied(
                            "non-GET redirects are forbidden".into(),
                        ));
                    }
                    if redirects >= redirect_cap {
                        return Err(EgressError::Denied("redirect cap exceeded".into()));
                    }
                    let next_url = destination
                        .url
                        .join(&location)
                        .map_err(|_| EgressError::Denied("invalid redirect location".into()))?;
                    let next = state.config.authorize(next_url.as_str(), method.as_str())?;
                    if expected_destination_id.is_some_and(|expected| next.policy.id != expected) {
                        return Err(EgressError::Denied(
                            "redirect escaped the scoped destination".into(),
                        ));
                    }
                    if next.policy.id != destination.policy.id
                        && !destination.policy.redirect_to.contains(&next.policy.id)
                    {
                        return Err(EgressError::Denied(
                            "redirect destination is not allowlisted by the current hop".into(),
                        ));
                    }
                    destination = next;
                    redirects = redirects.saturating_add(1);
                }
                Ok(HopResponse::Final {
                    status,
                    body,
                    content_type,
                    headers,
                    response_bytes,
                    secret_redactions,
                }) => {
                    durable
                        .settle_budget(
                            reservation,
                            u64::try_from(response_bytes).map_err(|_| {
                                EgressError::Budget("response size overflow".into())
                            })?,
                            elapsed_ms,
                        )
                        .await?;
                    return Ok(HttpResponse {
                        status,
                        body,
                        content_type,
                        headers,
                        destination_id: destination.policy.id,
                        response_bytes,
                        redirects,
                        secret_redactions,
                    });
                }
                Err(error) => {
                    durable.settle_budget(reservation, 0, elapsed_ms).await?;
                    return Err(error);
                }
            }
        }
    }

    async fn perform_hop(
        &self,
        destination: &AuthorizedDestination,
        method: &Method,
        request: &HttpRequest,
        secret: Option<&ResolvedSecret>,
    ) -> Result<HopResponse> {
        let addresses = self
            .resolver
            .resolve(&destination.policy.host, destination.policy.port)
            .await?;
        let addresses = validate_resolved_addresses(
            &addresses,
            destination.policy.port,
            destination.policy.allow_private_ips,
        )?;
        let mut builder = reqwest::Client::builder()
            // Egress policy, pinned DNS, and SSRF checks are the only routing authority. Never
            // inherit HTTP(S)_PROXY / ALL_PROXY from the service environment.
            .no_proxy()
            .connect_timeout(Duration::from_millis(destination.policy.connect_timeout_ms))
            .redirect(reqwest::redirect::Policy::none());
        if destination.policy.host.parse::<IpAddr>().is_err() {
            builder = builder.resolve_to_addrs(&destination.policy.host, &addresses);
        }
        let client = builder.build().map_err(|_| EgressError::Transport)?;
        let headers = request_headers(&destination.policy, &request.headers, secret)?;
        let mut outgoing = client
            .request(method.clone(), destination.url.clone())
            .headers(headers);
        if let Some(body) = &request.body {
            outgoing = outgoing.body(body.clone());
        }
        let response = outgoing.send().await.map_err(|_| EgressError::Transport)?;
        let status = response.status();
        if status.is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| EgressError::Denied("redirect omitted a valid Location".into()))?
                .to_string();
            return Ok(HopResponse::Redirect { location });
        }
        if let Some(length) = response.content_length()
            && length > destination.policy.max_response_bytes as u64
        {
            return Err(EgressError::Budget("per-request response-byte cap".into()));
        }
        let mut content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.chars().take(256).collect::<String>());
        let mut exposed_headers = exposed_response_headers(response.headers())?;
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| EgressError::Transport)?;
            if bytes.len().saturating_add(chunk.len()) > destination.policy.max_response_bytes {
                return Err(EgressError::Budget("per-request response-byte cap".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        let response_bytes = bytes.len();
        let mut body = String::from_utf8(bytes).map_err(|_| EgressError::NonUtf8)?;
        let mut redactions = 0usize;
        if let Some(secret) = secret {
            redactions = redactions.saturating_add(redact_exact(&mut body, &secret.value));
            if let Some(value) = &mut content_type {
                redactions = redactions.saturating_add(redact_exact(value, &secret.value));
            }
            for value in exposed_headers.values_mut() {
                redactions = redactions.saturating_add(redact_exact(value, &secret.value));
            }
        }
        Ok(HopResponse::Final {
            status: status.as_u16(),
            body,
            content_type,
            headers: exposed_headers,
            response_bytes,
            secret_redactions: redactions,
        })
    }
}

enum HopResponse {
    Redirect {
        location: String,
    },
    Final {
        status: u16,
        body: String,
        content_type: Option<String>,
        headers: BTreeMap<String, String>,
        response_bytes: usize,
        secret_redactions: usize,
    },
}

const EXPOSED_RESPONSE_HEADERS: &[&str] = &["mcp-session-id"];
const MAX_EXPOSED_RESPONSE_HEADER_VALUE_BYTES: usize = 1024;
const MAX_EXPOSED_RESPONSE_HEADER_BYTES: usize = 4 * 1024;

fn exposed_response_headers(headers: &HeaderMap) -> Result<BTreeMap<String, String>> {
    let mut exposed = BTreeMap::new();
    let mut total_bytes = 0usize;
    for name in EXPOSED_RESPONSE_HEADERS {
        let values = headers.get_all(*name).iter().collect::<Vec<_>>();
        if values.is_empty() {
            continue;
        }
        if values.len() != 1 {
            return Err(EgressError::Denied(
                "ambiguous exposed response header".into(),
            ));
        }
        let value = values[0]
            .to_str()
            .map_err(|_| EgressError::Denied("invalid exposed response header".into()))?;
        if value.len() > MAX_EXPOSED_RESPONSE_HEADER_VALUE_BYTES
            || value.chars().any(|character| character.is_control())
        {
            return Err(EgressError::Denied(
                "exposed response header exceeds bounds".into(),
            ));
        }
        total_bytes = total_bytes
            .checked_add(name.len())
            .and_then(|bytes| bytes.checked_add(value.len()))
            .ok_or_else(|| EgressError::Denied("response header size overflow".into()))?;
        if total_bytes > MAX_EXPOSED_RESPONSE_HEADER_BYTES {
            return Err(EgressError::Denied(
                "exposed response headers exceed bounds".into(),
            ));
        }
        exposed.insert((*name).to_string(), value.to_string());
    }
    Ok(exposed)
}

fn resolve_secret(
    ctx: &InvocationCtx,
    state: &RuntimeState,
    handles: &Mutex<HashMap<String, HandleRecord>>,
    handle: &SecretHandle,
    destination: &EgressDestinationConfig,
) -> Result<ResolvedSecret> {
    let (_, secret) = resolve_secret_snapshot(ctx, state, handles, handle, destination)?;
    let value = std::env::var(&secret.env).map_err(|_| EgressError::SecretUnavailable)?;
    if value.is_empty()
        || value.len() > MAX_SECRET_BYTES
        || value
            .chars()
            .any(|character| matches!(character, '\r' | '\n' | '\0'))
    {
        return Err(EgressError::SecretUnavailable);
    }
    Ok(ResolvedSecret {
        config: secret,
        value: Zeroizing::new(value),
    })
}

fn resolve_secret_snapshot(
    ctx: &InvocationCtx,
    state: &RuntimeState,
    handles: &Mutex<HashMap<String, HandleRecord>>,
    handle: &SecretHandle,
    destination: &EgressDestinationConfig,
) -> Result<(SecretSnapshot, EgressSecretConfig)> {
    let record = {
        let mut handles = handles.lock().expect("secret handle lock poisoned");
        let now = Instant::now();
        handles.retain(|_, handle| handle.expires_at > now);
        handles
            .get(&handle.token)
            .cloned()
            .ok_or(EgressError::InvalidSecretHandle)?
    };
    if record.session_id != ctx.session_id || record.actor_id != ctx.actor_id {
        return Err(EgressError::InvalidSecretHandle);
    }
    let secret = state
        .config
        .secrets
        .get(&record.secret_id)
        .filter(|secret| secret.version == record.secret_version)
        .ok_or(EgressError::InvalidSecretHandle)?;
    ensure_secret_not_revoked(state, secret)?;
    authorize_secret_for_config(ctx, secret, destination)?;
    Ok((
        SecretSnapshot {
            handle_digest: sha256_domain("tm.egress.secret-handle.v1", handle.token.as_bytes()),
            secret_id: secret.id.clone(),
            secret_version: secret.version,
            expires_at: record.expires_at,
        },
        secret.clone(),
    ))
}

impl EgressAdmin {
    /// Atomically validate and replace policy. The old generation remains active on failure.
    pub async fn replace(&self, config: EgressConfig) -> Result<()> {
        self.runtime.replace(config).await
    }

    pub async fn revoke_destination(&self, id: &str) -> Result<()> {
        self.runtime.revoke_destination(id).await
    }

    pub async fn revoke_secret(&self, id: &str) -> Result<()> {
        self.runtime.revoke_secret(id).await
    }

    pub async fn policy_generation(&self) -> u64 {
        self.runtime.policy_generation().await
    }

    pub fn install_state_store(&self, store: Arc<dyn EgressStateStore>) {
        *self
            .runtime
            .durable
            .write()
            .expect("egress durable state lock poisoned") = store;
    }

    pub async fn clear_session(&self, session_id: &str) -> Result<()> {
        self.runtime.clear_session(session_id).await
    }
}

fn authorize_secret_for_destination(
    ctx: &InvocationCtx,
    state: &RuntimeState,
    secret: &ResolvedSecret,
    destination: &EgressDestinationConfig,
) -> Result<()> {
    ensure_secret_not_revoked(state, &secret.config)?;
    authorize_secret_for_config(ctx, &secret.config, destination)
}

fn authorize_secret_for_config(
    ctx: &InvocationCtx,
    secret: &EgressSecretConfig,
    destination: &EgressDestinationConfig,
) -> Result<()> {
    require_exact_grant(&ctx.grants, &format!("secrets.use:{}", secret.id))?;
    if !secret.destinations.contains(&destination.id) {
        return Err(EgressError::InvalidSecretHandle);
    }
    Ok(())
}

fn request_headers(
    destination: &EgressDestinationConfig,
    caller: &BTreeMap<String, String>,
    secret: Option<&ResolvedSecret>,
) -> Result<HeaderMap> {
    validate_request_headers(destination, caller)?;
    let mut headers = HeaderMap::new();
    for (name, value) in caller {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| EgressError::InvalidRequest("invalid request header".into()))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| EgressError::InvalidRequest("invalid request header value".into()))?;
        headers.insert(name, value);
    }
    if let Some(secret) = secret {
        let (name, value) = match &secret.config.injection {
            EgressSecretInjection::AuthorizationBearer => (
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", secret.value.as_str()),
            ),
            EgressSecretInjection::Header { name, prefix } => (
                HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| EgressError::InvalidConfig("invalid secret header".into()))?,
                format!("{prefix}{}", secret.value.as_str()),
            ),
        };
        let value = HeaderValue::from_str(&value).map_err(|_| EgressError::SecretUnavailable)?;
        headers.insert(name, value);
    }
    Ok(headers)
}

fn canonical_request_headers(
    headers: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let mut canonical = BTreeMap::new();
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        if canonical.insert(name, value.clone()).is_some() {
            return Err(EgressError::InvalidRequest(
                "duplicate request header name".into(),
            ));
        }
    }
    Ok(canonical)
}

fn request_size_without_secret_value(request: &HttpRequest, current_url: &str) -> Result<usize> {
    request
        .method
        .len()
        .checked_add(current_url.len())
        .and_then(|size| {
            request
                .headers
                .iter()
                .try_fold(size, |size, (name, value)| {
                    size.checked_add(name.len())?
                        .checked_add(value.len())?
                        .checked_add(4)
                })
        })
        .and_then(|size| size.checked_add(request.body.as_ref().map_or(0, |body| body.len())))
        .ok_or_else(|| EgressError::Budget("request size overflow".into()))
}

fn request_size_for_url(
    request: &HttpRequest,
    current_url: &str,
    secret: Option<&ResolvedSecret>,
) -> Result<usize> {
    let mut bytes = request_size_without_secret_value(request, current_url)?;
    if let Some(secret) = secret {
        bytes = bytes
            .checked_add(secret.value.len())
            .and_then(|size| match &secret.config.injection {
                EgressSecretInjection::AuthorizationBearer => size.checked_add(22),
                EgressSecretInjection::Header { name, prefix } => size
                    .checked_add(name.len())?
                    .checked_add(prefix.len())?
                    .checked_add(4),
            })
            .ok_or_else(|| EgressError::Budget("request size overflow".into()))?;
    }
    Ok(bytes)
}

fn sha256_domain(domain: &str, value: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0]);
    digest.update(value);
    hex::encode(digest.finalize())
}

fn sha256_json(value: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| EgressError::InvalidRequest("request digest failed".into()))?;
    Ok(sha256_domain("tm.egress.canonical-json.v1", &bytes))
}

fn budget_request(
    ctx: &InvocationCtx,
    session_limits: &tm_host::EgressSessionLimits,
    destination: &EgressDestinationConfig,
    request_bytes: usize,
    timeout_ms: u64,
) -> Result<EgressBudgetRequest> {
    let request_bytes = u64::try_from(request_bytes)
        .map_err(|_| EgressError::Budget("request size overflow".into()))?;
    Ok(EgressBudgetRequest {
        reservation_id: random_token(16)?,
        session_id: ctx.session_id.clone(),
        destination_id: destination.id.clone(),
        request_bytes,
        response_reserved: u64::try_from(destination.max_response_bytes)
            .map_err(|_| EgressError::Budget("response size overflow".into()))?,
        time_reserved_ms: timeout_ms,
        session_limits: EgressUsageLimits {
            max_requests: u64::from(session_limits.max_requests),
            max_request_bytes: u64::try_from(session_limits.max_request_bytes)
                .map_err(|_| EgressError::Budget("session limit overflow".into()))?,
            max_response_bytes: u64::try_from(session_limits.max_response_bytes)
                .map_err(|_| EgressError::Budget("session limit overflow".into()))?,
            max_time_ms: session_limits.max_time_ms,
        },
        destination_limits: EgressUsageLimits {
            max_requests: u64::from(destination.max_requests_per_session),
            max_request_bytes: u64::try_from(destination.max_request_bytes_per_session)
                .map_err(|_| EgressError::Budget("destination limit overflow".into()))?,
            max_response_bytes: u64::try_from(destination.max_response_bytes_per_session)
                .map_err(|_| EgressError::Budget("destination limit overflow".into()))?,
            max_time_ms: destination.max_time_ms_per_session,
        },
    })
}

fn authorize_destination(
    ctx: &InvocationCtx,
    state: &RuntimeState,
    destination: &EgressDestinationConfig,
) -> Result<()> {
    require_exact_grant(
        &ctx.grants,
        &format!("egress.destination:{}", destination.id),
    )?;
    if state.revoked_destinations.contains(&TargetVersion {
        id: destination.id.clone(),
        version: destination.version,
    }) {
        return Err(EgressError::Revoked("destination".into()));
    }
    Ok(())
}

fn ensure_secret_not_revoked(state: &RuntimeState, secret: &EgressSecretConfig) -> Result<()> {
    if state.revoked_secrets.contains(&TargetVersion {
        id: secret.id.clone(),
        version: secret.version,
    }) {
        return Err(EgressError::Revoked("secret".into()));
    }
    Ok(())
}

fn require_session(ctx: &InvocationCtx) -> Result<()> {
    if ctx.session_id.is_empty() || ctx.session_id == "default" {
        Err(EgressError::Denied(
            "egress requires an explicit session identity".into(),
        ))
    } else {
        Ok(())
    }
}

fn require_exact_grant(grants: &CapabilityGrants, required: &str) -> Result<()> {
    if grants.names().any(|granted| granted == required) {
        Ok(())
    } else {
        Err(EgressError::Denied(format!(
            "missing exact grant {required}"
        )))
    }
}

async fn emit_denial(
    ctx: &InvocationCtx,
    audit_id: &str,
    method: &str,
    destination_id: Option<&str>,
    policy_generation: u64,
    error: &EgressError,
) -> Result<()> {
    ctx.emit_event(
        "egress_denied",
        json!({
            "auditId": audit_id,
            "destinationId": destination_id,
            "policyGeneration": policy_generation,
            "method": method,
            "errorCode": error.code(),
        }),
    )
    .await
    .map_err(|_| EgressError::Audit)
}

fn random_token(bytes: usize) -> Result<String> {
    let mut token = vec![0u8; bytes];
    getrandom::fill(&mut token).map_err(|_| EgressError::Transport)?;
    Ok(hex::encode(token))
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn redact_exact(text: &mut String, secret: &str) -> usize {
    if secret.is_empty() || !text.contains(secret) {
        return 0;
    }
    let count = text.matches(secret).count();
    *text = text.replace(secret, "[REDACTED_SECRET]");
    count
}

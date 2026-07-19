use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tm_host::{
    GrantDoc, HostError, HostFn, HostRegistry, InvocationCtx, SecretHandle, ToolDocs, ToolErrorDoc,
    ToolExample,
};
use url::Url;

use crate::{
    EgressMutationRecord, EgressRuntime, HttpRequest, MutationExecution, PreparedMutation,
};

#[derive(Debug, Clone)]
pub struct SecretsUseFn {
    runtime: EgressRuntime,
    docs: ToolDocs,
}

impl SecretsUseFn {
    pub fn new(runtime: EgressRuntime) -> Self {
        Self {
            runtime,
            docs: tool_docs(
                "secrets.use",
                "secrets",
                "Issue a session-bound opaque handle for one configured secret",
                "secrets.use({ name }) -> { token }",
                "none; issuance requires the exact secrets.use:<id> grant",
            ),
        }
    }
}

#[async_trait]
impl HostFn for SecretsUseFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            name: String,
        }

        let args: Args = serde_json::from_value(args)
            .map_err(|error| HostError::InvalidArgs(error.to_string()))?;
        let handle = self
            .runtime
            .issue_secret_handle(ctx, &args.name)
            .await
            .map_err(|error| error.into_host())?;
        serde_json::to_value(handle).map_err(|error| HostError::HostCall(error.to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct HttpGetFn {
    runtime: EgressRuntime,
    docs: ToolDocs,
}

impl HttpGetFn {
    pub fn new(runtime: EgressRuntime) -> Self {
        Self {
            runtime,
            docs: tool_docs(
                "http.get",
                "http",
                "Fetch bounded UTF-8 text from an exact configured destination",
                "http.get({ url, headers?, auth?, timeoutMs? }) -> HttpResponse",
                "none; policy, exact destination grant, DNS, and budgets are still enforced",
            ),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GetArgs {
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    auth: Option<SecretHandle>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[async_trait]
impl HostFn for HttpGetFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: GetArgs = serde_json::from_value(args)
            .map_err(|error| HostError::InvalidArgs(error.to_string()))?;
        execute_and_encode(
            &self.runtime,
            ctx,
            HttpRequest {
                method: "GET".into(),
                url: args.url,
                headers: args.headers,
                body: None,
                auth: args.auth,
                timeout_ms: args.timeout_ms,
            },
        )
        .await
    }
}

#[derive(Debug, Clone)]
pub struct HttpRequestFn {
    runtime: EgressRuntime,
    docs: ToolDocs,
}

impl HttpRequestFn {
    pub fn new(runtime: EgressRuntime) -> Self {
        Self {
            runtime,
            docs: tool_docs(
                "http.request",
                "http",
                "Send a bounded HTTP request through the destination-scoped egress boundary",
                "http.request({ method, url, headers?, body?, auth?, timeoutMs? }) -> HttpResponse",
                "required for every method except GET; approval shows bounded redacted semantics plus exact digests and non-secret target metadata",
            ),
        }
    }
}

#[async_trait]
impl HostFn for HttpRequestFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let request: HttpRequest = serde_json::from_value(args)
            .map_err(|error| HostError::InvalidArgs(error.to_string()))?;
        if request.method.eq_ignore_ascii_case("GET") {
            return execute_and_encode(&self.runtime, ctx, request).await;
        }
        let before = self
            .runtime
            .prepare_mutation(ctx, &request)
            .await
            .map_err(|error| error.into_host())?;
        ctx.require_approval(&approval_action(&request, &before)?)
            .await?;
        let after = self
            .runtime
            .prepare_mutation(ctx, &request)
            .await
            .map_err(|error| error.into_host())?;
        if before != after {
            return Err(HostError::CapabilityDenied(
                "egress policy or secret handle changed during approval".into(),
            ));
        }
        match self
            .runtime
            .begin_mutation(ctx, &after)
            .await
            .map_err(|error| error.into_host())?
        {
            MutationExecution::Execute(effect) => {
                let response = self
                    .runtime
                    .execute_prepared_mutation(ctx, request, after, &effect)
                    .await
                    .map_err(|error| error.into_host())?;
                serde_json::to_value(response)
                    .map_err(|error| HostError::HostCall(error.to_string()))
            }
            MutationExecution::Replay(effect) => mutation_replay_receipt(&effect),
        }
    }
}

pub fn register_egress_functions(registry: &mut HostRegistry, runtime: EgressRuntime) {
    registry.register(Arc::new(SecretsUseFn::new(runtime.clone())));
    registry.register(Arc::new(HttpGetFn::new(runtime.clone())));
    registry.register(Arc::new(HttpRequestFn::new(runtime)));
}

async fn execute_and_encode(
    runtime: &EgressRuntime,
    ctx: &InvocationCtx,
    request: HttpRequest,
) -> tm_host::Result<Value> {
    let response = runtime
        .execute(ctx, request)
        .await
        .map_err(|error| error.into_host())?;
    serde_json::to_value(response).map_err(|error| HostError::HostCall(error.to_string()))
}

fn approval_action(request: &HttpRequest, prepared: &PreparedMutation) -> tm_host::Result<String> {
    let url = Url::parse(&request.url)
        .map_err(|_| HostError::InvalidArgs("http.request URL must be absolute".into()))?;
    let host = url
        .host_str()
        .ok_or_else(|| HostError::InvalidArgs("http.request URL must have a host".into()))?;
    let header_names = request
        .headers
        .keys()
        .take(MAX_APPROVAL_HEADERS)
        .map(|name| name.to_ascii_lowercase())
        .map(|name| bounded_text(&name, MAX_APPROVAL_HEADER_NAME_CHARS))
        .collect::<Vec<_>>();
    let path = bounded_text(&redacted_path_preview(url.path()), MAX_APPROVAL_PATH_CHARS);
    let query_preview = redacted_query_preview(&url);
    let body_bytes = request.body.as_ref().map_or(0, |body| body.len());
    let body_sha256 = request
        .body
        .as_ref()
        .map(|body| hex::encode(Sha256::digest(body.as_bytes())));
    let body_preview = request.body.as_deref().and_then(json_body_preview);
    let method = bounded_text(&request.method.to_ascii_uppercase(), 32);
    serde_json::to_string(&json!({
        "operation": "http.request",
        "method": method,
        "scheme": url.scheme(),
        "host": host,
        "port": url.port_or_known_default(),
        "path": path,
        "pathBytes": url.path().len(),
        "queryBytes": url.query().map_or(0, str::len),
        "queryPreview": query_preview,
        "queryDigest": prepared.query_digest,
        "bodyBytes": body_bytes,
        "bodySha256": body_sha256,
        "bodyPreview": body_preview,
        "headerNames": header_names,
        "headerNamesTruncated": request.headers.len() > MAX_APPROVAL_HEADERS,
        "usesSecretHandle": request.auth.is_some(),
        "destinationId": prepared.destination_id,
        "destinationVersion": prepared.destination_version,
        "secretId": prepared.secret.as_ref().map(|secret| secret.secret_id.as_str()),
        "secretVersion": prepared.secret.as_ref().map(|secret| secret.secret_version),
        "targetDigest": prepared.target_digest,
        "requestDigest": prepared.request_digest,
    }))
    .map_err(|error| HostError::HostCall(error.to_string()))
}

const MAX_APPROVAL_HEADERS: usize = 32;
const MAX_APPROVAL_HEADER_NAME_CHARS: usize = 64;
const MAX_APPROVAL_PATH_CHARS: usize = 512;
const MAX_APPROVAL_PATH_SEGMENT_CHARS: usize = 96;
const MAX_APPROVAL_QUERY_PAIRS: usize = 32;
const MAX_APPROVAL_QUERY_KEY_CHARS: usize = 64;
const MAX_APPROVAL_QUERY_VALUE_CHARS: usize = 128;
const MAX_APPROVAL_JSON_BYTES: usize = 64 * 1024;
const MAX_APPROVAL_JSON_DEPTH: usize = 4;
const MAX_APPROVAL_JSON_NODES: usize = 64;
const MAX_APPROVAL_JSON_TEXT_BYTES: usize = 1_024;
const MAX_APPROVAL_JSON_KEY_CHARS: usize = 64;
const MAX_APPROVAL_JSON_STRING_CHARS: usize = 128;

fn redacted_path_preview(path: &str) -> String {
    let mut previous_sensitive = false;
    path.split('/')
        .map(|segment| {
            let decoded_hint = segment.to_ascii_lowercase();
            let sensitive_name = sensitive_key(&decoded_hint);
            let redacted = previous_sensitive || looks_like_secret(segment);
            previous_sensitive = sensitive_name;
            if redacted {
                "[REDACTED]".to_string()
            } else {
                bounded_text(segment, MAX_APPROVAL_PATH_SEGMENT_CHARS)
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn redacted_query_preview(url: &Url) -> Value {
    let mut preview = Vec::new();
    let mut truncated = false;
    for (index, (key, value)) in url.query_pairs().enumerate() {
        if index >= MAX_APPROVAL_QUERY_PAIRS {
            truncated = true;
            break;
        }
        let key = bounded_text(&key, MAX_APPROVAL_QUERY_KEY_CHARS);
        let value = if sensitive_key(&key) || looks_like_secret(&value) {
            "[REDACTED]".to_string()
        } else {
            bounded_text(&value, MAX_APPROVAL_QUERY_VALUE_CHARS)
        };
        preview.push(json!({ "name": key, "value": value }));
    }
    json!({ "pairs": preview, "truncated": truncated })
}

fn mutation_replay_receipt(effect: &EgressMutationRecord) -> tm_host::Result<Value> {
    Ok(json!({
        "replayed": true,
        "effectId": effect.intent.effect_id,
        "status": effect.status,
        "destinationId": effect.intent.destination_id,
        "destinationVersion": effect.intent.destination_version,
        "targetDigest": effect.intent.target_digest,
        "requestDigest": effect.intent.request_digest,
        "resultDigest": effect.result_digest,
        "resultBytes": effect.result_bytes,
        "errorCode": effect.error_code,
        "errorDigest": effect.error_digest,
    }))
}

fn json_body_preview(body: &str) -> Option<Value> {
    if body.len() > MAX_APPROVAL_JSON_BYTES {
        return Some(json!({"truncated": true, "reason": "json_body_exceeds_preview_cap"}));
    }
    let value = serde_json::from_str::<Value>(body).ok()?;
    let mut budget = JsonPreviewBudget {
        nodes: MAX_APPROVAL_JSON_NODES,
        text_bytes: MAX_APPROVAL_JSON_TEXT_BYTES,
    };
    Some(redacted_json_preview(&value, 0, &mut budget))
}

struct JsonPreviewBudget {
    nodes: usize,
    text_bytes: usize,
}

fn redacted_json_preview(value: &Value, depth: usize, budget: &mut JsonPreviewBudget) -> Value {
    if budget.nodes == 0 || depth > MAX_APPROVAL_JSON_DEPTH {
        return Value::String("[TRUNCATED]".into());
    }
    budget.nodes -= 1;
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::String(value) => {
            if looks_like_secret(value) {
                Value::String("[REDACTED]".into())
            } else {
                Value::String(bounded_budget_text(
                    value,
                    MAX_APPROVAL_JSON_STRING_CHARS,
                    budget,
                ))
            }
        }
        Value::Array(values) => {
            let mut preview = Vec::new();
            for value in values {
                if budget.nodes == 0 {
                    preview.push(Value::String("[TRUNCATED]".into()));
                    break;
                }
                preview.push(redacted_json_preview(value, depth + 1, budget));
            }
            Value::Array(preview)
        }
        Value::Object(values) => {
            let mut preview = serde_json::Map::new();
            for (key, value) in values {
                if budget.nodes == 0 {
                    preview.insert("[TRUNCATED]".into(), Value::Bool(true));
                    break;
                }
                let key_preview = bounded_budget_text(key, MAX_APPROVAL_JSON_KEY_CHARS, budget);
                let value = if sensitive_key(key) {
                    budget.nodes -= 1;
                    Value::String("[REDACTED]".into())
                } else {
                    redacted_json_preview(value, depth + 1, budget)
                };
                preview.insert(key_preview, value);
            }
            Value::Object(preview)
        }
    }
}

fn sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "authorization",
        "password",
        "passwd",
        "credential",
        "cookie",
        "secret",
        "token",
    ]
    .iter()
    .any(|sensitive| normalized.contains(sensitive))
        || matches!(
            normalized.as_str(),
            "key" | "apikey" | "privatekey" | "accesskey"
        )
}

fn looks_like_secret(value: &str) -> bool {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("bearer ")
        || lower.starts_with("basic ")
        || lower.starts_with("sk-")
        || lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("xox")
        || lower.starts_with("akia")
        || lower.starts_with("-----begin ")
        || (lower.starts_with("eyj") && lower.matches('.').count() == 2)
        || lower.contains("token=")
        || lower.contains("secret=")
        || lower.contains("api_key=")
        || lower.contains("password=")
        || lower.contains("secret")
        || (trimmed.len() >= 32
            && !trimmed.chars().any(char::is_whitespace)
            && trimmed.chars().any(|character| character.is_ascii_digit())
            && trimmed
                .chars()
                .any(|character| character.is_ascii_alphabetic()))
}

fn bounded_budget_text(value: &str, max_chars: usize, budget: &mut JsonPreviewBudget) -> String {
    let allowed = max_chars.min(budget.text_bytes);
    let bounded = bounded_text(value, allowed);
    budget.text_bytes = budget.text_bytes.saturating_sub(bounded.len());
    bounded
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let bounded = chars
        .by_ref()
        .take(max_chars)
        .map(|character| {
            if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect::<String>();
    if chars.next().is_some() {
        format!("{bounded}…")
    } else {
        bounded
    }
}

fn tool_docs(
    name: &str,
    namespace: &str,
    summary: &str,
    signature: &str,
    approval: &str,
) -> ToolDocs {
    ToolDocs {
        name: name.into(),
        namespace: namespace.into(),
        summary: summary.into(),
        description: Some(
            "Fail-closed host egress. URLs, methods, redirects, DNS answers, byte/time budgets, current exact grants, and runtime revocations are checked by the host. Caller Authorization and Cookie headers are forbidden.".into(),
        ),
        signature: signature.into(),
        args_schema: json!({ "type": "object" }),
        result_schema: Some(json!({ "type": "object" })),
        examples: Vec::<ToolExample>::new(),
        errors: vec![
            ToolErrorDoc {
                name: "CapabilityDeniedError".into(),
                when: "the destination, secret, URL, DNS answer, or current grant is denied".into(),
                retryable: false,
            },
            ToolErrorDoc {
                name: "QuotaExceededError".into(),
                when: "a request or session byte, request-count, or time budget is exhausted".into(),
                retryable: false,
            },
            ToolErrorDoc {
                name: "TimeoutError".into(),
                when: "the per-request timeout expires".into(),
                retryable: true,
            },
        ],
        grants: vec![GrantDoc {
            kind: "exact".into(),
            description: "requires the tool grant plus exact egress.destination:<id> and, when used, secrets.use:<id> grants".into(),
        }],
        sensitive: true,
        approval: approval.into(),
        since: "0.1.0".into(),
        stability: "experimental".into(),
    }
}

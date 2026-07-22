//! Model-controlled long-term memory retrieval through the sandbox SDK.
//!
//! `memory.search` is a capability-gated [`tm_host::HostFn`], not a second native model tool.
//! A turn performs no long-term retrieval unless the model calls this function from `execute`.

use async_trait::async_trait;
use serde::Deserialize;
use tm_host::{GrantDoc, HostError, HostFn, InvocationCtx, ToolDocs, ToolErrorDoc, ToolExample};

use super::*;

pub(super) const MEMORY_SEARCH_CAPABILITY: &str = "memory.search";
const MAX_MEMORY_SEARCH_QUERY_BYTES: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MemorySearchArgs {
    query: String,
}

pub(super) struct MemorySearchHostFn<S, M> {
    store: Arc<S>,
    memory: Arc<M>,
    docs: ToolDocs,
}

impl<S, M> MemorySearchHostFn<S, M> {
    pub(super) fn new<C>(state: &AppState<S, M, C>) -> Self {
        Self {
            store: Arc::clone(&state.store),
            memory: Arc::clone(&state.memory),
            docs: memory_search_docs(),
        }
    }
}

#[async_trait]
impl<S, M> HostFn for MemorySearchHostFn<S, M>
where
    S: Store,
    M: MemoryProvider,
{
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: MemorySearchArgs = serde_json::from_value(args)
            .map_err(|error| HostError::InvalidArgs(error.to_string()))?;
        let query = args.query.trim();
        if query.is_empty() || query.len() > MAX_MEMORY_SEARCH_QUERY_BYTES {
            return Err(HostError::InvalidArgs(format!(
                "query must contain 1-{MAX_MEMORY_SEARCH_QUERY_BYTES} bytes"
            )));
        }
        let session_id = ctx.session_id.parse::<Uuid>().map_err(|_| {
            HostError::InvalidArgs("memory.search requires an authorized session context".into())
        })?;
        let turn_id = ctx
            .events
            .effect_scope_id()
            .ok_or_else(|| {
                HostError::InvalidArgs(
                    "memory.search requires a durable turn execution context".into(),
                )
            })?
            .parse::<Uuid>()
            .map_err(|_| {
                HostError::InvalidArgs("memory.search received an invalid durable turn id".into())
            })?;
        let session = self
            .store
            .get_session(session_id)
            .await
            .map_err(host_error)?;
        let expected = tm_host::MemoryAuthority {
            subject: session.owner_subject.clone(),
            scope: session.memory_scope(),
        };
        if ctx.memory_authority.as_ref() != Some(&expected) {
            return Err(HostError::CapabilityDenied(
                "memory.search authority does not match server authority".to_string(),
            ));
        }
        self.store
            .ensure_memory_scope_active(&session.owner_subject, &session.memory_scope())
            .await
            .map_err(host_error)?;

        if let Some(event) = self
            .store
            .event_for_turn(session_id, turn_id, "memory_recall")
            .await
            .map_err(host_error)?
        {
            validate_persisted_context(&event, &session.owner_subject, &session.memory_scope())?;
            return Ok(event.payload_json);
        }

        let memory = self
            .memory
            .context_for_turn(&session.owner_subject, &session.memory_scope(), query)
            .await
            .map_err(host_error)?;
        let payload = json!({
            "schemaVersion": 1,
            "resourceUri": format!("memory://recalls/{turn_id}"),
            "context": memory,
        });
        let (event, _) = self
            .store
            .append_event_for_turn_once(session_id, "memory_recall", payload, turn_id)
            .await
            .map_err(host_error)?;
        validate_persisted_context(&event, &session.owner_subject, &session.memory_scope())?;
        Ok(event.payload_json)
    }
}

fn validate_persisted_context(
    event: &SessionEvent,
    subject: &str,
    scope: &str,
) -> tm_host::Result<()> {
    let memory: crate::MemoryContext =
        serde_json::from_value(event.payload_json.get("context").cloned().ok_or_else(|| {
            HostError::HostCall("persisted memory search is missing context".into())
        })?)
        .map_err(|error| {
            HostError::HostCall(format!("persisted memory search is invalid: {error}"))
        })?;
    if memory.subject != subject || memory.scope != scope {
        return Err(HostError::CapabilityDenied(
            "persisted memory search authority does not match the current session".to_string(),
        ));
    }
    Ok(())
}

fn host_error(error: impl std::fmt::Display) -> HostError {
    HostError::HostCall(error.to_string())
}

fn memory_search_docs() -> ToolDocs {
    ToolDocs {
        name: MEMORY_SEARCH_CAPABILITY.to_string(),
        namespace: "memory".to_string(),
        summary: "Search long-term memory only when the current request needs it".to_string(),
        description: Some(
            "Run bounded, authority-scoped hybrid memory retrieval. Call only when prior preferences, commitments, decisions, project context, or cross-session continuity could materially improve the answer. If the current conversation is sufficient, answer without calling it. The first search in a durable turn is persisted and reused exactly on retries."
                .to_string(),
        ),
        signature: "@memory.search {query} -> MemorySearchResult".to_string(),
        args_schema: json!({
            "type": "object",
            "required": ["query"],
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": MAX_MEMORY_SEARCH_QUERY_BYTES
                }
            }
        }),
        result_schema: Some(json!({
            "type": "object",
            "required": ["schemaVersion", "resourceUri", "context"],
            "properties": {
                "schemaVersion": { "type": "integer" },
                "resourceUri": { "type": "string" },
                "context": { "type": "object" }
            }
        })),
        examples: vec![ToolExample {
            title: Some("Recall a relevant preference".to_string()),
            code: "let result = @memory.search {query: \"owner preferences relevant to choosing this implementation\"};\nresult.context |> display {kind: \"json\"}".to_string(),
            notes: Some(
                "Skip this call when the active transcript already contains enough context."
                    .to_string(),
            ),
        }],
        errors: vec![
            ToolErrorDoc {
                name: "CapabilityDeniedError".to_string(),
                when: "The active mode lacks memory.search or the session scope no longer matches server authority.".to_string(),
                retryable: false,
            },
            ToolErrorDoc {
                name: "InvalidArgsError".to_string(),
                when: "The query is empty, oversized, or the call is outside a durable turn.".to_string(),
                retryable: false,
            },
            ToolErrorDoc {
                name: "HostCallError".to_string(),
                when: "The authoritative session or memory store is unavailable.".to_string(),
                retryable: true,
            },
        ],
        grants: vec![GrantDoc {
            kind: "capability".to_string(),
            description: "Requires the exact memory.search turn grant; handler registration alone grants no authority.".to_string(),
        }],
        sensitive: false,
        approval: "never".to_string(),
        since: "model-controlled-recall".to_string(),
        stability: "experimental".to_string(),
    }
}

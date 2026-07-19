use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{McpError, Result};

/// Hash-only description of one approved mutation. `effect_scope_id` is supplied by the host's
/// durable turn boundary; the effect id additionally binds the exact catalog target and arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpMutationIntent {
    pub effect_id: String,
    pub session_id: String,
    pub effect_scope_id: String,
    pub actor_id: Option<String>,
    pub catalog_generation: u64,
    pub catalog_digest: String,
    pub server: String,
    pub tool: String,
    pub target_digest: String,
    pub request_digest: String,
    pub request_bytes: usize,
}

impl McpMutationIntent {
    /// Compare the stable remote-effect identity while deliberately ignoring catalog generation
    /// metadata. An equivalent catalog reload must not mint a second mutation for the same
    /// durable turn, exact target digest, and exact arguments. The first attempt's catalog
    /// generation and digest remain recorded on the returned effect for audit.
    pub fn same_effect_identity(&self, other: &Self) -> bool {
        self.effect_id == other.effect_id
            && self.session_id == other.session_id
            && self.effect_scope_id == other.effect_scope_id
            && self.actor_id == other.actor_id
            && self.server == other.server
            && self.tool == other.tool
            && self.target_digest == other.target_digest
            && self.request_digest == other.request_digest
            && self.request_bytes == other.request_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpMutationEffectStatus {
    Started,
    Succeeded,
    Failed,
    Uncertain,
}

impl McpMutationEffectStatus {
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Started)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpMutationEffectRecord {
    pub intent: McpMutationIntent,
    pub status: McpMutationEffectStatus,
    pub result_digest: Option<String>,
    pub result_bytes: Option<usize>,
    pub error_code: Option<String>,
    pub error_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpMutationEffectClaim {
    pub record: McpMutationEffectRecord,
    pub created: bool,
}

impl McpMutationEffectRecord {
    fn started(intent: McpMutationIntent) -> Self {
        Self {
            intent,
            status: McpMutationEffectStatus::Started,
            result_digest: None,
            result_bytes: None,
            error_code: None,
            error_digest: None,
        }
    }
}

/// Host-owned idempotency boundary for mutation tools.
///
/// `begin` must atomically insert-or-read by `effect_id`. A pre-existing `Started` row means the
/// previous process may have reached the remote peer, so the caller must treat it as uncertain and
/// never resend. Terminal writes are compare-and-set from `Started` and idempotent for an identical
/// terminal record.
#[async_trait]
pub trait McpMutationEffectStore: Send + Sync {
    async fn begin(&self, intent: McpMutationIntent) -> Result<McpMutationEffectClaim>;

    async fn finish(
        &self,
        effect_id: &str,
        status: McpMutationEffectStatus,
        result_digest: Option<&str>,
        result_bytes: Option<usize>,
        error_code: Option<&str>,
        error_digest: Option<&str>,
    ) -> Result<McpMutationEffectRecord>;
}

/// Process-local implementation used by the CLI and unit tests. Server startup replaces this with
/// its durable Store-backed adapter before any user turn can invoke an imported mutation.
#[derive(Debug, Default)]
pub struct VolatileMcpMutationEffectStore {
    effects: Mutex<BTreeMap<String, McpMutationEffectRecord>>,
}

#[async_trait]
impl McpMutationEffectStore for VolatileMcpMutationEffectStore {
    async fn begin(&self, intent: McpMutationIntent) -> Result<McpMutationEffectClaim> {
        let mut effects = self.effects.lock();
        if let Some(existing) = effects.get(&intent.effect_id) {
            if !existing.intent.same_effect_identity(&intent) {
                return Err(McpError::Unavailable(
                    "MCP mutation effect id collides with different intent".to_string(),
                ));
            }
            return Ok(McpMutationEffectClaim {
                record: existing.clone(),
                created: false,
            });
        }
        let record = McpMutationEffectRecord::started(intent);
        effects.insert(record.intent.effect_id.clone(), record.clone());
        Ok(McpMutationEffectClaim {
            record,
            created: true,
        })
    }

    async fn finish(
        &self,
        effect_id: &str,
        status: McpMutationEffectStatus,
        result_digest: Option<&str>,
        result_bytes: Option<usize>,
        error_code: Option<&str>,
        error_digest: Option<&str>,
    ) -> Result<McpMutationEffectRecord> {
        if !status.is_terminal() {
            return Err(McpError::Unavailable(
                "MCP mutation finish requires a terminal status".to_string(),
            ));
        }
        let mut effects = self.effects.lock();
        let record = effects.get_mut(effect_id).ok_or_else(|| {
            McpError::Unavailable("MCP mutation effect intent was not persisted".to_string())
        })?;
        let requested = McpMutationEffectRecord {
            intent: record.intent.clone(),
            status,
            result_digest: result_digest.map(str::to_string),
            result_bytes,
            error_code: error_code.map(str::to_string),
            error_digest: error_digest.map(str::to_string),
        };
        if record.status.is_terminal() {
            if *record == requested {
                return Ok(record.clone());
            }
            return Err(McpError::Unavailable(
                "MCP mutation effect already has a different terminal state".to_string(),
            ));
        }
        *record = requested;
        Ok(record.clone())
    }
}

pub(crate) fn volatile_effect_store() -> Arc<dyn McpMutationEffectStore> {
    Arc::new(VolatileMcpMutationEffectStore::default())
}

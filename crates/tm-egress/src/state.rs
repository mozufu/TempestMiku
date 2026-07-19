use std::{collections::BTreeMap, sync::Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{EgressError, Result, budget::BudgetBook};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressMutationIntent {
    pub effect_id: String,
    pub session_id: String,
    pub effect_scope_id: String,
    pub session_digest: String,
    pub actor_digest: String,
    pub destination_id: String,
    pub destination_version: u64,
    pub target_digest: String,
    pub request_digest: String,
    pub request_bytes: usize,
}

impl EgressMutationIntent {
    pub fn same_effect_identity(&self, other: &Self) -> bool {
        self == other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressMutationStatus {
    Started,
    Succeeded,
    Failed,
    Uncertain,
}

impl EgressMutationStatus {
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Started)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressMutationRecord {
    pub intent: EgressMutationIntent,
    pub status: EgressMutationStatus,
    pub result_digest: Option<String>,
    pub result_bytes: Option<usize>,
    pub error_code: Option<String>,
    pub error_digest: Option<String>,
}

impl EgressMutationRecord {
    fn started(intent: EgressMutationIntent) -> Self {
        Self {
            intent,
            status: EgressMutationStatus::Started,
            result_digest: None,
            result_bytes: None,
            error_code: None,
            error_digest: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressMutationClaim {
    pub record: EgressMutationRecord,
    pub created: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressUsageLimits {
    pub max_requests: u64,
    pub max_request_bytes: u64,
    pub max_response_bytes: u64,
    pub max_time_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressUsage {
    pub requests: u64,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub response_reserved: u64,
    pub time_ms: u64,
    pub time_reserved_ms: u64,
}

impl EgressUsage {
    pub fn validate_reservation(
        self,
        limits: EgressUsageLimits,
        request_bytes: u64,
        response_reserved: u64,
        time_reserved_ms: u64,
    ) -> Result<()> {
        if self.requests >= limits.max_requests {
            return Err(EgressError::Budget("session request cap".into()));
        }
        if self.request_bytes.saturating_add(request_bytes) > limits.max_request_bytes {
            return Err(EgressError::Budget("session request-byte cap".into()));
        }
        if self
            .response_bytes
            .saturating_add(self.response_reserved)
            .saturating_add(response_reserved)
            > limits.max_response_bytes
        {
            return Err(EgressError::Budget("session response-byte cap".into()));
        }
        if self
            .time_ms
            .saturating_add(self.time_reserved_ms)
            .saturating_add(time_reserved_ms)
            > limits.max_time_ms
        {
            return Err(EgressError::Budget("session time cap".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressBudgetRequest {
    pub reservation_id: String,
    pub session_id: String,
    pub destination_id: String,
    pub request_bytes: u64,
    pub response_reserved: u64,
    pub time_reserved_ms: u64,
    pub session_limits: EgressUsageLimits,
    pub destination_limits: EgressUsageLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressBudgetReservation {
    pub reservation_id: String,
    pub session_id: String,
    pub destination_id: String,
    pub response_reserved: u64,
    pub time_reserved_ms: u64,
}

#[async_trait]
pub trait EgressStateStore: Send + Sync {
    async fn begin_mutation(&self, intent: EgressMutationIntent) -> Result<EgressMutationClaim>;

    #[allow(clippy::too_many_arguments)]
    async fn finish_mutation(
        &self,
        effect_id: &str,
        status: EgressMutationStatus,
        result_digest: Option<&str>,
        result_bytes: Option<usize>,
        error_code: Option<&str>,
        error_digest: Option<&str>,
    ) -> Result<EgressMutationRecord>;

    async fn reserve_budget(&self, request: EgressBudgetRequest)
    -> Result<EgressBudgetReservation>;

    async fn settle_budget(
        &self,
        reservation: EgressBudgetReservation,
        response_bytes: u64,
        elapsed_ms: u64,
    ) -> Result<()>;

    async fn clear_session(&self, session_id: &str) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct VolatileEgressStateStore {
    effects: Mutex<BTreeMap<String, EgressMutationRecord>>,
    budgets: BudgetBook,
}

#[async_trait]
impl EgressStateStore for VolatileEgressStateStore {
    async fn begin_mutation(&self, intent: EgressMutationIntent) -> Result<EgressMutationClaim> {
        let mut effects = self.effects.lock().expect("egress effect lock poisoned");
        if let Some(existing) = effects.get(&intent.effect_id) {
            if !existing.intent.same_effect_identity(&intent) {
                return Err(EgressError::Durability(
                    "egress mutation effect id collision".into(),
                ));
            }
            return Ok(EgressMutationClaim {
                record: existing.clone(),
                created: false,
            });
        }
        let record = EgressMutationRecord::started(intent);
        effects.insert(record.intent.effect_id.clone(), record.clone());
        Ok(EgressMutationClaim {
            record,
            created: true,
        })
    }

    async fn finish_mutation(
        &self,
        effect_id: &str,
        status: EgressMutationStatus,
        result_digest: Option<&str>,
        result_bytes: Option<usize>,
        error_code: Option<&str>,
        error_digest: Option<&str>,
    ) -> Result<EgressMutationRecord> {
        if !status.is_terminal() {
            return Err(EgressError::Durability(
                "egress mutation terminal status required".into(),
            ));
        }
        let mut effects = self.effects.lock().expect("egress effect lock poisoned");
        let record = effects.get_mut(effect_id).ok_or_else(|| {
            EgressError::Durability("egress mutation effect was not persisted".into())
        })?;
        let requested = EgressMutationRecord {
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
            return Err(EgressError::Durability(
                "egress mutation already has a different terminal state".into(),
            ));
        }
        *record = requested;
        Ok(record.clone())
    }

    async fn reserve_budget(
        &self,
        request: EgressBudgetRequest,
    ) -> Result<EgressBudgetReservation> {
        self.budgets.reserve(request)
    }

    async fn settle_budget(
        &self,
        reservation: EgressBudgetReservation,
        response_bytes: u64,
        elapsed_ms: u64,
    ) -> Result<()> {
        self.budgets.settle(reservation, response_bytes, elapsed_ms)
    }

    async fn clear_session(&self, session_id: &str) -> Result<()> {
        self.budgets.clear_session(session_id);
        Ok(())
    }
}

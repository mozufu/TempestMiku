use std::sync::Arc;

use async_trait::async_trait;

use super::Store;

/// Adapts the server Store to tm-egress without coupling the egress crate to server persistence.
/// Server startup installs this before a turn can invoke HTTP capabilities.
pub struct StoreEgressStateStore<S> {
    store: Arc<S>,
}

impl<S> StoreEgressStateStore<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<S> tm_egress::EgressStateStore for StoreEgressStateStore<S>
where
    S: Store,
{
    async fn begin_mutation(
        &self,
        intent: tm_egress::EgressMutationIntent,
    ) -> tm_egress::Result<tm_egress::EgressMutationClaim> {
        self.store
            .begin_egress_mutation_effect(intent)
            .await
            .map_err(store_error)
    }

    async fn finish_mutation(
        &self,
        effect_id: &str,
        status: tm_egress::EgressMutationStatus,
        result_digest: Option<&str>,
        result_bytes: Option<usize>,
        error_code: Option<&str>,
        error_digest: Option<&str>,
    ) -> tm_egress::Result<tm_egress::EgressMutationRecord> {
        self.store
            .finish_egress_mutation_effect(
                effect_id,
                status,
                result_digest,
                result_bytes,
                error_code,
                error_digest,
            )
            .await
            .map_err(store_error)
    }

    async fn reserve_budget(
        &self,
        request: tm_egress::EgressBudgetRequest,
    ) -> tm_egress::Result<tm_egress::EgressBudgetReservation> {
        self.store
            .reserve_egress_budget(request)
            .await
            .map_err(store_error)
    }

    async fn settle_budget(
        &self,
        reservation: tm_egress::EgressBudgetReservation,
        response_bytes: u64,
        elapsed_ms: u64,
    ) -> tm_egress::Result<()> {
        self.store
            .settle_egress_budget(reservation, response_bytes, elapsed_ms)
            .await
            .map_err(store_error)
    }

    async fn clear_session(&self, session_id: &str) -> tm_egress::Result<()> {
        self.store
            .clear_egress_session(session_id)
            .await
            .map_err(store_error)
    }
}

fn store_error(error: crate::ServerError) -> tm_egress::EgressError {
    match error {
        crate::ServerError::Policy(_) => {
            tm_egress::EgressError::Budget("durable session or destination cap".into())
        }
        crate::ServerError::Conflict(_) => {
            tm_egress::EgressError::Durability("persistent state collision".into())
        }
        crate::ServerError::NotFound(_)
        | crate::ServerError::Unauthorized
        | crate::ServerError::Forbidden
        | crate::ServerError::InvalidRequest(_)
        | crate::ServerError::Store(_)
        | crate::ServerError::Backend(_) => {
            tm_egress::EgressError::Durability("persistent state unavailable".into())
        }
    }
}

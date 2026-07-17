use std::{fmt, sync::Arc, time::Duration};

use chrono::Utc;
use uuid::Uuid;

use crate::{Result, ServerError};

use super::{
    PushCipher, PushProvider, PushProviderOutcome, PushRegistrationMetadata, PushRuntimeMetrics,
    PushStore,
};

const MAX_DELIVERY_ATTEMPTS: i32 = 5;

#[derive(Clone)]
pub struct PushService {
    store: Arc<dyn PushStore>,
    provider: Arc<dyn PushProvider>,
    cipher: PushCipher,
}

impl fmt::Debug for PushService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PushService")
            .field("provider", &self.provider.name())
            .finish_non_exhaustive()
    }
}

impl PushService {
    pub fn new(
        store: Arc<dyn PushStore>,
        provider: Arc<dyn PushProvider>,
        cipher: PushCipher,
    ) -> Self {
        Self {
            store,
            provider,
            cipher,
        }
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub async fn register(
        &self,
        device_id: Uuid,
        provider: &str,
        secret: &str,
    ) -> Result<PushRegistrationMetadata> {
        if provider != self.provider.name() {
            return Err(ServerError::InvalidRequest(format!(
                "push provider {provider} is not configured"
            )));
        }
        let encrypted = self.cipher.encrypt(device_id, provider, secret)?;
        self.store
            .upsert_registration(device_id, provider, encrypted, Utc::now())
            .await
    }

    pub async fn unregister(&self, device_id: Uuid) -> Result<()> {
        self.store
            .disable_registration(device_id, None, Utc::now())
            .await?;
        Ok(())
    }

    pub async fn runtime_metrics(&self) -> Result<PushRuntimeMetrics> {
        self.store.runtime_metrics(Utc::now()).await
    }

    pub async fn tick(&self, owner_id: Uuid) -> Result<usize> {
        let now = Utc::now();
        self.store.materialize_deliveries(now, 64).await?;
        let mut handled = 0;
        for _ in 0..32 {
            let Some(lease) = self
                .store
                .claim_next_delivery(owner_id, Utc::now(), chrono::Duration::seconds(60))
                .await?
            else {
                break;
            };
            let registration =
                match self
                    .cipher
                    .decrypt(lease.device_id, &lease.provider, &lease.encrypted_secret)
                {
                    Ok(registration) => registration,
                    Err(error) => {
                        self.store
                            .fail_delivery_and_disable_registration(
                                &lease,
                                &error.to_string(),
                                Utc::now(),
                            )
                            .await?;
                        handled += 1;
                        continue;
                    }
                };
            let result = self.provider.deliver(&registration, &lease.message).await;
            let error = result.error.as_deref().unwrap_or("push provider failure");
            match result.outcome {
                PushProviderOutcome::Delivered => {
                    self.store.complete_delivery(&lease, Utc::now()).await?;
                }
                PushProviderOutcome::PermanentRegistrationFailure => {
                    self.store
                        .fail_delivery_and_disable_registration(&lease, error, Utc::now())
                        .await?;
                }
                PushProviderOutcome::TransientFailure => {
                    if lease.attempts >= MAX_DELIVERY_ATTEMPTS
                        || Utc::now() >= lease.message.expires_at
                    {
                        self.store.fail_delivery(&lease, error, Utc::now()).await?;
                    } else {
                        let delay = retry_delay(lease.attempts);
                        let available_at = Utc::now()
                            + chrono::Duration::from_std(delay)
                                .expect("push retry delay fits chrono duration");
                        self.store
                            .retry_delivery(&lease, error, available_at, Utc::now())
                            .await?;
                    }
                }
            }
            handled += 1;
        }
        Ok(handled)
    }
}

fn retry_delay(attempts: i32) -> Duration {
    match attempts {
        ..=1 => Duration::from_secs(1),
        2 => Duration::from_secs(5),
        3 => Duration::from_secs(30),
        4 => Duration::from_secs(120),
        _ => Duration::from_secs(300),
    }
}

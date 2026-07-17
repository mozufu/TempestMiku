use std::fmt;

use async_trait::async_trait;
use parking_lot::Mutex;

use super::{PushMessage, PushProvider, PushProviderResult};

#[derive(Default)]
pub struct FakePushProvider {
    deliveries: Mutex<Vec<(String, PushMessage)>>,
    outcomes: Mutex<Vec<PushProviderResult>>,
}

impl fmt::Debug for FakePushProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FakePushProvider")
            .field("delivery_count", &self.deliveries.lock().len())
            .field("queued_outcome_count", &self.outcomes.lock().len())
            .finish()
    }
}

impl FakePushProvider {
    pub fn deliveries(&self) -> Vec<(String, PushMessage)> {
        self.deliveries.lock().clone()
    }

    pub fn queue_outcome(&self, outcome: PushProviderResult) {
        self.outcomes.lock().push(outcome);
    }
}

#[async_trait]
impl PushProvider for FakePushProvider {
    fn name(&self) -> &str {
        "fake"
    }

    async fn deliver(&self, registration: &str, message: &PushMessage) -> PushProviderResult {
        self.deliveries
            .lock()
            .push((registration.to_string(), message.clone()));
        let mut outcomes = self.outcomes.lock();
        if outcomes.is_empty() {
            PushProviderResult::delivered()
        } else {
            outcomes.remove(0)
        }
    }
}

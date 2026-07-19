mod broker;
mod durable;
mod outcome;
mod types;

use std::{collections::BTreeMap, sync::Arc};

use parking_lot::{Mutex, RwLock};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::Store;

pub use types::{
    ApprovalOption, ApprovalOutcome, ApprovalPrompt, ApprovalResolveDecision, ApprovalStatus,
    DetailedApprovalOutcome, DurableApprovalSpec, ResolveApprovalRequest,
};

pub struct ApprovalBroker {
    pending: Mutex<BTreeMap<Uuid, PendingApproval>>,
    store: RwLock<Option<Arc<dyn Store>>>,
    instance_id: Uuid,
}

struct PendingApproval {
    session_id: Uuid,
    sender: oneshot::Sender<ResolveApprovalRequest>,
}

impl Default for ApprovalBroker {
    fn default() -> Self {
        Self {
            pending: Mutex::new(BTreeMap::new()),
            store: RwLock::new(None),
            instance_id: Uuid::new_v4(),
        }
    }
}

impl ApprovalBroker {
    pub(crate) fn requester_id(&self) -> Uuid {
        self.instance_id
    }
}

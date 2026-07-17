use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use futures::future::BoxFuture;
use tm_core::CancellationToken;
use tokio::sync::watch;
use uuid::Uuid;

/// Request-local cancellation observed by both the agent loop and tm-lang's commit shield.
pub(crate) struct TurnCancellation {
    cancelled: watch::Sender<bool>,
}

impl TurnCancellation {
    fn new() -> Self {
        let (cancelled, _) = watch::channel(false);
        Self { cancelled }
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.send_replace(true);
    }
}

impl CancellationToken for TurnCancellation {
    fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }

    fn cancelled(&self) -> BoxFuture<'_, ()> {
        let mut cancelled = self.cancelled.subscribe();
        Box::pin(async move {
            if *cancelled.borrow() {
                return;
            }
            let _ = cancelled.wait_for(|value| *value).await;
        })
    }
}

/// Stable token installed in a cached tm-lang session and rebound to each request-local token.
pub(crate) struct CancellationProxy {
    target: watch::Sender<Option<Arc<TurnCancellation>>>,
}

impl CancellationProxy {
    pub(crate) fn new() -> Self {
        let (target, _) = watch::channel(None);
        Self { target }
    }

    pub(crate) fn bind(&self, target: Arc<TurnCancellation>) {
        self.target.send_replace(Some(target));
    }

    pub(crate) fn clear(&self) {
        self.target.send_replace(None);
    }
}

impl CancellationToken for CancellationProxy {
    fn is_cancelled(&self) -> bool {
        self.target
            .borrow()
            .as_ref()
            .is_some_and(|target| target.is_cancelled())
    }

    fn cancelled(&self) -> BoxFuture<'_, ()> {
        let mut target = self.target.subscribe();
        Box::pin(async move {
            loop {
                let current = target.borrow().clone();
                if let Some(current) = current {
                    current.cancelled().await;
                    return;
                }
                if target.changed().await.is_err() {
                    return;
                }
            }
        })
    }
}

/// Cancellation that trips when either an application-level or request-level source trips.
pub(crate) struct CombinedCancellation {
    first: Arc<dyn CancellationToken>,
    second: Arc<dyn CancellationToken>,
}

impl CombinedCancellation {
    pub(crate) fn new(
        first: Arc<dyn CancellationToken>,
        second: Arc<dyn CancellationToken>,
    ) -> Self {
        Self { first, second }
    }
}

impl CancellationToken for CombinedCancellation {
    fn is_cancelled(&self) -> bool {
        self.first.is_cancelled() || self.second.is_cancelled()
    }

    fn cancelled(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            tokio::select! {
                () = self.first.cancelled() => {}
                () = self.second.cancelled() => {}
            }
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct ActiveTurnRegistry {
    inner: Arc<Mutex<HashMap<ActiveTurnKey, Vec<Arc<TurnCancellation>>>>>,
}

impl ActiveTurnRegistry {
    pub(crate) fn register(&self, session_id: Uuid, turn_id: Option<Uuid>) -> ActiveTurnGuard {
        let key = ActiveTurnKey {
            session_id,
            // Non-durable callers still need a unique cancellation registration, but their
            // request-local key is never exposed to durable finalize/abort operations.
            turn_id: turn_id.unwrap_or_else(Uuid::new_v4),
        };
        let cancellation = Arc::new(TurnCancellation::new());
        self.inner
            .lock()
            .expect("active turn registry lock poisoned")
            .entry(key)
            .or_default()
            .push(Arc::clone(&cancellation));
        ActiveTurnGuard {
            registry: self.clone(),
            key,
            cancellation,
            finished: false,
        }
    }

    pub(crate) fn cancel(&self, session_id: Uuid, turn_id: Uuid) {
        let cancellations = self
            .inner
            .lock()
            .expect("active turn registry lock poisoned")
            .get(&ActiveTurnKey {
                session_id,
                turn_id,
            })
            .cloned()
            .unwrap_or_default();
        for cancellation in cancellations {
            cancellation.cancel();
        }
    }

    fn remove(&self, key: ActiveTurnKey, cancellation: &Arc<TurnCancellation>) {
        let mut active = self
            .inner
            .lock()
            .expect("active turn registry lock poisoned");
        let remove_key = if let Some(cancellations) = active.get_mut(&key) {
            cancellations.retain(|candidate| !Arc::ptr_eq(candidate, cancellation));
            cancellations.is_empty()
        } else {
            false
        };
        if remove_key {
            active.remove(&key);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ActiveTurnKey {
    session_id: Uuid,
    turn_id: Uuid,
}

pub(crate) struct ActiveTurnGuard {
    registry: ActiveTurnRegistry,
    key: ActiveTurnKey,
    cancellation: Arc<TurnCancellation>,
    finished: bool,
}

impl ActiveTurnGuard {
    pub(crate) fn cancellation(&self) -> Arc<TurnCancellation> {
        Arc::clone(&self.cancellation)
    }

    pub(crate) fn finish(mut self) {
        self.finished = true;
        self.registry.remove(self.key, &self.cancellation);
    }
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.cancellation.cancel();
            self.registry.remove(self.key, &self.cancellation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dropped_guard_cancels_request_and_removes_registration() {
        let registry = ActiveTurnRegistry::default();
        let session_id = Uuid::new_v4();
        let turn_id = Uuid::new_v4();
        let guard = registry.register(session_id, Some(turn_id));
        let cancellation = guard.cancellation();

        drop(guard);

        cancellation.cancelled().await;
        assert!(cancellation.is_cancelled());
        assert!(
            registry
                .inner
                .lock()
                .expect("active turn registry lock poisoned")
                .get(&ActiveTurnKey {
                    session_id,
                    turn_id,
                })
                .is_none()
        );
    }

    #[tokio::test]
    async fn cancellation_is_scoped_to_the_exact_durable_turn() {
        let registry = ActiveTurnRegistry::default();
        let session_id = Uuid::new_v4();
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let first = registry.register(session_id, Some(first_id));
        let second = registry.register(session_id, Some(second_id));

        registry.cancel(session_id, first_id);

        assert!(first.cancellation().is_cancelled());
        assert!(!second.cancellation().is_cancelled());
    }

    #[tokio::test]
    async fn combined_cancellation_observes_either_source() {
        let first = Arc::new(TurnCancellation::new());
        let second = Arc::new(TurnCancellation::new());
        let combined = CombinedCancellation::new(first.clone(), second.clone());

        second.cancel();

        combined.cancelled().await;
        assert!(combined.is_cancelled());
        assert!(!first.is_cancelled());
    }
}

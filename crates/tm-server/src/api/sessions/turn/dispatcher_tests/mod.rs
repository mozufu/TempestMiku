use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use tm_core::EventSink;
use tm_host::{FsMode, LinkedFolderConfig, LinkedFolders};

use super::*;
use super::{
    body::execute_turn_body,
    dispatcher::{TurnMaintenance, execute_claimed_turn, run_turn_dispatcher},
};
use crate::{
    AuthConfig, ChatTurn, EchoChatRunner, InMemoryStore, MemoryContext, NewSession,
    RecallChunkRecord, StoreMemoryProvider,
};

#[derive(Default)]
struct ConcurrentRunner {
    active: AtomicUsize,
    max_active: AtomicUsize,
}

struct SlowRunner;

struct ReplayMustNotQueryMemory;

#[derive(Clone, Copy)]
enum OwnershipLossPoint {
    BeforeComplete,
    DuringHeartbeat,
}

struct LinearizedRunner {
    store: Arc<InMemoryStore>,
    loss: OwnershipLossPoint,
    losses_remaining: AtomicUsize,
    committed: AtomicUsize,
    pending: Mutex<Option<(Uuid, Uuid, usize)>>,
    observed: Mutex<Vec<usize>>,
    cancelled: AtomicBool,
    cancellation: tokio::sync::Notify,
    promotions: AtomicUsize,
    aborts: AtomicUsize,
}

impl LinearizedRunner {
    fn new(store: Arc<InMemoryStore>, loss: OwnershipLossPoint) -> Self {
        Self {
            store,
            loss,
            losses_remaining: AtomicUsize::new(1),
            committed: AtomicUsize::new(0),
            pending: Mutex::new(None),
            observed: Mutex::new(Vec::new()),
            cancelled: AtomicBool::new(false),
            cancellation: tokio::sync::Notify::new(),
            promotions: AtomicUsize::new(0),
            aborts: AtomicUsize::new(0),
        }
    }

    fn take_loss(&self) -> bool {
        self.losses_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    }
}

#[async_trait]
impl MemoryProvider for ReplayMustNotQueryMemory {
    async fn context_for_turn(
        &self,
        _subject: &str,
        _scope: &str,
        _query: &str,
    ) -> Result<MemoryContext> {
        panic!("a durable memory_recall event must be replayed without re-querying memory")
    }
}

#[async_trait]
impl ChatRunner for ConcurrentRunner {
    async fn run_turn(
        &self,
        _turn: ChatTurn,
        _sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(75)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok("done".to_string())
    }
}

#[async_trait]
impl ChatRunner for SlowRunner {
    async fn run_turn(
        &self,
        _turn: ChatTurn,
        _sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        tokio::time::sleep(Duration::from_millis(250)).await;
        Ok("slow turn done".to_string())
    }
}

#[async_trait]
impl ChatRunner for LinearizedRunner {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        _sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        let turn_id = turn
            .durable_turn_id
            .expect("linearization fixture only runs durable turns");
        self.cancelled.store(false, Ordering::SeqCst);
        let next = self.committed.load(Ordering::SeqCst) + 1;
        self.observed.lock().unwrap().push(next);
        *self.pending.lock().unwrap() = Some((turn.session_id, turn_id, next));

        if self.take_loss() {
            self.store
                .fail_stale_running_turns(
                    Utc::now() + chrono::Duration::seconds(1),
                    Utc::now(),
                    "fixture ownership loss",
                )
                .await?;
            if matches!(self.loss, OwnershipLossPoint::DuringHeartbeat) {
                while !self.cancelled.load(Ordering::SeqCst) {
                    self.cancellation.notified().await;
                }
                return Err(ServerError::Backend("fixture cancelled".to_string()));
            }
        }

        Ok(format!("state {next}"))
    }

    async fn promote_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
        let pending = self.pending.lock().unwrap().take();
        if let Some((pending_session, pending_turn, value)) = pending {
            if pending_session != session_id || pending_turn != turn_id {
                return Err(ServerError::Conflict(
                    "fixture promotion identity mismatch".to_string(),
                ));
            }
            self.committed.store(value, Ordering::SeqCst);
            self.promotions.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    async fn abort_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
        let mut pending = self.pending.lock().unwrap();
        if pending
            .as_ref()
            .is_some_and(|(pending_session, pending_turn, _)| {
                *pending_session == session_id && *pending_turn == turn_id
            })
        {
            pending.take();
        }
        self.aborts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn cancel_turn(&self, _session_id: Uuid, _turn_id: Uuid) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.cancellation.notify_one();
    }
}

mod maintenance;
mod memory_replay;
mod ownership;

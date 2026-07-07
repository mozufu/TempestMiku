use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{Mutex, RwLock, mpsc};

use crate::actor::ActorId;

use super::{ActorMessage, MAX_INBOX_MESSAGES};

pub(super) struct ActorInbox {
    pub(super) sender: mpsc::Sender<ActorMessage>,
    receiver: Mutex<mpsc::Receiver<ActorMessage>>,
    backlog: Mutex<VecDeque<ActorMessage>>,
    unread: AtomicU64,
    last_activity: RwLock<Option<DateTime<Utc>>>,
}

impl ActorInbox {
    pub(super) fn new() -> Arc<Self> {
        let (sender, receiver) = mpsc::channel(MAX_INBOX_MESSAGES);
        Arc::new(Self {
            sender,
            receiver: Mutex::new(receiver),
            backlog: Mutex::new(VecDeque::new()),
            unread: AtomicU64::new(0),
            last_activity: RwLock::new(None),
        })
    }

    pub(super) async fn touch(&self, at: DateTime<Utc>) {
        *self.last_activity.write().await = Some(at);
    }

    pub(super) async fn last_activity(&self) -> Option<DateTime<Utc>> {
        *self.last_activity.read().await
    }

    pub(super) fn unread(&self) -> usize {
        self.unread.load(Ordering::Relaxed) as usize
    }

    pub(super) fn mark_delivered_to_inbox(&self) {
        self.unread.fetch_add(1, Ordering::Relaxed);
    }

    fn mark_taken_by_actor(&self) {
        let _ = self
            .unread
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(1))
            });
    }

    pub(super) async fn drain(&self, from: Option<&ActorId>) -> Vec<ActorMessage> {
        let mut drained = Vec::new();

        {
            let mut backlog = self.backlog.lock().await;
            let mut retained = VecDeque::new();
            while let Some(message) = backlog.pop_front() {
                if message_matches_from(&message, from) {
                    self.mark_taken_by_actor();
                    drained.push(message);
                } else {
                    retained.push_back(message);
                }
            }
            *backlog = retained;
        }

        let mut receiver = self.receiver.lock().await;
        loop {
            match receiver.try_recv() {
                Ok(message) if message_matches_from(&message, from) => {
                    self.mark_taken_by_actor();
                    drained.push(message);
                }
                Ok(message) => {
                    self.backlog.lock().await.push_back(message);
                }
                Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                    break;
                }
            }
        }

        drained
    }

    pub(super) async fn wait(
        &self,
        from: Option<&ActorId>,
        timeout: Duration,
    ) -> Option<ActorMessage> {
        if let Some(message) = self.take_matching_backlog(from).await {
            return Some(message);
        }

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return None;
            }
            let remaining = deadline.saturating_duration_since(now);
            let received = {
                let mut receiver = self.receiver.lock().await;
                tokio::time::timeout(remaining, receiver.recv()).await
            };
            match received {
                Ok(Some(message)) if message_matches_from(&message, from) => {
                    self.mark_taken_by_actor();
                    return Some(message);
                }
                Ok(Some(message)) => {
                    self.backlog.lock().await.push_back(message);
                }
                Ok(None) | Err(_) => return None,
            }
        }
    }

    async fn take_matching_backlog(&self, from: Option<&ActorId>) -> Option<ActorMessage> {
        let mut backlog = self.backlog.lock().await;
        let index = backlog
            .iter()
            .position(|message| message_matches_from(message, from))?;
        let message = backlog.remove(index)?;
        drop(backlog);
        self.mark_taken_by_actor();
        Some(message)
    }
}

fn message_matches_from(message: &ActorMessage, from: Option<&ActorId>) -> bool {
    match from {
        Some(from) => &message.from == from,
        None => true,
    }
}

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::actor::ActorId;

use super::inbox::ActorInbox;
use super::{ActorKey, ActorMessage, MailboxRegistry, Receipt, RegistryError};

const MAX_MESSAGES: usize = 1000;
pub(super) const MAX_MESSAGE_LOG_BYTES: usize = 512 * 1024;

struct LoggedMessage {
    session_id: String,
    message: ActorMessage,
    retained_bytes: usize,
}

#[derive(Default)]
pub(super) struct MessageLog {
    entries: VecDeque<LoggedMessage>,
    retained_bytes: usize,
}

impl MailboxRegistry {
    /// Store the full transcript for an actor (P3.3).
    ///
    /// Called by the orchestrator after `run_to_digest` returns `history_content`.
    /// Served by `HistoryResourceHandler`.
    pub async fn store_transcript_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
        content: String,
    ) {
        self.transcripts.write().await.insert(
            ActorKey::new(session_id, id),
            tm_memory::redact_dream_text(&content).text,
        );
    }

    /// Retrieve the stored transcript for an actor, if any.
    pub async fn get_transcript_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
    ) -> Option<String> {
        self.transcripts
            .read()
            .await
            .get(&ActorKey::new(session_id, id))
            .cloned()
    }

    /// Append a message to the bounded in-memory log.
    ///
    /// Oldest messages are dropped once the log exceeds [`MAX_MESSAGES`].
    pub async fn record_message_for_session(
        &self,
        session_id: &str,
        msg: ActorMessage,
    ) -> Result<(), RegistryError> {
        msg.validate().map_err(RegistryError::InvalidText)?;
        let retained_bytes = msg.retained_bytes().saturating_add(session_id.len());
        let mut log = self.messages.write().await;
        while !log.entries.is_empty()
            && (log.entries.len() >= MAX_MESSAGES
                || log.retained_bytes.saturating_add(retained_bytes) > MAX_MESSAGE_LOG_BYTES)
        {
            if let Some(oldest) = log.entries.pop_front() {
                log.retained_bytes = log.retained_bytes.saturating_sub(oldest.retained_bytes);
            }
        }
        if retained_bytes > MAX_MESSAGE_LOG_BYTES {
            return Err(RegistryError::InvalidText(
                "message exceeds aggregate log budget".to_string(),
            ));
        }
        log.retained_bytes = log.retained_bytes.saturating_add(retained_bytes);
        log.entries.push_back(LoggedMessage {
            session_id: session_id.to_string(),
            message: msg,
            retained_bytes,
        });
        Ok(())
    }

    /// Deliver a message to the recipient's live inbox and append it to the bounded log.
    ///
    /// Unknown or terminated actors return [`Receipt::Unreachable`]. Full inboxes return
    /// [`Receipt::Backpressured`] and drop the message. The synthetic `Root`
    /// actor is always reachable so child actors can reply to the top-level orchestrator.
    pub async fn send_message_for_session(
        &self,
        session_id: &str,
        msg: ActorMessage,
    ) -> Result<Receipt, RegistryError> {
        msg.validate().map_err(RegistryError::InvalidText)?;
        if !self.is_reachable(session_id, &msg.to).await {
            return Ok(Receipt::Unreachable);
        }

        let inbox = self.ensure_inbox(session_id, &msg.to).await;
        let retained_bytes = msg.retained_bytes();
        if !inbox.try_reserve(retained_bytes) {
            return Ok(Receipt::Backpressured);
        }
        match inbox.sender.try_send(msg.clone()) {
            Ok(()) => {
                inbox.mark_delivered_to_inbox();
                inbox.touch(msg.sent_at).await;
                if let Err(error) = self.record_message_for_session(session_id, msg).await {
                    tracing::warn!(%error, "message delivered but debug log rejected it");
                }
                Ok(Receipt::Delivered)
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                inbox.release(retained_bytes);
                Ok(Receipt::Backpressured)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                inbox.release(retained_bytes);
                Ok(Receipt::Unreachable)
            }
        }
    }

    /// Drain all pending messages for `actor_id`, optionally filtering by sender.
    pub async fn drain_inbox_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
        from: Option<&ActorId>,
    ) -> Vec<ActorMessage> {
        let inbox = self.ensure_inbox(session_id, actor_id).await;
        inbox.drain(from).await
    }

    /// Wait for the next matching message for `actor_id` until `timeout` elapses.
    pub async fn wait_for_message_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
        from: Option<&ActorId>,
        timeout: Duration,
    ) -> Option<ActorMessage> {
        let inbox = self.ensure_inbox(session_id, actor_id).await;
        inbox.wait(from, timeout).await
    }

    pub async fn unread_count_for_session(&self, session_id: &str, actor_id: &ActorId) -> usize {
        let Some(inbox) = self
            .inboxes
            .read()
            .await
            .get(&ActorKey::new(session_id, actor_id))
            .cloned()
        else {
            return 0;
        };
        inbox.unread()
    }

    pub async fn last_activity_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
    ) -> Option<DateTime<Utc>> {
        let inbox = self
            .inboxes
            .read()
            .await
            .get(&ActorKey::new(session_id, actor_id))
            .cloned()?;
        inbox.last_activity().await
    }

    /// Snapshot of all messages in the log (oldest-first).
    pub async fn messages_for_session(&self, session_id: &str) -> Vec<ActorMessage> {
        self.messages
            .read()
            .await
            .entries
            .iter()
            .filter(|logged| logged.session_id == session_id)
            .map(|logged| logged.message.clone())
            .collect()
    }

    async fn ensure_inbox(&self, session_id: &str, actor_id: &ActorId) -> Arc<ActorInbox> {
        let key = ActorKey::new(session_id, actor_id);
        if let Some(inbox) = self.inboxes.read().await.get(&key).cloned() {
            return inbox;
        }
        let mut inboxes = self.inboxes.write().await;
        inboxes.entry(key).or_insert_with(ActorInbox::new).clone()
    }

    async fn is_reachable(&self, session_id: &str, actor_id: &ActorId) -> bool {
        if actor_id.as_str() == "Root" {
            return true;
        }
        self.actors
            .read()
            .await
            .get(&ActorKey::new(session_id, actor_id))
            .is_some_and(|record| Self::is_live_status(record.status))
    }
}

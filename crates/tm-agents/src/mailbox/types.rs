use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::actor::ActorId;

/// A plain-prose message between actors (§23.2).
///
/// Protocol invariants:
/// - Plain prose only — never control-payload blobs (`{"type":"done"}` is banned).
/// - One ask per message; lead with the answer when replying; set `reply_to`.
/// - Pass large payloads by reference (`artifact://`, `memory://`), never inline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorMessage {
    pub from: ActorId,
    pub to: ActorId,
    /// Plain-prose body. The message is the interface.
    pub text: String,
    /// Set for request/reply; recipient echoes this in its reply.
    pub reply_to: Option<ActorId>,
    pub sent_at: DateTime<Utc>,
}

/// Delivery confirmation for a sent message (§23.2).
///
/// A failed receipt means the message was not enqueued — sender moves on, no retry-loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Receipt {
    Delivered,
    /// Actor unknown, dead, or otherwise unreachable.
    Unreachable,
    /// Recipient inbox is full; this send is dropped rather than silently accepted.
    Backpressured,
}

impl Receipt {
    pub fn is_delivered(self) -> bool {
        self == Self::Delivered
    }

    pub fn is_failed(self) -> bool {
        !self.is_delivered()
    }

    pub fn failure_reason(self) -> Option<&'static str> {
        match self {
            Self::Delivered => None,
            Self::Unreachable => Some("unreachable"),
            Self::Backpressured => Some("backpressured"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelActorResult {
    Cancelled,
    AlreadyCancelled,
    AlreadyTerminated,
    NotFound,
}

impl CancelActorResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::AlreadyCancelled => "already_cancelled",
            Self::AlreadyTerminated => "already_terminated",
            Self::NotFound => "not_found",
        }
    }
}

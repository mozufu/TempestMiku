use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use thiserror::Error;
use uuid::Uuid;

use crate::MemorySummaryRecord;
use crate::{
    DreamQueueRecord, NewDreamQueueRecord, NewMemorySummaryRecord, NewSkillProposalRecord,
    ProfileFactRecord, RecallChunkRecord, SkillProposalRecord, SkillProposalStatus,
};

#[derive(Debug, Error)]
pub enum MemoryStoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("store error: {0}")]
    Store(String),
}

pub type MemoryStoreResult<T> = std::result::Result<T, MemoryStoreError>;

#[async_trait]
pub trait EpisodicMemoryStore: Send + Sync {
    type Message: Send + Sync;
    type Event: Send + Sync;

    async fn session_messages(&self, session_id: Uuid) -> MemoryStoreResult<Vec<Self::Message>>;
    async fn session_events_after(
        &self,
        session_id: Uuid,
        last_event_id: Option<i64>,
    ) -> MemoryStoreResult<Vec<Self::Event>>;
}

#[async_trait]
pub trait ProfileMemoryStore: Send + Sync {
    async fn upsert_profile_fact(
        &self,
        fact: ProfileFactRecord,
    ) -> MemoryStoreResult<ProfileFactRecord>;
    async fn upsert_recall_chunk(
        &self,
        chunk: RecallChunkRecord,
    ) -> MemoryStoreResult<RecallChunkRecord>;
    async fn profile_facts(&self, subject: &str) -> MemoryStoreResult<Vec<ProfileFactRecord>>;
    async fn profile_fact(&self, subject: &str, id: Uuid) -> MemoryStoreResult<ProfileFactRecord>;
    async fn recall_chunks(
        &self,
        scope: &str,
        query: &str,
        limit: usize,
    ) -> MemoryStoreResult<Vec<RecallChunkRecord>>;
    async fn recall_chunk(&self, scope: &str, id: Uuid) -> MemoryStoreResult<RecallChunkRecord>;
}

#[async_trait]
pub trait MemorySummaryStore: Send + Sync {
    async fn upsert_memory_summary(
        &self,
        summary: NewMemorySummaryRecord,
    ) -> MemoryStoreResult<MemorySummaryRecord>;
    async fn memory_summary(&self, id: Uuid) -> MemoryStoreResult<MemorySummaryRecord>;
    async fn memory_summaries(
        &self,
        scope: &str,
        limit: usize,
    ) -> MemoryStoreResult<Vec<MemorySummaryRecord>>;
}

#[async_trait]
pub trait SkillProposalStore: Send + Sync {
    async fn upsert_skill_proposal(
        &self,
        proposal: NewSkillProposalRecord,
    ) -> MemoryStoreResult<SkillProposalRecord>;
    async fn update_skill_proposal_status(
        &self,
        id: Uuid,
        status: SkillProposalStatus,
    ) -> MemoryStoreResult<SkillProposalRecord>;
    async fn skill_proposal(&self, id: Uuid) -> MemoryStoreResult<SkillProposalRecord>;
    async fn skill_proposals_for_session(
        &self,
        session_id: Uuid,
    ) -> MemoryStoreResult<Vec<SkillProposalRecord>>;
}

#[async_trait]
pub trait DreamLeaseStore: Send + Sync {
    async fn enqueue_dream(&self, new: NewDreamQueueRecord) -> MemoryStoreResult<DreamQueueRecord>;
    async fn dream_queue_for_session(
        &self,
        session_id: Uuid,
    ) -> MemoryStoreResult<Vec<DreamQueueRecord>>;
    async fn dream_queue(
        &self,
        scope: &str,
        limit: usize,
    ) -> MemoryStoreResult<Vec<DreamQueueRecord>>;
    async fn dream(&self, dream_id: Uuid) -> MemoryStoreResult<DreamQueueRecord>;
    async fn claim_ready_dream(
        &self,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> MemoryStoreResult<Option<DreamQueueRecord>>;
    async fn heartbeat_dream(
        &self,
        dream_id: Uuid,
        now: DateTime<Utc>,
    ) -> MemoryStoreResult<DreamQueueRecord>;
    async fn complete_dream(
        &self,
        dream_id: Uuid,
        now: DateTime<Utc>,
    ) -> MemoryStoreResult<DreamQueueRecord>;
    async fn fail_dream(
        &self,
        dream_id: Uuid,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> MemoryStoreResult<DreamQueueRecord>;
}

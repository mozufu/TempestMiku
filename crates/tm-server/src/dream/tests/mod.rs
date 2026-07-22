use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use tm_host::SelfEvolutionTier;
use tm_memory::{
    DreamInputBudget, DreamLease, DreamQueueRecord, DreamReason, DreamStatus, DreamWorker,
    DreamWorkerReport, MemorySummaryKind, MemorySummaryRecord, NewDreamQueueRecord,
    NewMemorySummaryRecord, NewSkillProposalRecord, SkillProposalRecord, SkillProposalStatus,
    SkillVerification,
};
use tm_modes::{AssetStatus, ModeId, ModesConfig};
use tokio::sync::broadcast;
use uuid::Uuid;

use super::worker::SenderFactory;
use super::*;
use crate::store::StoreRuntimeMetrics;
use crate::{
    ApprovalBroker, ApprovalEffectLease, ApprovalEffectRecord, ApprovalRequestRecord,
    ApprovalResolveDecision, CronJobRecord, CronLease, CronRunRecord, EndSessionDreamResult,
    InMemoryStore, MemoryWriteStatus, MessageRecord, NewApprovalRequest, NewApprovalResolution,
    NewCronJobRecord, NewCronRunRecord, NewProjectItem, NewSession, ProfileFactRecord,
    ProjectItemKind, ProjectItemRecord, RecallChunkRecord, ResolveApprovalRequest, Result,
    ServerError, SessionEvent, SessionRecord, SessionSummaryRecord, SessionTurnRecord, Store,
    StoreCodingEventSink,
};

mod approval_effects;
mod daemon_budget;
mod evolution;
mod failure_modes;
mod skill_proposals;
mod summaries;
mod support;
mod worker_basics;

mod actor_smoke;
mod client;
mod config;
mod evidence;
mod record;
mod speaker;
mod sse;
mod workflow;

pub use actor_smoke::{ActorSmokeReport, run_actor_smoke};
pub use client::{MikuClient, SessionInfo};
pub use config::E2eConfig;
pub use evidence::*;
pub use record::*;
pub use speaker::{E2eSpeaker, LiveSpeaker, ScriptedSpeaker, WorkflowContext, WorkflowStep};
pub use sse::{E2eEvent, parse_sse_block};
pub use workflow::{
    ConversationRound, WORKFLOW_RECORD_SCHEMA_VERSION, WorkflowOptions, WorkflowRecord,
    WorkflowReport, run_workflow, write_workflow_record,
};

#[cfg(test)]
mod tests;

mod actor_smoke;
mod client;
mod config;
mod drive_smoke;
mod evidence;
mod native_coding;
mod record;
mod speaker;
mod sse;
mod voice_eval;
mod workflow;

pub use actor_smoke::{ActorSmokeReport, run_actor_smoke};
pub use client::{MikuClient, SessionInfo};
pub use config::{E2eConfig, load_dotenv};
pub use drive_smoke::{DriveSmokeReport, run_drive_smoke};
pub use evidence::*;
pub use native_coding::run_record_native_coding;
pub use record::*;
pub use speaker::{E2eSpeaker, LiveSpeaker, ScriptedSpeaker, WorkflowContext, WorkflowStep};
pub use sse::{E2eEvent, parse_sse_block};
pub use voice_eval::{
    P2_VOICE_LIVE_SCHEMA_VERSION, P2VoiceLiveCase, P2VoiceLiveReport, run_p2_voice_live_eval,
    write_p2_voice_live_report,
};
pub use workflow::{
    ConversationRound, WORKFLOW_RECORD_SCHEMA_VERSION, WorkflowOptions, WorkflowRecord,
    WorkflowReport, run_workflow, write_workflow_record,
};

#[cfg(test)]
mod tests;

mod assets;
mod evolution;
mod prompt;
mod skills;
mod types;

pub use assets::{AssetStatus, ModeAssets, ModesConfig};
pub use evolution::{
    MAX_REVIEW_METADATA_BYTES, MAX_REVIEW_PROPOSAL_CHANGES, REVIEW_PROPOSAL_WIRE_VERSION,
    ReviewAddendumChange, ReviewAddendumSection, ReviewApplyContract, ReviewMetadata,
    ReviewProposalStatus, ReviewProposalTarget,
};
pub use prompt::ComposedPrompt;
pub use skills::resolve_active_skills;
pub use types::{Mode, ModeCatalog, ModeId, ModeProfile, ModeRoute, SkillActivation, SkillTrigger};

#[cfg(test)]
pub(crate) use assets::MISSING_SKILL_PROMPT_FALLBACK;

pub const KNOWN_SKILLS: &[&str] = &[
    "miku-voice",
    "ambiguity-grill",
    "negative-state-grounding",
    "oh-my-pi-handoff",
    "personal-assistant-state-capture",
    "scope-guard",
    "weekly-ship-ledger",
    "serious-engineer-ops",
];

#[cfg(test)]
mod tests;

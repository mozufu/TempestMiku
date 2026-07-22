mod config;
mod evolution;
pub(crate) mod proposals;
mod summary;
mod util;
mod worker;

pub use config::{
    DreamModelRoles, DreamRedactionConfig, DreamSummaryCadence, DreamWorkerConfig,
    EvolutionDreamConfig,
};
pub use worker::{DreamWorkerDaemon, DreamWorkerDaemonHandle, SenderFactory, ServerDreamWorker};

#[cfg(test)]
mod tests;

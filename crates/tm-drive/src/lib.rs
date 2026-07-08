//! Local-first TempestMiku drive.
//!
//! The crate owns concrete P5 drive behavior: metadata records, in-memory store,
//! deterministic transducers, virtual-directory mapping, organizer proposals,
//! host-call registration, and the `drive://` resource handler.

pub mod organize;
pub mod policy;
pub mod resources;
pub mod store;
pub mod transduce;
pub mod types;
pub mod vdir;

pub use organize::{
    apply_tags, generate_organizer_proposals, generate_organizer_proposals_for_run, propose_path,
    slug, title_filename,
};
pub use policy::{drive_link_plan, drive_link_policy, memory_scope_for_project};
pub use resources::DriveResourceHandler;
pub use store::{DriveRead, InMemoryDriveStore, register_drive_functions};
pub use transduce::{Transducer, TransducerInput, Transduction, transduce_document};
pub use types::*;
pub use vdir::{drive_uri_path, parse_virtual_dir};

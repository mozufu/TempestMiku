use std::{
    collections::BTreeMap,
    env, fs,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tm_artifacts::{ArtifactRef, ArtifactStore, ResourceContent, preview};

use crate::{
    HostError, HostFn, HostRegistry, InvocationCtx, ResourceEntry, ResourceHandler,
    ResourceRegistry, Result, ToolDocs,
};

use super::util::{
    apply_line_hunks, collect_files, compile_globs, display_path, ensure_rw, file_tag, fs_entry,
    linked_uri, list_entries, load_simple_gitignore, parse_args, parse_linked_path, simple_diff,
    validate_command_name,
};
use super::{LinkedFolders, docs::docs};

mod code;
#[path = "fs.rs"]
mod fs_tools;
mod proc;
mod resources;

pub(in crate::linked) use code::CodeSearchFn;
pub use fs_tools::FsEntry;
pub(in crate::linked) use fs_tools::{
    FsFindFn, FsLsFn, FsMoveFn, FsPatchFn, FsReadFn, FsRemoveFn, FsWriteFn, PatchHunk,
};
pub(in crate::linked) use proc::ProcRunFn;
pub use resources::LinkedResourceHandler;

pub fn register_p0_linked_folder_functions(
    host_registry: &mut HostRegistry,
    resource_registry: &mut ResourceRegistry,
    linked_folders: LinkedFolders,
    artifact_store: ArtifactStore,
    proc_run_timeout: Duration,
) {
    host_registry.register(Arc::new(FsReadFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(FsWriteFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(FsPatchFn::new(
        linked_folders.clone(),
        artifact_store.clone(),
    )));
    host_registry.register(Arc::new(FsMoveFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(FsRemoveFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(FsLsFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(FsFindFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(CodeSearchFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(ProcRunFn::with_timeout_ms(
        linked_folders.clone(),
        artifact_store,
        u64::try_from(proc_run_timeout.as_millis()).unwrap_or(u64::MAX),
    )));
    resource_registry.register(Arc::new(LinkedResourceHandler::new(linked_folders)));
}

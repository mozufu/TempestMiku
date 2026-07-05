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
    linked_uri, list_entries, load_simple_gitignore, parse_args, simple_diff, stdin_present,
    validate_command_name,
};
use super::{LinkedFolders, docs::docs};

mod code;
#[path = "fs.rs"]
mod fs_tools;
mod proc;
mod resources;

pub(in crate::linked) use code::{CodeEditFn, CodeSearchFn, InsertAt, PatchHunk};
pub use fs_tools::FsEntry;
pub(in crate::linked) use fs_tools::{FsFindFn, FsLsFn, FsReadFn, FsWriteFn};
pub(in crate::linked) use proc::ProcRunFn;
pub use resources::LinkedResourceHandler;

pub fn register_p0_linked_folder_functions(
    host_registry: &mut HostRegistry,
    resource_registry: &mut ResourceRegistry,
    linked_folders: LinkedFolders,
    artifact_store: ArtifactStore,
) {
    host_registry.register(Arc::new(FsReadFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(FsWriteFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(FsLsFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(FsFindFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(CodeSearchFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(CodeEditFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(ProcRunFn::new(
        linked_folders.clone(),
        artifact_store,
    )));
    resource_registry.register(Arc::new(LinkedResourceHandler::new(linked_folders)));
}

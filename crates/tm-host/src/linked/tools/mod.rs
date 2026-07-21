use std::{
    collections::BTreeMap,
    env,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tm_artifacts::{ArtifactRef, ArtifactStore, ResourceContent, preview};

use crate::{
    HostError, HostFn, HostRegistry, InvocationCtx, ResourceEntry, ResourceHandler,
    ResourceRegistry, Result, ToolDocs,
};

use super::isolation::ProcIsolationConfig;
use super::secure_fs::{
    SecureKind, WalkControl, WalkOptions, atomic_replace, create_file, open_entry_file,
    open_existing, open_parent, read_bounded, read_entry_bounded, remove_entry, rename_entry,
    stat_entry, walk_secure,
};
use super::util::{
    MAX_FS_RESULT_LIMIT, MAX_FS_WALK_ENTRIES, MAX_MUTATION_FILE_BYTES, MAX_SEARCH_CONTEXT_LINES,
    MAX_SEARCH_FILE_BYTES, MAX_SEARCH_PATHS, MAX_SEARCH_PATTERN_BYTES, MAX_SEARCH_RESULT_BYTES,
    MAX_SEARCH_TOTAL_BYTES, apply_line_hunks, approval_action, compile_globs, display_path,
    ensure_rw, file_tag, fs_entry_from_secure, linked_uri, list_entries, load_simple_gitignore,
    parse_args, parse_linked_path, push_bounded_fs_entry, simple_diff, validate_command_name,
    validate_result_limit,
};
use super::{LinkedFolders, docs::docs};

#[path = "fs.rs"]
mod fs_tools;
mod git;
mod proc;
mod resources;

pub use fs_tools::FsEntry;
pub(in crate::linked) use fs_tools::{
    FsFindFn, FsGrepFn, FsLsFn, FsMoveFn, FsPatchFn, FsReadFn, FsRemoveFn, FsWriteFn, PatchHunk,
};
pub(in crate::linked) use git::GitReadFn;
pub(in crate::linked) use proc::ProcRunFn;
pub use resources::LinkedResourceHandler;

pub fn register_p0_linked_folder_functions(
    host_registry: &mut HostRegistry,
    resource_registry: &mut ResourceRegistry,
    linked_folders: LinkedFolders,
    artifact_store: ArtifactStore,
    proc_run_timeout: Duration,
) {
    register_p0_linked_folder_functions_with_isolation(
        host_registry,
        resource_registry,
        linked_folders,
        artifact_store,
        proc_run_timeout,
        ProcIsolationConfig::default(),
    );
}

pub fn register_p0_linked_folder_functions_with_isolation(
    host_registry: &mut HostRegistry,
    resource_registry: &mut ResourceRegistry,
    linked_folders: LinkedFolders,
    artifact_store: ArtifactStore,
    proc_run_timeout: Duration,
    proc_isolation: ProcIsolationConfig,
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
    host_registry.register(Arc::new(FsGrepFn::new(linked_folders.clone())));
    host_registry.register(Arc::new(GitReadFn::status(
        linked_folders.clone(),
        artifact_store.clone(),
        docs(
            "git.status",
            "git",
            "Inspect linked repository status",
            true,
        ),
    )));
    host_registry.register(Arc::new(GitReadFn::diff(
        linked_folders.clone(),
        artifact_store.clone(),
        docs("git.diff", "git", "Inspect linked repository diff", true),
    )));
    host_registry.register(Arc::new(GitReadFn::grep(
        linked_folders.clone(),
        artifact_store.clone(),
        docs("git.grep", "git", "Search tracked repository content", true),
    )));
    host_registry.register(Arc::new(GitReadFn::log(
        linked_folders.clone(),
        artifact_store.clone(),
        docs("git.log", "git", "Inspect recent repository history", true),
    )));
    host_registry.register(Arc::new(GitReadFn::show(
        linked_folders.clone(),
        artifact_store.clone(),
        docs("git.show", "git", "Inspect a snapshotted Git object", true),
    )));
    host_registry.register(Arc::new(GitReadFn::clone(
        linked_folders.clone(),
        artifact_store.clone(),
        docs(
            "git.clone",
            "git",
            "Clone HTTPS into an empty linked folder",
            true,
        ),
    )));
    host_registry.register(Arc::new(GitReadFn::init(
        linked_folders.clone(),
        artifact_store.clone(),
        docs("git.init", "git", "Initialize a linked repository", true),
    )));
    host_registry.register(Arc::new(GitReadFn::add(
        linked_folders.clone(),
        artifact_store.clone(),
        docs("git.add", "git", "Stage exact repository paths", true),
    )));
    host_registry.register(Arc::new(GitReadFn::mv(
        linked_folders.clone(),
        artifact_store.clone(),
        docs("git.mv", "git", "Move an exact repository entry", true),
    )));
    host_registry.register(Arc::new(GitReadFn::restore(
        linked_folders.clone(),
        artifact_store.clone(),
        docs("git.restore", "git", "Restore exact repository paths", true),
    )));
    host_registry.register(Arc::new(GitReadFn::rm(
        linked_folders.clone(),
        artifact_store.clone(),
        docs("git.rm", "git", "Remove exact repository paths", true),
    )));
    host_registry.register(Arc::new(GitReadFn::bisect(
        linked_folders.clone(),
        artifact_store.clone(),
        docs(
            "git.bisect",
            "git",
            "Advance a closed bisect transition",
            true,
        ),
    )));
    host_registry.register(Arc::new(GitReadFn::commit(
        linked_folders.clone(),
        artifact_store.clone(),
        docs("git.commit", "git", "Commit the staged index", true),
    )));
    host_registry.register(Arc::new(GitReadFn::push(
        linked_folders.clone(),
        artifact_store.clone(),
        docs(
            "git.push",
            "git",
            "Push HEAD to the current HTTPS upstream",
            true,
        ),
    )));
    host_registry.register(Arc::new(GitReadFn::pull(
        linked_folders.clone(),
        artifact_store.clone(),
        docs(
            "git.pull",
            "git",
            "Fast-forward from the current HTTPS upstream",
            true,
        ),
    )));
    host_registry.register(Arc::new(ProcRunFn::with_timeout_and_isolation(
        linked_folders.clone(),
        artifact_store,
        u64::try_from(proc_run_timeout.as_millis()).unwrap_or(u64::MAX),
        proc_isolation,
    )));
    resource_registry.register(Arc::new(LinkedResourceHandler::new(linked_folders)));
}

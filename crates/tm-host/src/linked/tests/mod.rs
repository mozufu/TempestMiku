use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tm_artifacts::ArtifactStore;

use crate::{
    ApprovalDecision, ApprovalPolicy, ArtifactResourceHandler, CapabilityGrants,
    DefaultDenyApprovalPolicy, HostError, HostFn, HostRegistry, InvocationCtx, ResourceRegistry,
    Result, ToolDocs,
};

use super::*;
use super::{
    docs::docs,
    secure_fs::MAX_SECURE_WALK_DEPTH,
    tools::{
        CodeSearchFn, FsFindFn, FsLsFn, FsMoveFn, FsPatchFn, FsReadFn, FsRemoveFn, FsWriteFn,
        ProcRunFn,
    },
    util::{
        MAX_FS_RESULT_BYTES, MAX_GLOB_PATTERN_BYTES, MAX_GLOB_PATTERNS, MAX_GLOB_TOTAL_BYTES,
        MAX_MUTATION_FILE_BYTES, file_tag, load_simple_gitignore, push_bounded_fs_entry,
    },
};

mod foundation;
mod fs_policy;
mod mutations;
mod proc_approval;
mod proc_runtime;
mod support;

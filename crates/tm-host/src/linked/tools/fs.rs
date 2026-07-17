use std::sync::atomic::AtomicU64;

const PATCH_DIFF_PREVIEW_BYTES: usize = 12 * 1024;
const MAX_PATCH_ARTIFACT_BYTES: usize = 4 * 1024 * 1024 - 256;
static PATCH_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[path = "fs/list_find.rs"]
mod list_find;
#[path = "fs/move_entry.rs"]
mod move_entry;
#[path = "fs/patch.rs"]
mod patch;
#[path = "fs/read_write.rs"]
mod read_write;
#[path = "fs/remove.rs"]
mod remove;

pub use list_find::FsEntry;
pub(in crate::linked) use list_find::{FsFindFn, FsLsFn};
pub(in crate::linked) use move_entry::FsMoveFn;
pub(in crate::linked) use patch::{FsPatchFn, PatchHunk};
pub(in crate::linked) use read_write::{FsReadFn, FsWriteFn};
pub(in crate::linked) use remove::FsRemoveFn;

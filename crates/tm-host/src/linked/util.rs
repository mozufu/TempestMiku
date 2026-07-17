mod listing;
mod patch;
mod path;
mod text;

pub(super) use listing::{
    compile_globs, file_tag, fs_entry_from_secure, list_entries, load_simple_gitignore,
    push_bounded_fs_entry, validate_result_limit,
};
pub(super) use patch::{apply_line_hunks, simple_diff};
pub(super) use path::{
    display_path, ensure_rw, linked_uri, parse_args, parse_linked_path, validate_alias,
    validate_command_name,
};
pub(super) use text::{approval_action, read_text_page_from_file};

pub(super) const DEFAULT_FS_RESULT_LIMIT: usize = 1_000;
pub(super) const MAX_FS_RESULT_LIMIT: usize = 10_000;
pub(super) const MAX_FS_WALK_ENTRIES: usize = 100_000;
pub(super) const MAX_FS_RESULT_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_GLOB_PATTERNS: usize = 64;
pub(super) const MAX_GLOB_PATTERN_BYTES: usize = 4 * 1024;
pub(super) const MAX_GLOB_TOTAL_BYTES: usize = 64 * 1024;
pub(super) const MAX_SEARCH_PATHS: usize = 64;
pub(super) const MAX_SEARCH_CONTEXT_LINES: usize = 20;
pub(super) const MAX_SEARCH_PATTERN_BYTES: usize = 16 * 1024;
pub(super) const MAX_SEARCH_FILE_BYTES: u64 = 4 * 1024 * 1024;
pub(super) const MAX_SEARCH_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
pub(super) const MAX_SEARCH_RESULT_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_MUTATION_FILE_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_APPROVAL_ACTION_BYTES: usize = 4 * 1024;

//! Shared helper functions used by multiple commands. Mirrors the Rust
//! side of `cli/cmd/helpers.go`. (Lower-level helpers live in
//! `crate::util::text` / `crate::util::id` / `crate::util::time`.)

use crate::store::DraftRecord;

/// Convert a parsed selection into the underlying draft slice.
pub fn parse_selection(input: &str, all: &[DraftRecord]) -> Vec<DraftRecord> {
    let indices = crate::util::text::parse_selection_indices(input, all.len());
    indices.into_iter().map(|i| all[i].clone()).collect()
}

pub fn draft_ids(drafts: &[DraftRecord]) -> Vec<String> {
    drafts.iter().map(|d| d.id.clone()).collect()
}

/// Returns the current working-tree's branch name, or empty string when
/// detached. Now a thin wrapper over `shared::git::get_branch("")` —
/// kept as its own helper so call sites don't have to think about the
/// detached-HEAD normalization.
pub fn current_branch() -> String {
    let b = crate::sources::shared::git::git_output(&["rev-parse", "--abbrev-ref", "HEAD"]);
    if b == "HEAD" {
        String::new()
    } else {
        b
    }
}

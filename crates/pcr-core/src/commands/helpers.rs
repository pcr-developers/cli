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

/// Default cap on how many drafts the interactive browser shows
/// without `--all`. Older drafts stay reachable via `--all` or
/// `pcr gc --drafts-older-than`.
pub const DEFAULT_RECENT_DRAFTS_CAP: usize = 100;

/// Trim a draft list (assumed sorted ascending by `captured_at`) to
/// the newest `cap` entries. Returns the kept slice and how many
/// were hidden.
pub fn cap_recent_drafts(drafts: Vec<DraftRecord>, cap: usize) -> (Vec<DraftRecord>, usize) {
    if drafts.len() <= cap {
        return (drafts, 0);
    }
    let hidden = drafts.len() - cap;
    let kept = drafts.into_iter().skip(hidden).collect();
    (kept, hidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(id: &str) -> DraftRecord {
        DraftRecord {
            id: id.into(),
            ..DraftRecord::default()
        }
    }

    #[test]
    fn cap_keeps_full_list_under_threshold() {
        let drafts = vec![d("a"), d("b"), d("c")];
        let (kept, hidden) = cap_recent_drafts(drafts, 10);
        assert_eq!(hidden, 0);
        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn cap_drops_oldest_when_over_threshold() {
        let drafts = vec![d("a"), d("b"), d("c"), d("d"), d("e")];
        let (kept, hidden) = cap_recent_drafts(drafts, 3);
        assert_eq!(hidden, 2);
        // Oldest two ("a","b") are dropped; newest three ("c","d","e") kept.
        assert_eq!(
            kept.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["c", "d", "e"]
        );
    }

    #[test]
    fn cap_at_exact_size_is_a_noop() {
        let drafts = vec![d("a"), d("b")];
        let (kept, hidden) = cap_recent_drafts(drafts, 2);
        assert_eq!(hidden, 0);
        assert_eq!(kept.len(), 2);
    }
}

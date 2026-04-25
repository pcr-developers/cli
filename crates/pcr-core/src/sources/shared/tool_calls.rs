//! Tool-call utilities — extracting file paths and using them to attribute
//! captured prompts to registered projects.
//!
//! All matching here goes through `shared::path_norm` so we get
//! symlink-resolved, `~`-expanded, relative-resolved canonical paths on
//! both sides of the comparison. The "raw prefix match" of an earlier
//! version produced silent attribution holes for users with symlinked
//! workspaces or tools that emitted relative paths (EV-1 in the
//! multi-repo audit).

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::path_norm::{normalize_path, path_is_under};

/// Returns every registered project ID whose canonical path is a parent of
/// any tool-call path. Both sides are canonicalized before comparison so
/// symlinked / relative / `~`-prefixed paths attribute correctly.
///
/// `proj_by_id` MUST already contain canonical project paths. Build it
/// with [`super::path_norm::proj_id_to_canonical_paths`].
///
/// `cwd` is the working directory the source watcher knows about (Cursor
/// project path, Claude Code session cwd, VS Code workspace root). It's
/// used to resolve relative tool-call paths. Pass `None` when the source
/// has no cwd; relative paths will then be silently dropped.
pub fn touched_project_ids(
    tool_calls: &[Value],
    proj_by_id: &BTreeMap<String, String>,
    cwd: Option<&str>,
) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    for tc in tool_calls {
        let Some(raw) = extract_path_from_tool_call(tc) else {
            continue;
        };
        let Some(abs) = normalize_path(&raw, cwd) else {
            continue;
        };
        for (id, canon_path) in proj_by_id {
            if path_is_under(&abs, canon_path) {
                seen.insert(id.clone());
            }
        }
    }
    let mut v: Vec<String> = seen.into_iter().collect();
    v.sort();
    v
}

/// Returns git snapshots (head_sha + git_diff + branch) for every
/// non-primary repo referenced by tool-call paths.
///
/// Used by the watchers to populate `file_context.repo_snapshots`, which
/// the push pipeline then consumes to produce per-repo incremental diffs
/// in the review payload (`compute_incremental_diffs` in `commands/push.rs`).
///
/// Each snapshot is `{ head_sha, git_diff, branch }` so a reviewer can see
/// not just *what* changed in the secondary repo but *which branch* it was
/// on at the time of the prompt — important for users who switch branches
/// mid-session.
///
/// `proj_by_id` MUST contain canonical paths (use
/// [`super::path_norm::proj_id_to_canonical_paths`]).
pub fn repo_snapshots(
    tool_calls: &[Value],
    primary_project_id: &str,
    proj_by_id: &BTreeMap<String, String>,
    cwd: Option<&str>,
) -> Option<serde_json::Map<String, Value>> {
    use super::git::{get_branch, get_git_diff, get_head_sha};
    let mut result = serde_json::Map::new();
    for tc in tool_calls {
        let Some(raw) = extract_path_from_tool_call(tc) else {
            continue;
        };
        let Some(abs) = normalize_path(&raw, cwd) else {
            continue;
        };
        for (id, canon_path) in proj_by_id {
            if id == primary_project_id || canon_path.is_empty() {
                continue;
            }
            if !path_is_under(&abs, canon_path) {
                continue;
            }
            if result.contains_key(id) {
                continue;
            }
            let mut snap = serde_json::Map::new();
            snap.insert("head_sha".into(), Value::String(get_head_sha(canon_path)));
            snap.insert("git_diff".into(), Value::String(get_git_diff(canon_path)));
            // Capture branch alongside the diff so reviewers see which
            // branch the secondary repo was on. Without this, multi-repo
            // reviews silently fall back to the primary repo's branch
            // (BR-1 in the multi-repo audit).
            snap.insert("branch".into(), Value::String(get_branch(canon_path)));
            result.insert(id.clone(), Value::Object(snap));
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Like [`repo_snapshots`] but takes the secondary project IDs directly
/// instead of deriving them from tool calls. Used by the Cursor watcher,
/// which doesn't expose tool calls in its bubble data — secondary repos
/// are detected via the `diff_events` table instead, leaving us with a
/// list of project IDs that need to be snapshotted.
///
/// `proj_by_id_canonical` is the same map [`repo_snapshots`] takes; the
/// primary id is skipped so the result mirrors that helper's contract
/// (only secondary repos appear).
pub fn repo_snapshots_for_ids(
    primary_project_id: &str,
    touched_ids: &[String],
    proj_by_id_canonical: &BTreeMap<String, String>,
) -> Option<serde_json::Map<String, Value>> {
    use super::git::{get_branch, get_git_diff, get_head_sha};
    let mut result = serde_json::Map::new();
    for id in touched_ids {
        if id.is_empty() || id == primary_project_id {
            continue;
        }
        let Some(canon_path) = proj_by_id_canonical.get(id) else {
            continue;
        };
        if canon_path.is_empty() || result.contains_key(id) {
            continue;
        }
        let mut snap = serde_json::Map::new();
        snap.insert("head_sha".into(), Value::String(get_head_sha(canon_path)));
        snap.insert("git_diff".into(), Value::String(get_git_diff(canon_path)));
        snap.insert("branch".into(), Value::String(get_branch(canon_path)));
        result.insert(id.clone(), Value::Object(snap));
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Files written by tool calls (`write_file`, `edit_file`, `Write`, etc.).
/// Returns paths verbatim from the tool call (no normalization). Callers
/// that need canonical paths for matching should run them through
/// [`super::path_norm::normalize_path`].
pub fn changed_files_from_tool_calls(tool_calls: &[Value]) -> Vec<String> {
    let write_tools: BTreeSet<&str> = [
        "write_file",
        "create_file",
        "edit_file",
        "replace_string_in_file",
        "multi_replace_string_in_file",
        "edit_notebook_file",
        "Write",
    ]
    .into_iter()
    .collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut files: Vec<String> = Vec::new();
    for tc in tool_calls {
        let tool = tc.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        if !write_tools.contains(tool) {
            continue;
        }
        if let Some(p) = extract_path_from_tool_call(tc) {
            if seen.insert(p.clone()) {
                files.push(p);
            }
        }
    }
    files
}

pub fn extract_path_from_tool_call(tc: &Value) -> Option<String> {
    if let Some(input) = tc.get("input").and_then(|v| v.as_object()) {
        for key in ["path", "file_path", "filePath"] {
            if let Some(p) = input.get(key).and_then(|v| v.as_str()) {
                if !p.is_empty() {
                    return Some(p.to_string());
                }
            }
        }
    }
    if let Some(p) = tc.get("path").and_then(|v| v.as_str()) {
        if !p.is_empty() {
            return Some(p.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tc(path: &str) -> Value {
        json!({"tool": "Read", "input": {"path": path}})
    }

    #[test]
    fn touched_project_ids_attributes_via_canonical_path() {
        let mut by_id = BTreeMap::new();
        by_id.insert("p1".to_string(), "/repo/p1".to_string());
        by_id.insert("p2".to_string(), "/repo/p2".to_string());
        let calls = vec![
            tc("/repo/p1/src/main.rs"),
            tc("/repo/p2/lib.rs"),
            tc("/repo/p1/README.md"),
        ];
        let ids = touched_project_ids(&calls, &by_id, None);
        assert_eq!(ids, vec!["p1", "p2"]);
    }

    #[test]
    fn touched_project_ids_does_not_match_partial_segments() {
        // EV-1 regression: registered project `/repo/p1`, tool call against
        // `/repo/p1-fork/foo`. Naive starts_with would match. Canonical
        // path_is_under requires the trailing-slash boundary.
        let mut by_id = BTreeMap::new();
        by_id.insert("p1".to_string(), "/repo/p1".to_string());
        let calls = vec![tc("/repo/p1-fork/foo.rs")];
        assert!(touched_project_ids(&calls, &by_id, None).is_empty());
    }

    #[test]
    fn touched_project_ids_resolves_relative_paths_via_cwd() {
        let mut by_id = BTreeMap::new();
        by_id.insert("p1".to_string(), "/proj".to_string());
        let calls = vec![tc("./src/main.rs")];
        // With cwd, the relative path resolves under /proj.
        assert_eq!(
            touched_project_ids(&calls, &by_id, Some("/proj")),
            vec!["p1"]
        );
        // Without cwd, the relative path can't be resolved and is dropped.
        assert!(touched_project_ids(&calls, &by_id, None).is_empty());
    }

    #[test]
    fn touched_project_ids_dedupes_across_calls() {
        let mut by_id = BTreeMap::new();
        by_id.insert("p1".to_string(), "/proj".to_string());
        let calls = vec![tc("/proj/a.rs"), tc("/proj/b.rs"), tc("/proj/c.rs")];
        assert_eq!(touched_project_ids(&calls, &by_id, None), vec!["p1"]);
    }

    #[test]
    fn extract_path_from_tool_call_handles_three_shapes() {
        // `input.path`
        assert_eq!(
            extract_path_from_tool_call(&json!({"input": {"path": "/a"}})),
            Some("/a".into())
        );
        // `input.file_path`
        assert_eq!(
            extract_path_from_tool_call(&json!({"input": {"file_path": "/b"}})),
            Some("/b".into())
        );
        // `input.filePath` (camelCase from VS Code)
        assert_eq!(
            extract_path_from_tool_call(&json!({"input": {"filePath": "/c"}})),
            Some("/c".into())
        );
        // top-level `path`
        assert_eq!(
            extract_path_from_tool_call(&json!({"path": "/d"})),
            Some("/d".into())
        );
        // empty string is dropped
        assert_eq!(
            extract_path_from_tool_call(&json!({"input": {"path": ""}})),
            None
        );
        // missing entirely
        assert_eq!(extract_path_from_tool_call(&json!({"tool": "Other"})), None);
    }

    #[test]
    fn repo_snapshots_for_ids_skips_primary_and_unknown_ids() {
        // Build a project map with two real entries.
        let mut map = BTreeMap::new();
        map.insert("primary".to_string(), "/repo/primary".to_string());
        map.insert("secondary".to_string(), "/repo/secondary".to_string());

        // Pretend a Cursor turn touched both. Primary should be excluded
        // from the snapshot output (the row already carries it as
        // git_diff/head_sha). Secondary should be included.
        let touched = vec!["primary".to_string(), "secondary".to_string()];
        let result = repo_snapshots_for_ids("primary", &touched, &map);
        // We don't have real git repos in /repo/* — but the structure of
        // the response is what we're testing, not the contents.
        let snaps = result.expect("secondary should produce a snapshot entry");
        assert_eq!(snaps.len(), 1, "primary must be excluded");
        assert!(snaps.contains_key("secondary"));
        // Each snapshot must carry head_sha + git_diff + branch keys
        // (BR-1) so push's compute_incremental_diffs has everything it
        // needs without re-querying anything.
        let snap = snaps.get("secondary").unwrap().as_object().unwrap();
        assert!(snap.contains_key("head_sha"));
        assert!(snap.contains_key("git_diff"));
        assert!(snap.contains_key("branch"));
    }

    #[test]
    fn repo_snapshots_for_ids_returns_none_when_no_secondaries() {
        let mut map = BTreeMap::new();
        map.insert("primary".to_string(), "/repo/primary".to_string());
        // Touched only the primary → no secondary work for the helper.
        assert!(repo_snapshots_for_ids("primary", &["primary".to_string()], &map).is_none());
        // No touched ids at all → None.
        assert!(repo_snapshots_for_ids("primary", &[], &map).is_none());
    }

    #[test]
    fn repo_snapshots_for_ids_skips_empty_ids_and_unknown_projects() {
        let mut map = BTreeMap::new();
        map.insert("known".to_string(), "/repo/known".to_string());
        // "" id, "unknown" id (not in map), and the only valid one.
        let touched = vec!["".to_string(), "unknown".to_string(), "known".to_string()];
        let snaps =
            repo_snapshots_for_ids("primary", &touched, &map).expect("known should still snapshot");
        assert_eq!(snaps.len(), 1);
        assert!(snaps.contains_key("known"));
    }

    #[test]
    fn changed_files_from_tool_calls_only_includes_write_tools() {
        let calls = vec![
            json!({"tool": "Read", "input": {"path": "/a.rs"}}),
            json!({"tool": "Write", "input": {"path": "/b.rs"}}),
            json!({"tool": "Edit", "input": {"path": "/c.rs"}}),
            json!({"tool": "edit_file", "input": {"path": "/d.rs"}}),
            json!({"tool": "Bash", "input": {"command": "ls"}}),
        ];
        let files = changed_files_from_tool_calls(&calls);
        // Only Write and edit_file from our allowlist.
        assert_eq!(files, vec!["/b.rs", "/d.rs"]);
    }
}

//! Tool-call utilities — extracting file paths and using them to attribute
//! captured prompts to registered projects.
//!
//! All matching here goes through `shared::path_norm` so both sides of
//! the comparison are symlink-resolved, `~`-expanded, and relative-
//! resolved canonical paths. A naive `path.starts_with(project + "/")`
//! check would miss attribution for symlinked workspaces and for tool
//! calls that emit relative or `~`-prefixed paths.

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
        // Iterate every path in a tool call, not just the first.
        // Multi-file shapes (`apply-patch`, `replace_string_in_file`'s
        // array form, experimental Cursor tools) bundle several files
        // per call; without iterating, only the first attributes.
        for raw in extract_paths_from_tool_call(tc) {
            let Some(abs) = normalize_path(&raw, cwd) else {
                continue;
            };
            for (id, canon_path) in proj_by_id {
                if path_is_under(&abs, canon_path) {
                    seen.insert(id.clone());
                }
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
/// Watchers populate `file_context.repo_snapshots` from this; the push
/// pipeline (`compute_incremental_diffs` in `commands/push.rs`) then
/// emits per-repo incremental diffs in the review payload.
///
/// Each snapshot is `{ head_sha, git_diff, branch }` so reviewers see
/// not just *what* changed in the secondary repo but *which branch* it
/// was on at the time of the prompt. Branch matters because users
/// frequently switch branches mid-session and the primary repo's
/// branch alone tells reviewers nothing about where the secondary
/// changes landed.
///
/// `proj_by_id` MUST contain canonical paths — use
/// [`super::path_norm::proj_id_to_canonical_paths`].
pub fn repo_snapshots(
    tool_calls: &[Value],
    primary_project_id: &str,
    proj_by_id: &BTreeMap<String, String>,
    cwd: Option<&str>,
) -> Option<serde_json::Map<String, Value>> {
    use super::git::{get_branch, get_git_diff, get_head_sha};
    let mut result = serde_json::Map::new();
    for tc in tool_calls {
        for raw in extract_paths_from_tool_call(tc) {
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
                // Reviewers need the per-secondary-repo branch — without
                // it, multi-repo reviews show only the primary repo's
                // branch and the secondary diffs lose their context.
                snap.insert("branch".into(), Value::String(get_branch(canon_path)));
                result.insert(id.clone(), Value::Object(snap));
            }
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
/// which doesn't expose tool calls in its bubble data — Cursor secondary
/// repos are detected via the `diff_events` table, which yields a list
/// of project IDs ready to be snapshotted.
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
    extract_paths_from_tool_call(tc).into_iter().next()
}

/// Returns every file path mentioned by a tool call. Multi-file shapes
/// seen in production transcripts:
///
/// - `input.path`, `input.file_path`, `input.filePath` — single-file edits.
/// - `input.files: [{path: ...}]` — apply-patch style tool calls that
///   touch multiple files in a single invocation.
/// - `input.fileNames: [...]` — VS Code's `replace_string_in_file` and
///   a handful of newer Cursor agent tools list targets this way.
/// - `input.targets: [...]` — some experimental Cursor tools.
/// - top-level `path` — legacy shape.
///
/// All attribution helpers in this module iterate every path returned
/// here. A single-path extractor would silently miss the trailing files
/// in a multi-file edit, leaving secondary projects untagged and their
/// snapshots / per-prompt diffs absent from review.
pub fn extract_paths_from_tool_call(tc: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut push_if_nonempty = |s: Option<&str>| {
        if let Some(s) = s.filter(|x| !x.is_empty()) {
            out.push(s.to_string());
        }
    };

    if let Some(input) = tc.get("input").and_then(|v| v.as_object()) {
        // Single-file scalar shapes.
        for key in ["path", "file_path", "filePath", "fileName", "filename"] {
            push_if_nonempty(input.get(key).and_then(|v| v.as_str()));
        }
        // Array-of-strings shapes.
        for key in ["fileNames", "filenames", "files", "paths", "targets"] {
            if let Some(arr) = input.get(key).and_then(|v| v.as_array()) {
                for v in arr {
                    if let Some(s) = v.as_str() {
                        push_if_nonempty(Some(s));
                    } else if let Some(obj) = v.as_object() {
                        // [{path: ...}] / [{file_path: ...}] / [{file: ...}]
                        for inner_key in ["path", "file_path", "filePath", "file"] {
                            push_if_nonempty(obj.get(inner_key).and_then(|v| v.as_str()));
                        }
                    }
                }
            }
        }
        // VS Code apply-patch sometimes uses `input.changes: [{file: ...}]`.
        if let Some(arr) = input.get("changes").and_then(|v| v.as_array()) {
            for v in arr {
                if let Some(obj) = v.as_object() {
                    for inner_key in ["file", "path", "file_path", "filePath"] {
                        push_if_nonempty(obj.get(inner_key).and_then(|v| v.as_str()));
                    }
                }
            }
        }
    }
    // Top-level `path` (legacy).
    push_if_nonempty(tc.get("path").and_then(|v| v.as_str()));

    // Dedupe in-place — many tool shapes set both `path` and `file_path`.
    let mut seen: HashSet<String> = HashSet::new();
    out.retain(|s| seen.insert(s.clone()));
    out
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
        // Registered project `/repo/p1`, tool call against
        // `/repo/p1-fork/foo`: a naive `starts_with` would match.
        // `path_is_under` requires the trailing-slash boundary.
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
    fn extract_paths_handles_multi_file_arrays() {
        // `input.files: [{path: ...}]` (apply-patch shape)
        let v = json!({
            "tool": "ApplyPatch",
            "input": {
                "files": [
                    {"path": "/repo/a.rs"},
                    {"path": "/repo/b.rs"},
                ]
            }
        });
        let paths = extract_paths_from_tool_call(&v);
        assert_eq!(paths, vec!["/repo/a.rs", "/repo/b.rs"]);
    }

    #[test]
    fn extract_paths_handles_filename_array() {
        // `input.fileNames: [...]` shape
        let v = json!({
            "tool": "MultiEdit",
            "input": {"fileNames": ["/repo/a.rs", "/repo/b.rs", "/repo/c.rs"]}
        });
        assert_eq!(
            extract_paths_from_tool_call(&v),
            vec!["/repo/a.rs", "/repo/b.rs", "/repo/c.rs"]
        );
    }

    #[test]
    fn extract_paths_handles_changes_array() {
        // VS Code apply-patch `input.changes: [{file: ...}]` shape
        let v = json!({
            "tool": "ApplyPatch",
            "input": {"changes": [{"file": "/a"}, {"file": "/b"}]}
        });
        assert_eq!(extract_paths_from_tool_call(&v), vec!["/a", "/b"]);
    }

    #[test]
    fn extract_paths_dedupes_redundant_keys() {
        // Some tool schemas set both `path` and `file_path` to the same
        // string. Duplicate output would inflate hit counts.
        let v = json!({
            "input": {"path": "/a", "file_path": "/a"}
        });
        assert_eq!(extract_paths_from_tool_call(&v), vec!["/a"]);
    }

    #[test]
    fn touched_project_ids_uses_all_paths_in_multi_file_calls() {
        // Multi-file tool call touching p1 AND p2 must tag both, not
        // just the first path's project.
        let mut by_id = BTreeMap::new();
        by_id.insert("p1".to_string(), "/repo/p1".to_string());
        by_id.insert("p2".to_string(), "/repo/p2".to_string());
        let calls = vec![json!({
            "tool": "ApplyPatch",
            "input": {"files": [{"path": "/repo/p1/a.rs"}, {"path": "/repo/p2/b.rs"}]}
        })];
        assert_eq!(touched_project_ids(&calls, &by_id, None), vec!["p1", "p2"]);
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
        // Each snapshot must carry head_sha + git_diff + branch so push's
        // compute_incremental_diffs has everything it needs without re-
        // querying anything live.
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

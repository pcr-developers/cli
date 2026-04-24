//! Tool-call utilities (extracting paths, computing touched projects).
//! Direct port of the remainder of `cli/internal/sources/shared/git.go`.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Returns every registered project ID whose path is a parent of any tool
/// call path. Mirrors `shared.TouchedProjectIDs`.
pub fn touched_project_ids(
    tool_calls: &[Value],
    proj_by_id: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    for tc in tool_calls {
        let Some(path) = extract_path_from_tool_call(tc) else {
            continue;
        };
        for (id, proj_path) in proj_by_id {
            if !proj_path.is_empty() && path.starts_with(&format!("{proj_path}/")) {
                seen.insert(id.clone());
            }
        }
    }
    let mut v: Vec<String> = seen.into_iter().collect();
    v.sort();
    v
}

/// Returns git snapshots for each non-primary repo referenced by tool-call
/// paths. Mirrors `shared.RepoSnapshots`.
pub fn repo_snapshots(
    tool_calls: &[Value],
    primary_project_id: &str,
    proj_by_id: &BTreeMap<String, String>,
) -> Option<serde_json::Map<String, Value>> {
    use super::git::{get_git_diff, get_head_sha};
    let mut result = serde_json::Map::new();
    for tc in tool_calls {
        let Some(path) = extract_path_from_tool_call(tc) else {
            continue;
        };
        for (id, proj_path) in proj_by_id {
            if id == primary_project_id || proj_path.is_empty() {
                continue;
            }
            if path.starts_with(&format!("{proj_path}/")) && !result.contains_key(id) {
                let mut snap = serde_json::Map::new();
                snap.insert("head_sha".into(), Value::String(get_head_sha(proj_path)));
                snap.insert("git_diff".into(), Value::String(get_git_diff(proj_path)));
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

/// Files written by tool calls (`write_file`, `edit_file`, `Write`, etc.).
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

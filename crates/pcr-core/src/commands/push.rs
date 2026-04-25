//! `pcr push`. Mirrors `cli/cmd/push.go` — auto-seals open bundles and
//! pushes every sealed bundle to Supabase via `upsert_bundle` /
//! `upsert_bundle_prompts`.

use std::collections::BTreeMap;

use crate::agent::OutputMode;
use crate::auth;
use crate::config;
use crate::display;
use crate::exit::ExitCode;
use crate::projects;
use crate::sources::shared::git;
use crate::sources::shared::path_norm::{
    canonicalize_project_path, normalize_path, proj_id_to_canonical_paths, strip_project_prefix,
};
use crate::store::{self, DraftRecord, PromptCommit};
use crate::supabase::{self, BundleData, TouchedProject};
use crate::util::text::plural;

pub fn run(_mode: OutputMode) -> ExitCode {
    let Some(a) = auth::load() else {
        display::eprintln("PCR: Not logged in.");
        display::print_hint("run `pcr login` — opens your browser to grab a CLI token");
        return ExitCode::AuthRequired;
    };

    let all_unpushed = match store::get_unpushed_commits() {
        Ok(v) => v,
        Err(e) => {
            display::print_error("push", &e.to_string());
            return ExitCode::GenericError;
        }
    };
    if all_unpushed.is_empty() {
        display::eprintln("PCR: No prompt bundles to push.");
        display::print_hint("create one first: `pcr bundle \"name\" --select all`");
        display::print_hint("or run `pcr status` to see where your drafts are in the pipeline");
        return ExitCode::Success;
    }

    let mut commits = Vec::<PromptCommit>::new();
    for mut c in all_unpushed {
        if c.bundle_status == "open" {
            if let Err(e) = store::close_bundle(&c.id) {
                display::print_error("push", &e.to_string());
                return ExitCode::GenericError;
            }
            c.bundle_status = "closed".into();
            display::eprintln(&format!("PCR: Sealed {:?}", c.message));
        }
        commits.push(c);
    }

    let mut pushed = 0usize;
    // BR-2: don't query the *current* branch — that's where the user
    // happens to be when running `pcr push`, not where the prompts were
    // captured. We pass it as a last-ditch fallback only.
    let cwd_branch_fallback = git::git_output(&["rev-parse", "--abbrev-ref", "HEAD"]);
    for commit in &commits {
        pushed += push_bundle(&commit.id, &cwd_branch_fallback, &a.user_id);
    }
    if pushed == 0 {
        display::eprintln("PCR: Nothing new pushed.");
    }
    ExitCode::Success
}

fn push_bundle(local_id: &str, cwd_branch_fallback: &str, user_id: &str) -> usize {
    let Some(c) = store::get_commit_with_items(local_id).ok().flatten() else {
        return 0;
    };

    let source = dominant_source(&c.items);
    let touched = collect_touched_projects(&c.items, cwd_branch_fallback);

    let remote_id = match supabase::upsert_bundle(
        "",
        &BundleData {
            bundle_id: c.id.clone(),
            message: c.message.clone(),
            source,
            project_name: c.project_name.clone(),
            session_shas: c.session_shas.clone(),
            head_sha: c.head_sha.clone(),
            exchange_count: c.items.len() as i64,
            committed_at: c.committed_at.clone(),
            touched_projects: touched,
        },
        user_id,
    ) {
        Ok(r) => r,
        Err(e) => {
            display::eprintln(&format!(
                "PCR: Failed to push prompt bundle {:?}: {e}",
                c.message
            ));
            return 0;
        }
    };

    let (prompt_records, diff_records) = build_payloads(&c);
    if let Err(e) = supabase::upsert_bundle_prompts("", &prompt_records, &diff_records, user_id) {
        display::eprintln(&format!(
            "PCR: Warning — prompt bundle pushed but prompts failed: {e}"
        ));
    }

    let remote_id = if remote_id.is_empty() {
        c.id.clone()
    } else {
        remote_id
    };
    if let Err(e) = store::mark_pushed(&c.id, &remote_id) {
        display::eprintln(&format!(
            "PCR: Warning — pushed but failed to mark locally: {e}"
        ));
    }

    let review_url = format!("{}/review/{}", config::APP_URL, remote_id);
    // BR-2: prefer the captured branch (most-common across the bundle's
    // drafts, then the bundle's own branch_name) over wherever the user
    // happens to be when running `pcr push`. Fall back to the cwd branch
    // only when nothing was captured at all (rare — an empty bundle).
    let branch = best_captured_branch(&c).unwrap_or_else(|| cwd_branch_fallback.to_string());
    display::eprintln(&format!(
        "PCR: Pushed {:?} ({} prompt{})",
        c.message,
        c.items.len(),
        plural(c.items.len())
    ));
    if !branch.is_empty() {
        display::eprintln(&format!("    Branch:  {branch}"));
    }
    display::eprintln(&format!("    Review:  {review_url}"));
    if let Some(pr_url) = detect_github_pr() {
        display::eprintln(&format!("    PR:      {pr_url}"));
    }
    1
}

/// Pick the bundle's branch from what the watchers actually captured.
///
/// Order of precedence:
/// 1. The most-common `branch_name` across the bundle's drafts. Real
///    capture-time branches survive branch-switching mid-session.
/// 2. The PromptCommit's own `branch_name`, set at bundle-creation time.
/// 3. None — caller falls back to the cwd branch as a last resort.
fn best_captured_branch(c: &PromptCommit) -> Option<String> {
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for item in &c.items {
        if item.branch_name.is_empty() {
            continue;
        }
        *counts.entry(item.branch_name.clone()).or_insert(0) += 1;
    }
    if let Some(top) = counts.into_iter().max_by_key(|(_, n)| *n).map(|(b, _)| b) {
        return Some(top);
    }
    if !c.branch_name.is_empty() {
        return Some(c.branch_name.clone());
    }
    None
}

/// Extract the per-repo branch a draft captured for a given project_id.
/// Looks at `file_context.repo_snapshots[id].branch` for secondary repos
/// and at `branch_name` for the primary. Returns None if neither is set.
fn captured_branch_for(item: &DraftRecord, project_id: &str) -> Option<String> {
    if item.project_id == project_id && !item.branch_name.is_empty() {
        return Some(item.branch_name.clone());
    }
    let fc = item.file_context.as_ref()?;
    let snaps = fc.get("repo_snapshots")?.as_object()?;
    let snap = snaps.get(project_id)?.as_object()?;
    let b = snap.get("branch")?.as_str()?;
    if b.is_empty() {
        None
    } else {
        Some(b.to_string())
    }
}

fn collect_touched_projects(
    items: &[DraftRecord],
    cwd_branch_fallback: &str,
) -> Vec<TouchedProject> {
    let mut hits: BTreeMap<String, i64> = BTreeMap::new();
    for item in items {
        if !item.project_id.is_empty() {
            *hits.entry(item.project_id.clone()).or_insert(0) += 1;
        }
        for id in item.touched_project_ids() {
            *hits.entry(id).or_insert(0) += 1;
        }
    }
    if hits.is_empty() {
        return Vec::new();
    }
    let mut proj_by_id: BTreeMap<String, String> = BTreeMap::new();
    for p in projects::load() {
        if !p.project_id.is_empty() {
            proj_by_id.insert(p.project_id, p.path);
        }
    }
    let mut sorted: Vec<(String, i64)> = hits.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted
        .into_iter()
        .enumerate()
        .map(|(i, (id, _count))| {
            // BR-2: prefer the branch captured at prompt time (per-repo) over
            // re-querying current. Only fall back to the live working tree
            // when no capture-time data exists for this project — which only
            // happens for legacy drafts written before BR-1 landed.
            let branch = items
                .iter()
                .find_map(|item| captured_branch_for(item, &id))
                .unwrap_or_else(|| {
                    proj_by_id
                        .get(&id)
                        .filter(|path| !path.is_empty())
                        .map(|path| {
                            git::git_output_in(path, &["rev-parse", "--abbrev-ref", "HEAD"])
                        })
                        .filter(|b| !b.is_empty())
                        .unwrap_or_else(|| cwd_branch_fallback.to_string())
                });
            TouchedProject {
                project_id: id,
                branch,
                is_primary: i == 0,
            }
        })
        .collect()
}

fn build_payloads(c: &PromptCommit) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let incremental = compute_incremental_diffs(&c.items);
    let mut prompts = Vec::new();
    let mut diffs = Vec::new();
    for item in &c.items {
        let mut ids = item.touched_project_ids();
        if ids.is_empty() && !item.project_id.is_empty() {
            ids = vec![item.project_id.clone()];
        }
        let mut rec = serde_json::json!({
            "id": item.id,
            "content_hash": item.content_hash,
            "bundle_id": c.id,
            "session_id": item.session_id,
            "prompt_text": item.prompt_text,
            "tool_calls": item.tool_calls,
            "model": item.model,
            "source": item.source,
            "branch_name": item.branch_name,
            "captured_at": item.captured_at,
            "capture_method": item.capture_method,
            "project_ids": ids,
            "permission_mode": item.permission_mode,
        });
        if !item.project_id.is_empty() {
            rec["project_id"] = serde_json::Value::String(item.project_id.clone());
        }
        if !item.response_text.is_empty() {
            rec["response_text"] = serde_json::Value::String(item.response_text.clone());
        }
        if let Some(fc) = &item.file_context {
            if !fc.is_empty() {
                rec["file_context"] = serde_json::Value::Object(fc.clone());
            }
        }
        prompts.push(rec);
        if let Some(diff) = incremental.get(&item.id) {
            if !diff.is_empty() {
                diffs.push(serde_json::json!({ "prompt_id": item.id, "diff": diff }));
            }
        }
    }
    (prompts, diffs)
}

fn dominant_source(items: &[DraftRecord]) -> String {
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for item in items {
        if !item.source.is_empty() {
            *counts.entry(item.source.clone()).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(s, _)| s)
        .unwrap_or_else(|| "unknown".into())
}

fn detect_github_pr() -> Option<String> {
    let out = std::process::Command::new("gh")
        .args(["pr", "view", "--json", "url", "-q", ".url"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

/// Incremental diff computation. Direct port of Go's `computeIncrementalDiffs`
/// in `cli/cmd/push.go` (~150 LOC).
///
/// For each (session, repo) timeline sorted by captured_at:
///   - First prompt: raw gitDiff filtered to tool-call files.
///   - HEAD changed: `git diff <prev>..<curr> -- <tool-call files>`.
///   - Same HEAD: working-tree delta filtered to tool-call files.
///
/// Secondary repos (from `file_context.repo_snapshots`) are appended after
/// the primary-repo diff, giving a complete multi-repo picture per prompt.
fn compute_incremental_diffs(items: &[DraftRecord]) -> BTreeMap<String, String> {
    use std::collections::HashMap;

    // Canonical project path lookup so `git diff <prev>..<curr>` runs
    // against the real on-disk repo even when the registered project path
    // is a symlink (EV-1 in the multi-repo audit).
    let registered = projects::load();
    let canonical_map = proj_id_to_canonical_paths(&registered);
    let mut proj_by_id: HashMap<String, String> = HashMap::new();
    for (id, canon) in canonical_map {
        proj_by_id.insert(id, canon);
    }

    #[derive(Clone)]
    struct RepoPrompt {
        item_id: String,
        captured_at: String,
        head_sha: String,
        git_diff: String,
        tool_files: Vec<String>,
    }

    #[derive(Hash, Eq, PartialEq, Clone)]
    struct RepoKey {
        session_id: String,
        project_id: String,
    }

    let mut timelines: HashMap<RepoKey, Vec<RepoPrompt>> = HashMap::new();
    let mut primary_proj_by_session: HashMap<String, String> = HashMap::new();

    for item in items {
        primary_proj_by_session
            .entry(item.session_id.clone())
            .or_insert_with(|| item.project_id.clone());

        let primary_path = proj_by_id
            .get(&item.project_id)
            .cloned()
            .unwrap_or_default();
        timelines
            .entry(RepoKey {
                session_id: item.session_id.clone(),
                project_id: item.project_id.clone(),
            })
            .or_default()
            .push(RepoPrompt {
                item_id: item.id.clone(),
                captured_at: item.captured_at.clone(),
                head_sha: item.head_sha.clone(),
                git_diff: item.git_diff.clone(),
                tool_files: tc_files_for_project(&item.tool_calls, &primary_path),
            });

        // Secondary repos from file_context.repo_snapshots.
        if let Some(fc) = &item.file_context {
            if let Some(serde_json::Value::Object(snaps)) = fc.get("repo_snapshots") {
                for (repo_id, snap) in snaps {
                    let serde_json::Value::Object(snap_obj) = snap else {
                        continue;
                    };
                    let head_sha = snap_obj
                        .get("head_sha")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let git_diff = snap_obj
                        .get("git_diff")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let secondary_path = proj_by_id.get(repo_id).cloned().unwrap_or_default();
                    timelines
                        .entry(RepoKey {
                            session_id: item.session_id.clone(),
                            project_id: repo_id.clone(),
                        })
                        .or_default()
                        .push(RepoPrompt {
                            item_id: item.id.clone(),
                            captured_at: item.captured_at.clone(),
                            head_sha,
                            git_diff,
                            tool_files: tc_files_for_project(&item.tool_calls, &secondary_path),
                        });
                }
            }
        }
    }

    let mut primary_diffs: HashMap<String, String> = HashMap::new();
    let mut secondary_diffs: HashMap<String, Vec<String>> = HashMap::new();

    for (key, mut timeline) in timelines {
        timeline.sort_by(|a, b| a.captured_at.cmp(&b.captured_at));
        let project_path = proj_by_id.get(&key.project_id).cloned().unwrap_or_default();
        let is_primary = primary_proj_by_session.get(&key.session_id) == Some(&key.project_id);

        for i in 0..timeline.len() {
            let data = timeline[i].clone();
            let mut diff = String::new();
            if i == 0 {
                diff = if !data.tool_files.is_empty() {
                    filter_diff_to_files(&data.git_diff, &data.tool_files)
                } else {
                    data.git_diff.clone()
                };
            } else {
                let prev = &timeline[i - 1];
                if !data.head_sha.is_empty()
                    && !prev.head_sha.is_empty()
                    && data.head_sha != prev.head_sha
                    && !project_path.is_empty()
                {
                    let mut args = vec![
                        "-C".to_string(),
                        project_path.clone(),
                        "diff".to_string(),
                        format!("{}..{}", prev.head_sha, data.head_sha),
                    ];
                    if !data.tool_files.is_empty() {
                        args.push("--".to_string());
                        args.extend(data.tool_files.iter().cloned());
                    }
                    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                    let raw = crate::sources::shared::git::git_output(&arg_refs);
                    if !raw.is_empty() {
                        diff = truncate_diff(&raw);
                    }
                } else {
                    let raw_delta = diff_delta(&prev.git_diff, &data.git_diff);
                    diff = if !data.tool_files.is_empty() {
                        filter_diff_to_files(&raw_delta, &data.tool_files)
                    } else {
                        raw_delta
                    };
                }
            }

            if diff.is_empty() {
                continue;
            }
            if is_primary {
                primary_diffs.insert(data.item_id, diff);
            } else {
                secondary_diffs.entry(data.item_id).or_default().push(diff);
            }
        }
    }

    let mut result: BTreeMap<String, String> = BTreeMap::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (id, d) in primary_diffs {
        seen.insert(id.clone());
        let mut combined = d;
        if let Some(secs) = secondary_diffs.get(&id) {
            for s in secs {
                combined.push_str(s);
            }
        }
        result.insert(id, combined);
    }
    for (id, secs) in secondary_diffs {
        if seen.contains(&id) {
            continue;
        }
        result.insert(id, secs.join(""));
    }
    result
}

/// Per-project relative file list extracted from a draft's tool calls.
/// Both sides go through canonicalization so symlinked workspaces and
/// (rare) relative tool-call paths attribute correctly. Without this,
/// `compute_incremental_diffs` produces empty per-project diffs for
/// users with symlinked repo roots.
fn tc_files_for_project(tool_calls: &[serde_json::Value], project_path: &str) -> Vec<String> {
    if project_path.is_empty() || tool_calls.is_empty() {
        return Vec::new();
    }
    let project_canon = canonicalize_project_path(project_path);
    if project_canon.is_empty() {
        return Vec::new();
    }
    // Push has no notion of the original session cwd, so we pass the
    // project path itself as the resolution base — the same project the
    // tool call presumably ran against. Absolute tool-call paths are
    // unaffected; relative ones get a sensible best-effort resolution.
    let cwd_for_relative: Option<&str> = Some(project_canon.as_str());
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut files: Vec<String> = Vec::new();
    for tc in tool_calls {
        let Some(raw) = tc_path(tc) else { continue };
        let Some(abs) = normalize_path(&raw, cwd_for_relative) else {
            continue;
        };
        let Some(rel) = strip_project_prefix(&abs, &project_canon) else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        let rel = rel.to_string();
        if seen.insert(rel.clone()) {
            files.push(rel);
        }
    }
    files
}

fn tc_path(tc: &serde_json::Value) -> Option<String> {
    if let Some(input) = tc.get("input").and_then(|v| v.as_object()) {
        if let Some(s) = input.get("path").and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
        if let Some(s) = input.get("file_path").and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    if let Some(s) = tc.get("path").and_then(|v| v.as_str()) {
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    None
}

fn filter_diff_to_files(diff: &str, rel_files: &[String]) -> String {
    if diff.is_empty() || rel_files.is_empty() {
        return diff.to_string();
    }
    let file_set: std::collections::HashSet<&str> = rel_files.iter().map(|s| s.as_str()).collect();
    let mut out = String::new();
    for section in split_diff_sections(diff) {
        let header = diff_file_header(section);
        for field in header.split_whitespace() {
            if let Some(rest) = field.strip_prefix("b/") {
                if file_set.contains(rest) {
                    out.push_str(section);
                    break;
                }
            }
        }
    }
    out
}

fn split_diff_sections(diff: &str) -> Vec<&str> {
    if diff.is_empty() {
        return Vec::new();
    }
    let mut starts: Vec<usize> = Vec::new();
    if diff.starts_with("diff --git ") {
        starts.push(0);
    }
    let mut idx = 0usize;
    while let Some(pos) = diff[idx..].find("\ndiff --git ") {
        starts.push(idx + pos + 1);
        idx += pos + 1;
    }
    let mut sections: Vec<&str> = Vec::with_capacity(starts.len());
    for (i, &start) in starts.iter().enumerate() {
        let end = if i + 1 < starts.len() {
            starts[i + 1]
        } else {
            diff.len()
        };
        sections.push(&diff[start..end]);
    }
    sections
}

fn diff_delta(prev_diff: &str, curr_diff: &str) -> String {
    if curr_diff.is_empty() {
        return String::new();
    }
    if prev_diff.is_empty() {
        return curr_diff.to_string();
    }
    let prev_sections = split_diff_by_file(prev_diff);
    let mut out = String::new();
    for section in split_diff_sections(curr_diff) {
        let header = diff_file_header(section);
        match prev_sections.get(header) {
            Some(prev) if *prev == section => {}
            _ => out.push_str(section),
        }
    }
    out
}

fn split_diff_by_file(diff: &str) -> std::collections::HashMap<&str, &str> {
    let mut out = std::collections::HashMap::new();
    for section in split_diff_sections(diff) {
        out.insert(diff_file_header(section), section);
    }
    out
}

fn diff_file_header(section: &str) -> &str {
    match section.find('\n') {
        Some(nl) => &section[..nl],
        None => section,
    }
}

fn truncate_diff(diff: &str) -> String {
    const MAX: usize = 50_000;
    if diff.len() > MAX {
        let mut out = diff[..MAX].to_string();
        out.push_str("\n[truncated]");
        out
    } else {
        diff.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn draft(
        id: &str,
        project_id: &str,
        branch: &str,
        repo_snapshots: Option<serde_json::Value>,
    ) -> DraftRecord {
        let mut fc = serde_json::Map::new();
        if let Some(snaps) = repo_snapshots {
            fc.insert("repo_snapshots".into(), snaps);
        }
        DraftRecord {
            id: id.into(),
            project_id: project_id.into(),
            branch_name: branch.into(),
            file_context: if fc.is_empty() { None } else { Some(fc) },
            ..Default::default()
        }
    }

    fn commit_with(items: Vec<DraftRecord>, fallback_branch: &str) -> PromptCommit {
        PromptCommit {
            id: "bundle-1".into(),
            message: "test".into(),
            project_id: String::new(),
            project_name: String::new(),
            branch_name: fallback_branch.into(),
            session_shas: Vec::new(),
            head_sha: String::new(),
            pushed_at: String::new(),
            committed_at: String::new(),
            bundle_status: "closed".into(),
            items,
            ..Default::default()
        }
    }

    // ── BR-2 regression: branch comes from captured drafts, not cwd ──────

    #[test]
    fn best_captured_branch_picks_most_common_draft_branch() {
        let items = vec![
            draft("d1", "p1", "feature/auth", None),
            draft("d2", "p1", "feature/auth", None),
            draft("d3", "p1", "main", None),
        ];
        // Bundle-level branch is "main" but most drafts were on the
        // feature branch — the most-common-draft-branch wins, which is the
        // honest answer to "what branch were the prompts on".
        let c = commit_with(items, "main");
        assert_eq!(best_captured_branch(&c), Some("feature/auth".into()));
    }

    #[test]
    fn best_captured_branch_falls_back_to_commit_branch() {
        // Drafts have no captured branch (legacy / cursor pre-BR-1).
        let items = vec![draft("d1", "p1", "", None), draft("d2", "p1", "", None)];
        let c = commit_with(items, "feature/x");
        assert_eq!(best_captured_branch(&c), Some("feature/x".into()));
    }

    #[test]
    fn best_captured_branch_returns_none_when_nothing_captured() {
        // Nothing on drafts AND nothing on the bundle row → caller falls
        // back to the cwd branch.
        let c = commit_with(vec![draft("d1", "p1", "", None)], "");
        assert_eq!(best_captured_branch(&c), None);
    }

    // ── captured_branch_for: secondary repo branch comes from snapshot ──

    #[test]
    fn captured_branch_for_uses_branch_name_for_primary() {
        let item = draft("d1", "p-primary", "main", None);
        assert_eq!(captured_branch_for(&item, "p-primary"), Some("main".into()));
    }

    #[test]
    fn captured_branch_for_uses_repo_snapshots_for_secondary() {
        // BR-1: secondary repo's branch was captured at prompt time and
        // stored under file_context.repo_snapshots[id].branch.
        let snaps = json!({
            "p-secondary": {
                "head_sha": "abc",
                "git_diff": "",
                "branch": "feature/secondary",
            }
        });
        let item = draft("d1", "p-primary", "main", Some(snaps));
        assert_eq!(
            captured_branch_for(&item, "p-secondary"),
            Some("feature/secondary".into())
        );
    }

    #[test]
    fn captured_branch_for_returns_none_when_unknown() {
        let item = draft("d1", "p-primary", "main", None);
        assert_eq!(captured_branch_for(&item, "p-other"), None);
    }

    #[test]
    fn captured_branch_for_returns_none_for_empty_branch_in_snapshot() {
        // Defensive: an older draft with `repo_snapshots` (BR-1 not yet
        // shipped) might have a snapshot without `branch`. Don't return ""
        // — let the caller fall through to the live-query fallback.
        let snaps = json!({
            "p-secondary": {
                "head_sha": "abc",
                "git_diff": "",
                "branch": "",
            }
        });
        let item = draft("d1", "p-primary", "main", Some(snaps));
        assert_eq!(captured_branch_for(&item, "p-secondary"), None);
    }
}

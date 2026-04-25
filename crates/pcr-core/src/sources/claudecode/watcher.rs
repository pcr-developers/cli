//! Claude Code JSONL transcript watcher. Direct port of
//! `cli/internal/sources/claudecode/watcher.go`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher as NotifyWatcher};
use serde_json::Value;
use walkdir::WalkDir;

use crate::display;
use crate::projects::{self, Project};
use crate::sources::claudecode::parser::parse_claude_code_session;
use crate::sources::shared::{
    git::{get_branch, get_commits_since, get_git_diff, get_head_sha, is_git_repo},
    path_norm::{canonicalize_project_path, proj_id_to_canonical_paths},
    tool_calls::{repo_snapshots, touched_project_ids},
    Deduplicator, FileState,
};
use crate::store;
use crate::supabase::{self, PromptRecord};
use crate::versions;

const SOURCE_ID: &str = "claude-code";

pub fn run(user_id: &str, dir: &Path) {
    let state = FileState::new(SOURCE_ID);
    let dedup = Deduplicator::new();
    let dir = dir.to_path_buf();

    display::print_watcher_initializing("Claude Code");
    if !dir.exists() {
        display::print_watcher_missing("Claude Code", &dir.display().to_string());
    } else {
        display::print_watcher_ready("Claude Code", &dir.display().to_string());
    }

    // Initial walk: register baselines for every existing .jsonl.
    for entry in WalkDir::new(&dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if entry.file_type().is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            if let Ok(bytes) = std::fs::read(path) {
                let lines = count_non_empty_lines(&bytes);
                state.set(&path.to_string_lossy(), lines);
            }
        }
    }

    // fsnotify watcher.
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let Ok(mut watcher) = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    }) else {
        display::print_error("claude-code", "Failed to create watcher");
        return;
    };
    let _ = watcher.watch(&dir, RecursiveMode::Recursive);

    let timers: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
    let debounce = Duration::from_secs(1);

    // Background debounce pump: every 250ms, check for paths whose timer
    // expired and run processFile.
    let pump_timers = timers.clone();
    let user_id_for_pump = user_id.to_string();
    let state_for_pump = state.clone();
    let dedup_for_pump = dedup.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(250));
        let now = Instant::now();
        let due: Vec<String> = {
            let Ok(mut guard) = pump_timers.lock() else {
                continue;
            };
            let mut done = Vec::new();
            guard.retain(|path, fire_at| {
                if now >= *fire_at {
                    done.push(path.clone());
                    false
                } else {
                    true
                }
            });
            done
        };
        for path in due {
            process_file(
                &path,
                &user_id_for_pump,
                &state_for_pump,
                &dedup_for_pump,
                false,
            );
        }
    });

    loop {
        let Ok(event) = rx.recv() else {
            return;
        };
        let Ok(event) = event else {
            continue;
        };
        match event.kind {
            EventKind::Create(_) => {
                for p in &event.paths {
                    if p.is_dir() {
                        let _ = watcher.watch(p, RecursiveMode::Recursive);
                    }
                }
            }
            _ => {}
        }
        let is_write_or_create = matches!(
            event.kind,
            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Any
        );
        if !is_write_or_create {
            continue;
        }
        for p in &event.paths {
            if p.extension().is_some_and(|e| e == "jsonl") {
                schedule_process(&timers, p.to_string_lossy().into_owned(), debounce);
            }
        }
    }
}

fn schedule_process(
    timers: &Arc<Mutex<HashMap<String, Instant>>>,
    path: String,
    debounce: Duration,
) {
    let Ok(mut guard) = timers.lock() else {
        return;
    };
    guard.insert(path, Instant::now() + debounce);
}

fn count_non_empty_lines(bytes: &[u8]) -> i64 {
    let s = std::str::from_utf8(bytes).unwrap_or("");
    s.trim()
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .count() as i64
}

/// Mirrors `Watcher.processFile` byte-for-byte: extract slug, resolve project,
/// diff the file against the saved line count, parse, attribute, save/enrich
/// each prompt, and print the capture line.
pub fn process_file(
    file_path: &str,
    user_id: &str,
    state: &FileState,
    dedup: &Deduplicator,
    force_full_scan: bool,
) {
    // Extract slug from ~/.claude/projects/<slug>/<session>.jsonl.
    let normalized = file_path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    let Some(projects_idx) = parts.iter().rposition(|p| *p == "projects") else {
        return;
    };
    if projects_idx + 1 >= parts.len() {
        return;
    }
    let project_slug = parts[projects_idx + 1];
    let Some(project) = projects::get_project_for_claude_slug(project_slug) else {
        return;
    };
    let project_name = project.name.clone();

    let Ok(bytes) = std::fs::read(file_path) else {
        return;
    };
    let content = String::from_utf8_lossy(&bytes);

    let line_count = count_non_empty_lines(&bytes);
    let prev_count = state.get(file_path);

    if !force_full_scan && line_count <= prev_count {
        return;
    }
    state.set(file_path, line_count);

    let session = parse_claude_code_session(&content, &project_name, file_path);
    if session.prompts.is_empty() {
        return;
    }

    let schema_v = versions::CAPTURE_SCHEMA_VERSION;
    let mut base_file_context = serde_json::Map::new();
    base_file_context.insert("capture_schema".into(), Value::Number(schema_v.into()));

    // Build per-project git data cache upfront.
    struct GitData {
        commit_shas: Vec<String>,
        git_diff: String,
        head_sha: String,
    }
    let mut git_cache: HashMap<String, GitData> = HashMap::new();
    let registered_projects = projects::load();
    let mut proj_by_id_project: HashMap<String, Project> = HashMap::new();
    for p in &registered_projects {
        if !p.project_id.is_empty() {
            proj_by_id_project.insert(p.project_id.clone(), p.clone());
        }
    }
    // Canonical paths so symlinked / aliased project paths attribute
    // correctly when tool calls use the resolved path.
    let proj_by_id_path: BTreeMap<String, String> =
        proj_id_to_canonical_paths(&registered_projects);
    // Claude Code tool calls are absolute in practice, but we pass a cwd
    // for relative-path resilience: the primary project's path is the
    // session's working directory in nearly every case.
    let session_cwd: Option<String> = (!project.path.is_empty()).then(|| project.path.clone());
    let cwd = session_cwd.as_deref();

    fn ensure_git_data<'a>(
        cache: &'a mut HashMap<String, GitData>,
        path: &str,
        session_created_at: &str,
    ) -> &'a GitData {
        if !cache.contains_key(path) {
            // Use the canonical path for git ops so symlinked workspaces
            // produce diffs against the real on-disk repo.
            let canon = canonicalize_project_path(path);
            let git_diff = get_git_diff(&canon);
            let head_sha = get_head_sha(&canon);
            let commit_shas = if !canon.is_empty() && !session_created_at.is_empty() {
                get_commits_since(&canon, session_created_at)
            } else {
                Vec::new()
            };
            cache.insert(
                path.to_string(),
                GitData {
                    commit_shas,
                    git_diff,
                    head_sha,
                },
            );
        }
        cache.get(path).expect("cache entry was just inserted")
    }

    let mut new_prompts: Vec<PromptRecord> = Vec::new();
    for mut p in session.prompts.clone() {
        let hash = supabase::prompt_content_hash(&p.session_id, &p.prompt_text, "");
        if dedup.is_duplicate(&p.session_id, &hash) {
            // Already processed in this run — enrich only.
            let _ = store::update_draft_response(&p.session_id, &p.prompt_text, &p.response_text);
            let _ = store::update_draft_tool_calls(&p.session_id, &p.prompt_text, &p.tool_calls);
            if let Some(snaps) = repo_snapshots(&p.tool_calls, &p.project_id, &proj_by_id_path, cwd)
            {
                let mut updates = serde_json::Map::new();
                updates.insert("repo_snapshots".into(), Value::Object(snaps));
                let _ = store::merge_draft_file_context(&p.session_id, &p.prompt_text, &updates);
            }
            // Backfill git_diff: use the project's own path.
            let resolved_path = proj_by_id_project
                .get(&p.project_id)
                .map(|pp| pp.path.clone())
                .unwrap_or_else(|| project.path.clone());
            if !resolved_path.is_empty() {
                let gd =
                    ensure_git_data(&mut git_cache, &resolved_path, &session.session_created_at);
                if !gd.git_diff.is_empty() {
                    let _ = store::update_draft_git_diff(
                        &p.session_id,
                        &p.prompt_text,
                        &gd.git_diff,
                        &gd.head_sha,
                    );
                }
            }
            continue;
        }

        if store::is_draft_saved(&p.session_id, &p.prompt_text) {
            dedup.mark(&p.session_id, &hash);
            let _ = store::update_draft_response(&p.session_id, &p.prompt_text, &p.response_text);
            let _ = store::update_draft_tool_calls(&p.session_id, &p.prompt_text, &p.tool_calls);
            let mut fc = serde_json::Map::new();
            if let Some(snaps) = repo_snapshots(&p.tool_calls, &p.project_id, &proj_by_id_path, cwd)
            {
                fc.insert("repo_snapshots".into(), Value::Object(snaps));
            }
            let ids = touched_project_ids(&p.tool_calls, &proj_by_id_path, cwd);
            // Only store touched_project_ids when there's >1. When it's
            // just the primary, project_id already carries that fact;
            // the redundant array bloats the row for no benefit.
            if ids.len() > 1 {
                fc.insert(
                    "touched_project_ids".into(),
                    Value::Array(ids.iter().map(|s| Value::String(s.clone())).collect()),
                );
            }
            if !fc.is_empty() {
                let _ = store::merge_draft_file_context(&p.session_id, &p.prompt_text, &fc);
            }
            let resolved_path = proj_by_id_project
                .get(&p.project_id)
                .map(|pp| pp.path.clone())
                .unwrap_or_else(|| project.path.clone());
            if !resolved_path.is_empty() {
                let gd =
                    ensure_git_data(&mut git_cache, &resolved_path, &session.session_created_at);
                if !gd.git_diff.is_empty() {
                    let _ = store::update_draft_git_diff(
                        &p.session_id,
                        &p.prompt_text,
                        &gd.git_diff,
                        &gd.head_sha,
                    );
                }
            }
            continue;
        }

        let mut merged = base_file_context.clone();
        if let Some(fc) = &p.file_context {
            for (k, v) in fc {
                merged.insert(k.clone(), v.clone());
            }
        }
        p.user_id = user_id.to_string();
        p.project_id = project.project_id.clone();
        p.project_name = project.name.clone();

        let ids = touched_project_ids(&p.tool_calls, &proj_by_id_path, cwd);
        // Multi-touched only — see comment above on storage gating.
        if ids.len() > 1 {
            merged.insert(
                "touched_project_ids".into(),
                Value::Array(ids.iter().map(|s| Value::String(s.clone())).collect()),
            );
        }
        p.file_context = Some(merged);
        new_prompts.push(p);
    }

    if new_prompts.is_empty() {
        return;
    }

    // Save each new prompt with full git metadata.
    let mut seen_hashes: HashSet<String> = HashSet::new();
    for p in new_prompts.iter_mut() {
        let resolved_path = if !p.project_id.is_empty() {
            proj_by_id_project
                .get(&p.project_id)
                .map(|pp| pp.path.clone())
                .unwrap_or_else(|| project.path.clone())
        } else {
            project.path.clone()
        };
        let gd = ensure_git_data(&mut git_cache, &resolved_path, &session.session_created_at);

        if let Some(snaps) = repo_snapshots(&p.tool_calls, &p.project_id, &proj_by_id_path, cwd) {
            let fc = p.file_context.get_or_insert_with(serde_json::Map::new);
            fc.insert("repo_snapshots".into(), Value::Object(snaps));
        }
        // Tag drafts captured against directories that aren't git repos
        // so reviewers see the empty diff is by-design (no git available)
        // rather than by-failure (git ran and found no changes).
        if !resolved_path.is_empty() && !is_git_repo(&resolved_path) {
            let fc = p.file_context.get_or_insert_with(serde_json::Map::new);
            fc.insert("git_unavailable".into(), Value::Bool(true));
        }
        // Re-read branch at save time so prompts in long sessions that
        // crossed a `git switch` get attributed to the branch they
        // actually landed on. The session-level branch (cwd at session
        // start) is still on the bundle row, but each prompt records
        // its own branch_name.
        if !resolved_path.is_empty() {
            let fresh_branch = get_branch(&resolved_path);
            if !fresh_branch.is_empty() {
                p.branch_name = fresh_branch;
            }
        }
        if let Err(e) = store::save_draft(p, &gd.commit_shas, &gd.git_diff, &gd.head_sha) {
            display::print_error("claude-code", &format!("Failed to save draft: {e}"));
            continue;
        }
        let hash = supabase::prompt_content_hash_v2(&p.session_id, &p.prompt_text, &p.captured_at);
        dedup.mark(&p.session_id, &hash);
        seen_hashes.insert(hash);
    }

    if new_prompts.is_empty() {
        return;
    }
    let last = new_prompts.last().cloned().unwrap_or_default();
    if user_id.is_empty() {
        display::print_drafted(&display::DraftDisplayOptions {
            project_name: &project_name,
            branch: &session.branch,
            prompt_text: &last.prompt_text,
            exchange_count: new_prompts.len() as u64,
        });
    } else {
        display::print_captured(&display::CaptureDisplayOptions {
            project_name: &project_name,
            branch: &session.branch,
            prompt_text: &last.prompt_text,
            tool_calls: &last.tool_calls,
            exchange_count: new_prompts.len() as u64,
            ..Default::default()
        });
    }
}

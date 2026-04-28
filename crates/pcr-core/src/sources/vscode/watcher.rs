//! VS Code Copilot Chat watcher. Direct port of
//! `cli/internal/sources/vscode/watcher.go`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher as NotifyWatcher};
use serde_json::Value;

use crate::display;
use crate::projects::{self, Project};
use crate::sources::shared::{
    git::{get_branch, get_commits_since, get_git_diff, get_head_sha, is_git_repo},
    path_norm::{canonicalize_project_path, proj_id_to_canonical_paths},
    tool_calls::{repo_snapshots, touched_project_ids},
    Deduplicator, FileState,
};
use crate::sources::vscode::chatsession_parser::parse_chatsession;
use crate::sources::vscode::parser::{
    exchange_to_prompt_record, parse_transcript, ParsedExchange, ParsedTranscript,
};
use crate::sources::vscode::workspace::{scan_workspaces, workspace_storage_bases, WorkspaceMatch};
use crate::store::{self, is_draft_saved_at};
use crate::supabase;

pub fn run(user_id: &str, _dir: &Path) {
    let workspaces = scan_workspaces();
    if workspaces.is_empty() {
        display::print_error(
            "vscode",
            "No VS Code workspaces match registered projects. Will activate when new workspaces appear.",
        );
    }

    let state = FileState::new("vscode");
    let dedup = Deduplicator::new();
    let workspaces_arc = Arc::new(Mutex::new(workspaces));
    let self_session_id = detect_self_session_id();

    // Empty-window session processor — fires in the background every so
    // often (cheap O(N) file read per pass).
    let user_id_empty = user_id.to_string();
    thread::spawn(move || {
        let state = FileState::new("vscode-empty");
        let dedup = Deduplicator::new();
        loop {
            super::empty_window::process_empty_window_sessions(&user_id_empty, &state, &dedup);
            thread::sleep(Duration::from_secs(20));
        }
    });

    // fsnotify watcher over every transcript dir + every workspaceStorage base.
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let Ok(mut watcher) = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    }) else {
        display::print_error("vscode", "Failed to create watcher");
        return;
    };

    {
        let Ok(workspaces) = workspaces_arc.lock() else {
            return;
        };
        for ws in workspaces.iter() {
            watch_transcript_dir(&mut watcher, &ws.transcript_dir, &state);
            watch_transcript_dir(&mut watcher, &ws.chat_sessions_dir, &state);
        }
    }
    let mut parent_dirs: HashSet<PathBuf> = HashSet::new();
    for base in workspace_storage_bases() {
        if parent_dirs.insert(base.clone()) {
            let _ = watcher.watch(&base, RecursiveMode::NonRecursive);
        }
    }

    let timers: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
    let debounce = Duration::from_secs(1);

    let pump_timers = timers.clone();
    let pump_state = state.clone();
    let pump_dedup = dedup.clone();
    let pump_ws = workspaces_arc.clone();
    let pump_self_session = self_session_id.clone();
    let pump_user_id = user_id.to_string();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(250));
        let now = Instant::now();
        let due: Vec<String> = {
            let Ok(mut guard) = pump_timers.lock() else {
                continue;
            };
            let mut out = Vec::new();
            guard.retain(|path, fire_at| {
                if now >= *fire_at {
                    out.push(path.clone());
                    false
                } else {
                    true
                }
            });
            out
        };
        for path in due {
            let Ok(ws_snapshot) = pump_ws.lock() else {
                continue;
            };
            let snapshot: Vec<WorkspaceMatch> = ws_snapshot.clone();
            drop(ws_snapshot);
            process_file(
                &path,
                &pump_user_id,
                &pump_state,
                &pump_dedup,
                &snapshot,
                &pump_self_session,
            );
        }
    });

    // Periodic full rescan: catches the cases the create-event path
    // misses \u2014 user re-registered a project, re-cloned a repo to a new
    // path, or VS Code created the chatSessions/transcripts subdir
    // before our parent watch existed. Cheap (one stat per workspace
    // hash) so we tick every 10 s.
    let mut last_rescan = Instant::now();
    let rescan_interval = Duration::from_secs(10);

    loop {
        let event = match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(ev)) => Some(ev),
            Ok(Err(_)) => None,
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };

        if last_rescan.elapsed() >= rescan_interval {
            rescan_workspaces(&mut watcher, &workspaces_arc, &state);
            last_rescan = Instant::now();
        }

        let Some(event) = event else {
            continue;
        };
        if matches!(event.kind, EventKind::Create(_)) {
            for p in &event.paths {
                if p.is_dir() {
                    let _ = watcher.watch(p, RecursiveMode::Recursive);
                    handle_new_dir(&mut watcher, p, &workspaces_arc, &state);
                }
            }
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

/// Re-scan the projects.json + workspaceStorage tree and watch any
/// newly-matched workspace. Existing watches are kept (notify de-dups
/// duplicate `watch` calls). Doesn't unwatch removed workspaces \u2014 a
/// disappeared transcript dir simply emits no more events.
fn rescan_workspaces(
    watcher: &mut notify::RecommendedWatcher,
    workspaces_arc: &Arc<Mutex<Vec<WorkspaceMatch>>>,
    state: &FileState,
) {
    let new_matches = scan_workspaces();
    let Ok(mut workspaces) = workspaces_arc.lock() else {
        return;
    };
    for nm in new_matches {
        if workspaces.iter().any(|w| w.hash == nm.hash) {
            // Workspace already known, but its chatSessions/transcripts
            // dir may have appeared since the last scan \u2014 re-watch is
            // idempotent so just call it again.
            watch_transcript_dir(watcher, &nm.transcript_dir, state);
            watch_transcript_dir(watcher, &nm.chat_sessions_dir, state);
            continue;
        }
        let td = nm.transcript_dir.clone();
        let cs = nm.chat_sessions_dir.clone();
        workspaces.push(nm);
        watch_transcript_dir(watcher, &td, state);
        watch_transcript_dir(watcher, &cs, state);
    }
}

fn schedule_process(
    timers: &Arc<Mutex<HashMap<String, Instant>>>,
    path: String,
    debounce: Duration,
) {
    if let Ok(mut guard) = timers.lock() {
        guard.insert(path, Instant::now() + debounce);
    }
}

fn detect_self_session_id() -> String {
    let Ok(log_path) = std::env::var("VSCODE_TARGET_SESSION_LOG") else {
        return String::new();
    };
    let parent = Path::new(&log_path).parent().map(|p| p.to_path_buf());
    parent
        .and_then(|p| {
            p.file_name()
                .and_then(|n| n.to_str().map(|s| s.to_string()))
        })
        .unwrap_or_default()
}

fn watch_transcript_dir(watcher: &mut notify::RecommendedWatcher, dir: &Path, state: &FileState) {
    if !dir.exists() {
        let parent = dir.parent();
        if let Some(p) = parent {
            if p.exists() {
                let _ = watcher.watch(p, RecursiveMode::NonRecursive);
            }
        }
        return;
    }
    let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
    display::print_watcher_ready("VS Code", &dir.display().to_string());

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                continue;
            }
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "jsonl") {
                if let Ok(bytes) = std::fs::read(&path) {
                    let lines = count_non_empty_lines(&bytes);
                    state.set(&path.to_string_lossy(), lines);
                }
            }
        }
    }
}

fn handle_new_dir(
    watcher: &mut notify::RecommendedWatcher,
    dir_path: &Path,
    workspaces_arc: &Arc<Mutex<Vec<WorkspaceMatch>>>,
    state: &FileState,
) {
    let Some(name) = dir_path.file_name().and_then(|s| s.to_str()) else {
        return;
    };
    if name == "transcripts" {
        let _ = watcher.watch(dir_path, RecursiveMode::NonRecursive);
        return;
    }
    let ws_file = dir_path.join("workspace.json");
    if !ws_file.exists() {
        return;
    }
    let new_matches = scan_workspaces();
    let Ok(mut workspaces) = workspaces_arc.lock() else {
        return;
    };
    for nm in new_matches {
        let found = workspaces.iter().any(|w| w.hash == nm.hash);
        if !found {
            let td = nm.transcript_dir.clone();
            let cs = nm.chat_sessions_dir.clone();
            workspaces.push(nm);
            watch_transcript_dir(watcher, &td, state);
            watch_transcript_dir(watcher, &cs, state);
        }
    }
}

fn count_non_empty_lines(bytes: &[u8]) -> i64 {
    let s = std::str::from_utf8(bytes).unwrap_or("");
    s.trim()
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .count() as i64
}

fn process_file(
    file_path: &str,
    user_id: &str,
    state: &FileState,
    dedup: &Deduplicator,
    workspaces: &[WorkspaceMatch],
    self_session_id: &str,
) {
    let Some(ws) = find_workspace(file_path, workspaces) else {
        return;
    };
    let Ok(bytes) = std::fs::read(file_path) else {
        return;
    };
    let content = String::from_utf8_lossy(&bytes);

    let lines = count_non_empty_lines(&bytes);
    let prev = state.get(file_path);
    let is_chat_sessions = file_path.contains("chatSessions");
    // Legacy transcripts are append-only — short-circuit when no new
    // lines have arrived. The new chatSessions format rewrites the
    // file in place (kind=0 snapshot can collapse many ops), so the
    // line-count guard would skip valid updates; always re-parse it.
    if !is_chat_sessions && lines <= prev {
        return;
    }
    state.set(file_path, lines);

    // Dispatch to the matching parser by source layout. Both produce
    // the same `ParsedTranscript` shape so the rest of process_file
    // works unchanged.
    let transcript: ParsedTranscript = if is_chat_sessions {
        parse_chatsession(&content)
    } else {
        parse_transcript(&content)
    };

    if !self_session_id.is_empty() && transcript.session_id == self_session_id {
        return;
    }
    if transcript.exchanges.is_empty() {
        return;
    }

    // Canonical project path map for symlink-resilient attribution.
    // `proj_id_to_canonical_paths` skips entries without a project_id
    // or a path.
    let registered_projects = projects::load();
    let proj_by_id: BTreeMap<String, String> = proj_id_to_canonical_paths(&registered_projects);
    let mut proj_by_path: HashMap<String, usize> = HashMap::new();
    for (i, p) in ws.projects.iter().enumerate() {
        if !p.path.is_empty() {
            proj_by_path.insert(p.path.clone(), i);
        }
    }

    let mut new_count = 0usize;
    let mut last_name = String::new();
    let mut last_prompt = String::new();
    let mut last_tool_calls: Vec<Value> = Vec::new();
    let mut last_branch = String::new();

    for ex in transcript.exchanges.iter() {
        let hash = supabase::prompt_content_hash_v2(
            &transcript.session_id,
            &ex.prompt_text,
            &ex.captured_at,
        );
        if dedup.is_duplicate(&transcript.session_id, &hash) {
            update_existing_draft(&transcript, ex, ws, &proj_by_path, &proj_by_id);
            continue;
        }
        if is_draft_saved_at(&transcript.session_id, &ex.prompt_text, &ex.captured_at) {
            dedup.mark(&transcript.session_id, &hash);
            update_existing_draft(&transcript, ex, ws, &proj_by_path, &proj_by_id);
            continue;
        }

        let primary = project_for_exchange(ex, &ws.projects, &proj_by_path);

        let (proj_name, proj_id, branch, proj_path) = match primary {
            Some(p) => (
                p.name.clone(),
                p.project_id.clone(),
                get_branch(&p.path),
                p.path.clone(),
            ),
            None => (String::new(), String::new(), String::new(), String::new()),
        };

        let mut record =
            exchange_to_prompt_record(ex, &transcript.session_id, &proj_name, &proj_id, &branch);
        record.user_id = user_id.to_string();
        record.id =
            supabase::prompt_id_v2(&transcript.session_id, &ex.prompt_text, &ex.captured_at);
        record.content_hash = hash.clone();

        let fc = record.file_context.get_or_insert_with(serde_json::Map::new);
        if !transcript.copilot_version.is_empty() {
            fc.insert(
                "copilot_version".into(),
                Value::String(transcript.copilot_version.clone()),
            );
        }
        if !transcript.vscode_version.is_empty() {
            fc.insert(
                "vscode_version".into(),
                Value::String(transcript.vscode_version.clone()),
            );
        }

        // Resolve relative tool-call paths against the workspace root.
        let prompt_cwd: Option<&str> = if proj_path.is_empty() {
            None
        } else {
            Some(proj_path.as_str())
        };
        let touched = touched_project_ids(&ex.tool_calls, &proj_by_id, prompt_cwd);
        if touched.len() > 1 {
            fc.insert(
                "touched_project_ids".into(),
                Value::Array(touched.iter().map(|s| Value::String(s.clone())).collect()),
            );
        }
        // Capture per-secondary-repo head_sha + git_diff + branch so the
        // push pipeline can emit complete multi-repo diffs in review.
        // Without these snapshots a multi-repo VS Code session shows
        // only the primary repo's diff.
        if let Some(snaps) = repo_snapshots(&ex.tool_calls, &proj_id, &proj_by_id, prompt_cwd) {
            fc.insert("repo_snapshots".into(), Value::Object(snaps));
        }

        // Use the canonical primary path for git ops so symlinked workspaces
        // produce diffs against the real on-disk repo.
        let canon_proj_path = canonicalize_project_path(&proj_path);
        let mut git_diff = String::new();
        let mut head_sha = String::new();
        let mut commit_shas: Vec<String> = Vec::new();
        if !canon_proj_path.is_empty() {
            git_diff = get_git_diff(&canon_proj_path);
            head_sha = get_head_sha(&canon_proj_path);
            if !transcript.start_time.is_empty() {
                commit_shas = get_commits_since(&canon_proj_path, &transcript.start_time);
            }
            // Tag drafts captured against directories that aren't a git
            // repo so the empty diff in review reads as "no git data
            // available" rather than "no changes".
            if !is_git_repo(&canon_proj_path) {
                fc.insert("git_unavailable".into(), Value::Bool(true));
            }
        }
        // VS Code already captures `branch` per-prompt above; no
        // re-read needed here.

        if let Err(e) = store::save_draft(&record, &commit_shas, &git_diff, &head_sha) {
            display::print_error("vscode", &format!("Failed to save draft: {e}"));
            continue;
        }
        dedup.mark(&transcript.session_id, &hash);
        new_count += 1;
        last_name = proj_name;
        last_prompt = ex.prompt_text.clone();
        last_tool_calls = ex.tool_calls.clone();
        last_branch = branch;
    }

    if new_count == 0 {
        return;
    }

    if user_id.is_empty() {
        display::print_drafted(&display::DraftDisplayOptions {
            project_name: &last_name,
            branch: &last_branch,
            prompt_text: &last_prompt,
            exchange_count: new_count as u64,
        });
    } else {
        display::print_captured(&display::CaptureDisplayOptions {
            project_name: &last_name,
            branch: &last_branch,
            prompt_text: &last_prompt,
            tool_calls: &last_tool_calls,
            exchange_count: new_count as u64,
            ..Default::default()
        });
    }
}

fn update_existing_draft(
    transcript: &ParsedTranscript,
    ex: &ParsedExchange,
    ws: &WorkspaceMatch,
    proj_by_path: &HashMap<String, usize>,
    proj_by_id: &BTreeMap<String, String>,
) {
    let _ =
        store::update_draft_response(&transcript.session_id, &ex.prompt_text, &ex.response_text);
    let _ = store::update_draft_tool_calls(&transcript.session_id, &ex.prompt_text, &ex.tool_calls);
    let mut updates = serde_json::Map::new();
    if ex.duration_ms > 0 {
        updates.insert(
            "response_duration_ms".into(),
            Value::Number(ex.duration_ms.into()),
        );
    }
    if ex.first_response_ms > 0 {
        updates.insert(
            "first_response_ms".into(),
            Value::Number(ex.first_response_ms.into()),
        );
    }
    if !ex.changed_files.is_empty() {
        updates.insert(
            "changed_files".into(),
            Value::Array(
                ex.changed_files
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if !ex.relevant_files.is_empty() {
        updates.insert(
            "relevant_files".into(),
            Value::Array(
                ex.relevant_files
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if !ex.reasoning_text.is_empty() {
        updates.insert(
            "reasoning_text".into(),
            Value::String(ex.reasoning_text.clone()),
        );
    }
    if !ex.tool_calls.is_empty() {
        updates.insert("is_agentic".into(), Value::Bool(true));
    }
    let primary = project_for_exchange(ex, &ws.projects, proj_by_path);
    let prompt_cwd: Option<&str> = primary.map(|p| p.path.as_str()).filter(|s| !s.is_empty());
    let touched = touched_project_ids(&ex.tool_calls, proj_by_id, prompt_cwd);
    if touched.len() > 1 {
        updates.insert(
            "touched_project_ids".into(),
            Value::Array(touched.iter().map(|s| Value::String(s.clone())).collect()),
        );
    }
    // Re-emit `repo_snapshots` on enrichment so a session that gained a
    // secondary-repo touch on a later prompt still ends up with the
    // secondary diff in review.
    let primary_id = primary.map(|p| p.project_id.as_str()).unwrap_or("");
    if let Some(snaps) = repo_snapshots(&ex.tool_calls, primary_id, proj_by_id, prompt_cwd) {
        updates.insert("repo_snapshots".into(), Value::Object(snaps));
    }
    let _ = store::merge_draft_file_context(&transcript.session_id, &ex.prompt_text, &updates);

    if let Some(primary) = primary {
        if !primary.path.is_empty() {
            let canon = canonicalize_project_path(&primary.path);
            let git_diff = get_git_diff(&canon);
            let head_sha = get_head_sha(&canon);
            let _ = store::update_draft_git_diff(
                &transcript.session_id,
                &ex.prompt_text,
                &git_diff,
                &head_sha,
            );
        }
    }
}

fn find_workspace<'a>(
    file_path: &str,
    workspaces: &'a [WorkspaceMatch],
) -> Option<&'a WorkspaceMatch> {
    for ws in workspaces {
        // The hash_dir is the common ancestor of both `transcripts/` and
        // `chatSessions/` — match against it so either layout resolves to
        // the same workspace entry.
        let hash_dir = ws.transcript_dir.parent().and_then(|p| p.parent());
        if let Some(hash_dir) = hash_dir {
            if file_path.starts_with(hash_dir.to_string_lossy().as_ref()) {
                return Some(ws);
            }
        }
    }
    None
}

fn project_for_exchange<'a>(
    ex: &ParsedExchange,
    ws_projects: &'a [Project],
    proj_by_path: &HashMap<String, usize>,
) -> Option<&'a Project> {
    let mut hits: HashMap<usize, usize> = HashMap::new();
    let mut all_files = ex.changed_files.clone();
    all_files.extend(ex.relevant_files.iter().cloned());
    for f in &all_files {
        for p in ws_projects {
            if p.path.is_empty() {
                continue;
            }
            if f.starts_with(&format!("{}/", p.path)) {
                if let Some(idx) = proj_by_path.get(&p.path) {
                    *hits.entry(*idx).or_insert(0) += 1;
                }
                break;
            }
        }
    }
    for tc in &ex.tool_calls {
        let Some(path) = extract_tool_call_path(tc) else {
            continue;
        };
        for p in ws_projects {
            if p.path.is_empty() {
                continue;
            }
            if path.starts_with(&format!("{}/", p.path)) {
                if let Some(idx) = proj_by_path.get(&p.path) {
                    *hits.entry(*idx).or_insert(0) += 1;
                }
                break;
            }
        }
    }
    if !hits.is_empty() {
        let mut best_idx: Option<usize> = None;
        let mut best_count = 0usize;
        for (idx, count) in hits {
            if count > best_count {
                best_count = count;
                best_idx = Some(idx);
            }
        }
        if let Some(p) = best_idx.and_then(|i| ws_projects.get(i)) {
            return Some(p);
        }
    }
    // No file references in this exchange (a pure Q&A turn with no
    // tool calls, edits, or `relevant_files`). Fall back to the
    // workspace's primary registered project so the prompt is still
    // attributed correctly. Without this fallback every conversational
    // prompt lands in the store with `project_id = NULL`, which makes
    // them invisible on the dashboard once pushed.
    ws_projects.iter().find(|p| !p.path.is_empty())
}

fn extract_tool_call_path(tc: &Value) -> Option<String> {
    let input = tc.get("input").and_then(|v| v.as_object())?;
    for key in ["filePath", "file_path", "path"] {
        if let Some(s) = input.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

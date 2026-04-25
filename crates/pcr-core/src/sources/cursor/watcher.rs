//! Cursor agent-transcript watcher (the PromptScanner). Direct port of
//! `cli/internal/sources/cursor/watcher.go`.

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use notify::{EventKind, RecursiveMode, Watcher as NotifyWatcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use walkdir::WalkDir;

use crate::display;
use crate::projects::{self, Project};
use crate::sources::cursor::db::{
    get_full_session_data, get_session_meta, BubbleMeta, SessionMeta,
};
use crate::sources::cursor::diff_tracker::DiffTracker;
use crate::sources::shared::git::{get_commit_range, get_git_diff};
use crate::store::{self, DiffEvent};
use crate::supabase::{self, CursorSessionData, PromptRecord};
use crate::versions;

pub struct PromptScanner {
    dir: PathBuf,
    user_id: String,
    diff_tracker: Option<Arc<DiffTracker>>,
    seen: Arc<Mutex<HashSet<String>>>,
    initial_scan: Arc<Mutex<bool>>,
}

impl PromptScanner {
    pub fn new(dir: PathBuf, user_id: String, diff_tracker: Option<Arc<DiffTracker>>) -> Self {
        Self {
            dir,
            user_id,
            diff_tracker,
            seen: Arc::new(Mutex::new(HashSet::new())),
            initial_scan: Arc::new(Mutex::new(true)),
        }
    }

    pub fn start(self: Arc<Self>) {
        display::print_watcher_initializing("Cursor");
        if !self.dir.exists() {
            display::print_watcher_missing("Cursor", &self.dir.display().to_string());
        } else {
            display::print_watcher_ready("Cursor", &self.dir.display().to_string());
        }
        // Initial silent scan.
        self.scan();
        if let Ok(mut flag) = self.initial_scan.lock() {
            *flag = false;
        }
        // Kick off fsnotify fast-path in a thread.
        let s2 = self.clone();
        thread::spawn(move || s2.watch_fsnotify());
        // Periodic 20-second scan.
        loop {
            thread::sleep(Duration::from_secs(20));
            self.scan();
        }
    }

    fn scan(&self) {
        if let Some(dt) = &self.diff_tracker {
            dt.poll();
        }
        for entry in WalkDir::new(&self.dir).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !is_agent_transcript(path) {
                continue;
            }
            let Some((project_slug, session_id)) = parse_transcript_path(path) else {
                continue;
            };
            self.process_session(&project_slug, &session_id);
        }
    }

    fn process_session(&self, project_slug: &str, session_id: &str) {
        let candidates = projects::get_all_projects_for_cursor_slug(project_slug);
        if candidates.is_empty() {
            return;
        }
        if let Some(dt) = &self.diff_tracker {
            for c in &candidates {
                dt.register_project(&c.project_id);
            }
        }
        let Some(meta) = get_session_meta(session_id) else {
            return;
        };
        if meta.bubbles.is_empty() {
            return;
        }

        for i in 0..meta.bubbles.len() {
            let b = &meta.bubbles[i];
            if b.ty != 1 || b.text.trim().is_empty() {
                continue;
            }
            let key = format!("{session_id}:{}", b.bubble_id);
            {
                let Ok(mut seen) = self.seen.lock() else {
                    continue;
                };
                if seen.contains(&key) {
                    continue;
                }
                // Atomically mark as seen so concurrent scans don't double-save.
                seen.insert(key.clone());
            }

            // Find the last assistant bubble of this turn and whether it's complete.
            let mut last_assistant: Option<BubbleMeta> = None;
            let mut response_text = String::new();
            for j in (i + 1)..meta.bubbles.len() {
                let nb = &meta.bubbles[j];
                if nb.ty == 1 {
                    break;
                }
                if nb.ty == 2 {
                    last_assistant = Some(nb.clone());
                    if response_text.is_empty() && !nb.text.trim().is_empty() {
                        response_text = nb.text.clone();
                    }
                }
            }
            let Some(last) = last_assistant else {
                // Un-mark so we try again next scan once the turn completes.
                if let Ok(mut seen) = self.seen.lock() {
                    seen.remove(&key);
                }
                continue;
            };
            if last.turn_duration_ms.is_none() {
                if let Ok(mut seen) = self.seen.lock() {
                    seen.remove(&key);
                }
                continue;
            }

            if store::is_draft_saved_by_bubble(session_id, &b.bubble_id)
                || store::is_draft_saved(session_id, &b.text)
            {
                continue;
            }

            let initial = self.initial_scan.lock().map(|g| *g).unwrap_or(true);
            if !initial {
                let dur_sec = last.turn_duration_ms.unwrap_or(0) / 1000;
                let short_sid: String = session_id.chars().take(8).collect();
                display::print_verbose_event(
                    "scan",
                    &format!(
                        "[{short_sid}]  turn complete  {dur_sec}s  {:?}",
                        truncate(&b.text, 50)
                    ),
                );
            }

            self.save_completed_turn(
                session_id,
                &meta.composer_id,
                b,
                &last,
                &response_text,
                &meta,
                &candidates,
                !initial,
            );
        }
    }

    fn save_completed_turn(
        &self,
        session_id: &str,
        _composer_id: &str,
        user_bubble: &BubbleMeta,
        last_assistant: &BubbleMeta,
        response_text: &str,
        meta: &SessionMeta,
        candidates: &[Project],
        show_output: bool,
    ) {
        let captured_at = if user_bubble.created_at.is_empty() {
            crate::util::time::now_rfc3339()
        } else {
            user_bubble.created_at.clone()
        };
        let turn_start = parse_bubble_time(&captured_at);

        let mut turn_end: DateTime<Utc> = Utc::now();
        if !last_assistant.created_at.is_empty() {
            if let Some(assistant_start) = parse_bubble_time(&last_assistant.created_at) {
                if let Some(ms) = last_assistant.turn_duration_ms {
                    turn_end = assistant_start + chrono::Duration::milliseconds(ms);
                }
            }
        }

        // Mode + model from state timeline.
        let mut mode = String::new();
        let mut model_name = meta.model_name.clone();
        if let Some(start) = turn_start {
            if let Ok(Some(state_event)) = store::get_session_state_at(session_id, start) {
                mode = state_event.unified_mode.clone();
                if !state_event.model_name.is_empty() {
                    model_name = state_event.model_name.clone();
                }
            }
        }
        if mode.is_empty() {
            mode = meta.unified_mode.clone();
        }

        let is_agent_turn = mode == "agent" || mode == "debug";
        let mut changed_files: Vec<String> = Vec::new();
        let mut proj: Option<Project> = None;
        let mut touched_ids: Vec<String> = Vec::new();
        let mut consumed_event_ids: Vec<i64> = Vec::new();

        if is_agent_turn {
            if let Some(start) = turn_start {
                let mut floor = start;
                if let Some(dt) = &self.diff_tracker {
                    let st = dt.started_at;
                    if st > floor {
                        floor = st;
                    }
                }
                let window_events =
                    store::get_diff_events_in_window(Some(floor), turn_end).unwrap_or_default();
                for e in &window_events {
                    consumed_event_ids.push(e.id);
                }
                if candidates.len() == 1 {
                    let first = candidates[0].clone();
                    if !first.project_id.is_empty() {
                        touched_ids = vec![first.project_id.clone()];
                    }
                    changed_files = extract_changed_files(
                        &window_events,
                        &first.project_id,
                        &touched_ids,
                        candidates,
                    );
                    proj = Some(first);
                } else {
                    let (primary, ids) = resolve_from_events(&window_events, candidates);
                    changed_files = extract_changed_files(
                        &window_events,
                        &primary.project_id,
                        &ids,
                        candidates,
                    );
                    touched_ids = ids;
                    proj = Some(primary);
                }
            }
        }

        if proj.is_none() {
            if candidates.len() == 1 {
                proj = Some(candidates[0].clone());
            } else if candidates.len() > 1 {
                let first = candidates[0].clone();
                for c in candidates {
                    if !c.project_id.is_empty() {
                        touched_ids.push(c.project_id.clone());
                    }
                }
                proj = Some(first);
            } else {
                proj = Some(Project::default());
            }
        }
        let proj = proj.unwrap();

        if is_agent_turn && changed_files.is_empty() {
            return;
        }

        let mut file_context = serde_json::Map::new();
        file_context.insert(
            "capture_schema".into(),
            serde_json::Value::Number(versions::CAPTURE_SCHEMA_VERSION.into()),
        );
        file_context.insert(
            "cursor_mode".into(),
            serde_json::Value::String(mode.clone()),
        );
        file_context.insert("is_agentic".into(), serde_json::Value::Bool(is_agent_turn));
        if !user_bubble.relevant_files.is_empty() {
            file_context.insert(
                "relevant_files".into(),
                serde_json::Value::Array(
                    user_bubble
                        .relevant_files
                        .iter()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .collect(),
                ),
            );
        }
        if touched_ids.len() > 1 {
            file_context.insert(
                "touched_project_ids".into(),
                serde_json::Value::Array(
                    touched_ids
                        .iter()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .collect(),
                ),
            );
        }
        if !changed_files.is_empty() {
            file_context.insert(
                "changed_files".into(),
                serde_json::Value::Array(
                    changed_files
                        .iter()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .collect(),
                ),
            );
        }
        if let Some(ms) = last_assistant.turn_duration_ms {
            file_context.insert(
                "turn_duration_ms".into(),
                serde_json::Value::Number(ms.into()),
            );
        }

        let mut commit_shas: Vec<String> = Vec::new();
        let mut git_diff = String::new();
        let mut branch = String::new();
        if !proj.path.is_empty() {
            if let Some(full_session) = get_full_session_data(session_id) {
                commit_shas = get_commit_range(
                    &proj.path,
                    full_session.session_created_at,
                    full_session.session_updated_at,
                );
                branch = full_session.branch.clone();

                if !self.user_id.is_empty() {
                    let mut start_sha = String::new();
                    let mut end_sha = String::new();
                    if !commit_shas.is_empty() {
                        end_sha = commit_shas.first().cloned().unwrap_or_default();
                        start_sha = commit_shas.last().cloned().unwrap_or_default();
                    }
                    let unified_mode_ptr = if full_session.unified_mode.is_empty() {
                        None
                    } else {
                        Some(true)
                    };
                    let _ = supabase::upsert_cursor_session(
                        "",
                        &CursorSessionData {
                            session_id: session_id.to_string(),
                            branch: full_session.branch.clone(),
                            model_name: full_session.model_name.clone(),
                            is_agentic: Some(full_session.is_agentic),
                            unified_mode: unified_mode_ptr,
                            plan_mode_used: full_session.plan_mode_used,
                            debug_mode_used: full_session.debug_mode_used,
                            schema_v: full_session.schema_v,
                            context_tokens_used: full_session.context_tokens_used,
                            context_token_limit: full_session.context_token_limit,
                            files_changed_count: full_session.files_changed_count,
                            total_lines_added: full_session.total_lines_added,
                            total_lines_removed: full_session.total_lines_removed,
                            session_created_at: full_session.session_created_at,
                            session_updated_at: full_session.session_updated_at,
                            commit_sha_start: start_sha,
                            commit_sha_end: end_sha,
                            commit_shas: commit_shas.clone(),
                            meta: full_session.meta.clone(),
                            ..Default::default()
                        },
                        &proj.project_id,
                        &self.user_id,
                    );
                }
            }
            git_diff = get_git_diff(&proj.path);
        }

        let hash = supabase::prompt_content_hash_v2(session_id, &user_bubble.text, &captured_at);
        let record = PromptRecord {
            id: supabase::prompt_id_v2(session_id, &user_bubble.text, &captured_at),
            content_hash: hash.clone(),
            session_id: session_id.to_string(),
            project_id: proj.project_id.clone(),
            project_name: proj.name.clone(),
            prompt_text: user_bubble.text.clone(),
            response_text: response_text.to_string(),
            model: model_name,
            source: "cursor".into(),
            capture_method: "prompt-scanner".into(),
            captured_at: captured_at.clone(),
            user_id: self.user_id.clone(),
            file_context: Some(file_context),
            ..Default::default()
        };

        if let Err(e) = store::save_draft(&record, &commit_shas, &git_diff, "") {
            display::print_error("cursor", &format!("Failed to save draft: {e}"));
            return;
        }
        let _ = store::delete_diff_events_by_id(&consumed_event_ids);
        let _ = store::mark_bubble_saved(session_id, &user_bubble.bubble_id, &hash);

        if !show_output {
            return;
        }

        let display_name = if proj.name.is_empty() {
            "?".to_string()
        } else {
            proj.name.clone()
        };
        if self.user_id.is_empty() {
            display::print_drafted(&display::DraftDisplayOptions {
                project_name: &display_name,
                branch: &branch,
                prompt_text: &user_bubble.text,
                exchange_count: 1,
            });
        } else {
            display::print_captured(&display::CaptureDisplayOptions {
                project_name: &display_name,
                branch: &branch,
                prompt_text: &user_bubble.text,
                exchange_count: 1,
                ..Default::default()
            });
        }
    }

    fn watch_fsnotify(self: Arc<Self>) {
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let Ok(mut watcher) = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        }) else {
            display::print_error("cursor fsnotify", "failed to create watcher");
            return;
        };
        for entry in WalkDir::new(&self.dir).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_dir() {
                let _ = watcher.watch(entry.path(), RecursiveMode::NonRecursive);
            }
        }

        let debounce_fire: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let debounce_checker = debounce_fire.clone();
        let scanner = self.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(100));
            let due = {
                let Ok(mut guard) = debounce_checker.lock() else {
                    continue;
                };
                match *guard {
                    Some(fire_at) if Instant::now() >= fire_at => {
                        *guard = None;
                        true
                    }
                    _ => false,
                }
            };
            if due {
                scanner.scan();
            }
        });

        loop {
            let Ok(event) = rx.recv() else {
                return;
            };
            let Ok(event) = event else {
                continue;
            };
            if matches!(event.kind, EventKind::Create(_)) {
                for p in &event.paths {
                    if p.is_dir() {
                        let _ = watcher.watch(p, RecursiveMode::NonRecursive);
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
                if is_agent_transcript(p) {
                    if let Ok(mut guard) = debounce_fire.lock() {
                        *guard = Some(Instant::now() + Duration::from_millis(500));
                    }
                }
            }
        }
    }
}

/// Entry point used by `Source::start`.
pub fn run(user_id: &str, dir: &Path) {
    let dt = Arc::new(DiffTracker::new(Duration::from_secs(3)));
    let dt_for_loop = dt.clone();
    thread::spawn(move || dt_for_loop.run_blocking());

    let state_watcher = super::session_state_watcher::SessionStateWatcher::new();
    thread::spawn(move || state_watcher.run_blocking());

    let scanner = Arc::new(PromptScanner::new(
        dir.to_path_buf(),
        user_id.to_string(),
        Some(dt),
    ));
    scanner.start();
}

/// Run a one-shot sync of the N most recently modified transcript files.
/// Called by `pcr bundle` before showing the draft list.
pub fn force_sync(user_id: &str, max_files: usize) {
    let dir = super::cursor_projects_dir();
    if !dir.exists() {
        return;
    }
    let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for entry in WalkDir::new(&dir).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_dir() {
            continue;
        }
        let path = entry.path();
        if !is_agent_transcript(path) {
            continue;
        }
        if let Ok(md) = entry.metadata() {
            if let Ok(modified) = md.modified() {
                files.push((path.to_path_buf(), modified));
            }
        }
    }
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files.truncate(max_files);
    if files.is_empty() {
        return;
    }

    let scanner = Arc::new(PromptScanner::new(dir, user_id.to_string(), None));
    for (path, _) in &files {
        if let Some((slug, sid)) = parse_transcript_path(path) {
            scanner.process_session(&slug, &sid);
        }
    }
}

// ─── Path helpers ────────────────────────────────────────────────────────────

pub(crate) fn is_agent_transcript(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    s.ends_with(".jsonl") && s.contains("/agent-transcripts/") && !s.contains("/subagents/")
}

pub(crate) fn parse_transcript_path(path: &Path) -> Option<(String, String)> {
    let s = path.to_string_lossy().replace('\\', "/");
    let parts: Vec<&str> = s.split('/').collect();
    for (i, p) in parts.iter().enumerate() {
        if *p == "agent-transcripts" && i >= 1 {
            let slug = parts[i - 1].to_string();
            let session_uuid = path
                .file_stem()
                .and_then(|s| s.to_str().map(|s| s.to_string()))
                .unwrap_or_default();
            return Some((slug, session_uuid));
        }
    }
    None
}

fn parse_bubble_time(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.3fZ") {
        return Some(Utc.from_utc_datetime(&dt));
    }
    None
}

// ─── Attribution helpers ─────────────────────────────────────────────────────

fn resolve_from_events(events: &[DiffEvent], candidates: &[Project]) -> (Project, Vec<String>) {
    use std::collections::HashMap;
    let mut hits: HashMap<String, usize> = HashMap::new();
    let mut by_id: HashMap<String, Project> = HashMap::new();
    for c in candidates {
        if !c.project_id.is_empty() {
            by_id.insert(c.project_id.clone(), c.clone());
        }
    }
    for e in events {
        if by_id.contains_key(&e.project_id) {
            *hits.entry(e.project_id.clone()).or_insert(0) += e.files.len();
        }
    }
    let mut all_ids: Vec<String> = hits.keys().cloned().collect();
    all_ids.sort();
    let mut primary: Option<Project> = None;
    let mut primary_hits = 0usize;
    for (id, n) in &hits {
        if *n == 0 {
            continue;
        }
        let p = by_id.get(id).cloned().unwrap_or_default();
        match &primary {
            None => {
                primary = Some(p);
                primary_hits = *n;
            }
            Some(cur) => {
                if *n > primary_hits || (*n == primary_hits && p.path.len() > cur.path.len()) {
                    primary = Some(p);
                    primary_hits = *n;
                }
            }
        }
    }
    (primary.unwrap_or_default(), all_ids)
}

fn extract_changed_files(
    events: &[DiffEvent],
    primary_project_id: &str,
    touched_ids: &[String],
    candidates: &[Project],
) -> Vec<String> {
    if events.is_empty() {
        return Vec::new();
    }
    let mut filter: HashSet<String> = HashSet::new();
    if !primary_project_id.is_empty() {
        filter.insert(primary_project_id.to_string());
    }
    for id in touched_ids {
        if !id.is_empty() {
            filter.insert(id.clone());
        }
    }
    if filter.is_empty() {
        for c in candidates {
            if !c.project_id.is_empty() {
                filter.insert(c.project_id.clone());
            }
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut files: Vec<String> = Vec::new();
    for e in events {
        if !filter.is_empty() && !filter.contains(&e.project_id) {
            continue;
        }
        for f in &e.files {
            if seen.insert(f.clone()) {
                files.push(f.clone());
            }
        }
    }
    files
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let take: String = s.chars().take(n.saturating_sub(1)).collect();
    format!("{take}…")
}

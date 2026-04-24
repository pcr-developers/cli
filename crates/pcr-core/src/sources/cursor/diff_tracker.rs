//! Per-project content-hash diff tracker. Direct port of
//! `cli/internal/sources/cursor/diff_tracker.go`.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config;
use crate::display;
use crate::projects;
use crate::store;

#[derive(Debug, Clone)]
pub struct DiffTracker {
    poll_interval: Duration,
    pub started_at: DateTime<Utc>,
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    // projectPath → relFile → contentHash.
    prev_state: HashMap<String, HashMap<String, String>>,
    watched_project_ids: HashSet<String>,
    fresh_start: bool,
}

impl DiffTracker {
    pub fn new(poll_interval: Duration) -> Self {
        let mut inner = Inner {
            prev_state: HashMap::new(),
            watched_project_ids: HashSet::new(),
            fresh_start: true,
        };
        load_state(&mut inner);
        Self {
            poll_interval,
            started_at: Utc::now(),
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn register_project(&self, id: &str) {
        if id.is_empty() {
            return;
        }
        if let Ok(mut guard) = self.inner.lock() {
            guard.watched_project_ids.insert(id.to_string());
        }
    }

    /// `poll()` — mirror of the Go `(*diffTracker).poll`.
    pub fn poll(&self) {
        let watched_ids: HashSet<String> = match self.inner.lock() {
            Ok(guard) => guard.watched_project_ids.clone(),
            Err(_) => return,
        };
        if watched_ids.is_empty() {
            if let Ok(mut guard) = self.inner.lock() {
                guard.fresh_start = false;
            }
            return;
        }

        let now = Utc::now();
        for p in projects::load() {
            if p.path.is_empty() || p.project_id.is_empty() || !watched_ids.contains(&p.project_id)
            {
                continue;
            }
            let current = dirty_hashes(&p.path);
            let (prev, known_project, fresh_start) = {
                let Ok(mut guard) = self.inner.lock() else {
                    continue;
                };
                let prev = guard.prev_state.get(&p.path).cloned().unwrap_or_default();
                let known = guard.prev_state.contains_key(&p.path);
                let fresh = guard.fresh_start;
                guard.prev_state.insert(p.path.clone(), current.clone());
                (prev, known, fresh)
            };

            if fresh_start || !known_project {
                continue;
            }
            let mut changed: Vec<String> = Vec::new();
            for (rel, hash) in &current {
                if prev.get(rel) != Some(hash) {
                    changed.push(
                        PathBuf::from(&p.path)
                            .join(rel)
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
            if !changed.is_empty() {
                let _ = store::record_diff_event(&p.project_id, &p.name, &changed, now);
                for f in &changed {
                    let base = PathBuf::from(f)
                        .file_name()
                        .and_then(|s| s.to_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| f.clone());
                    display::print_verbose_event("diff", &format!("[{}]  {}", p.name, base));
                }
            }
        }
        if let Ok(mut guard) = self.inner.lock() {
            guard.fresh_start = false;
        }
        let _ = store::prune_diff_events(now - ChronoDuration::hours(1));
        save_state(&self.inner);
    }

    /// Run the blocking poll loop. `start()` in Go spawns a ticker; we call
    /// this inside a thread.
    pub fn run_blocking(&self) {
        // Discard any diff events older than our start time — they came from
        // a previous run.
        let _ = store::prune_diff_events(self.started_at);
        loop {
            std::thread::sleep(self.poll_interval);
            self.poll();
        }
    }
}

// ─── State persistence ───────────────────────────────────────────────────────

fn state_path() -> PathBuf {
    config::pcr_dir().join("diff-tracker-state.json")
}

fn load_state(inner: &mut Inner) {
    let Ok(bytes) = std::fs::read(state_path()) else {
        return;
    };
    if let Ok(loaded) = serde_json::from_slice::<HashMap<String, HashMap<String, String>>>(&bytes) {
        inner.prev_state = loaded;
    }
}

fn save_state(inner: &Arc<Mutex<Inner>>) {
    let Ok(guard) = inner.lock() else {
        return;
    };
    let snapshot = guard.prev_state.clone();
    drop(guard);
    let Ok(bytes) = serde_json::to_vec(&snapshot) else {
        return;
    };
    if let Some(parent) = state_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(state_path(), bytes);
}

// ─── Git helpers ─────────────────────────────────────────────────────────────

fn dirty_hashes(project_path: &str) -> HashMap<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .arg("status")
        .arg("--porcelain")
        .output();
    let Ok(output) = out else {
        return HashMap::new();
    };
    if !output.status.success() || output.stdout.is_empty() {
        return HashMap::new();
    }
    let mut result: HashMap<String, String> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).split('\n') {
        if line.len() < 4 {
            continue;
        }
        let mut rel = line[3..].trim().to_string();
        if rel.len() >= 2 && rel.starts_with('"') && rel.ends_with('"') {
            rel = rel[1..rel.len() - 1].to_string();
        }
        if rel.is_empty() || rel.ends_with('/') {
            continue;
        }
        let Ok(content) = std::fs::read(PathBuf::from(project_path).join(&rel)) else {
            continue;
        };
        let mut h = Sha256::new();
        h.update(&content);
        // Match Go's `fmt.Sprintf("%x", h[:16])` — first 16 hex chars of the digest.
        let digest = h.finalize();
        let hex_full = hex::encode(digest);
        let short: String = hex_full.chars().take(32).collect(); // 16 bytes = 32 hex chars
        result.insert(rel, short);
    }
    result
}

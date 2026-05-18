//! Per-source file state tracker. Mirrors
//! `cli/internal/sources/shared/state.go` including the JSON format used
//! for persistence so an upgrade from the Go build reuses the same state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::config;

#[derive(Debug, Clone)]
pub struct FileState {
    inner: Arc<Mutex<FileStateInner>>,
}

#[derive(Debug)]
struct FileStateInner {
    data: HashMap<String, i64>,
    file_path: PathBuf,
}

impl FileState {
    /// Build a per-source line-state tracker rooted at
    /// `$HOME/.pcr-dev/<name>-state.json`. Panics with a clear
    /// message if `$HOME` can't be resolved — without a stable
    /// state file the watcher would silently drop its cursor on
    /// reboot (the audit's correctness concern) and re-emit every
    /// prompt on the next start. Failing fast at watcher
    /// construction is strictly better than the previous silent
    /// `/tmp` fallback. Source-watcher entry points
    /// (`vscode::watcher::run`, `claudecode::watcher::run`) are
    /// the only callers; all of them are themselves invoked from
    /// `pcr start` which has already validated the directory via
    /// `pid_file_path()?`.
    pub fn new(name: &str) -> Self {
        let file_path = config::pcr_dir()
            .expect(
                "pcr: cannot determine $HOME — refusing to put the watcher state file \
                 under /tmp (would reset capture cursors on every reboot)",
            )
            .join(format!("{name}-state.json"));
        let state = Self {
            inner: Arc::new(Mutex::new(FileStateInner {
                data: HashMap::new(),
                file_path,
            })),
        };
        state.load();
        state
    }

    pub fn load(&self) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        let Ok(bytes) = std::fs::read(&guard.file_path) else {
            return;
        };
        if let Ok(loaded) = serde_json::from_slice::<HashMap<String, i64>>(&bytes) {
            guard.data = loaded;
        }
    }

    pub fn get(&self, path: &str) -> i64 {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.data.get(path).copied())
            .unwrap_or(0)
    }

    pub fn set(&self, path: &str, lines: i64) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        guard.data.insert(path.to_string(), lines);
        let data = match serde_json::to_vec_pretty(&guard.data) {
            Ok(v) => v,
            Err(_) => return,
        };
        if let Some(parent) = guard.file_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&guard.file_path, data);
    }
}

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
    pub fn new(name: &str) -> Self {
        let file_path = config::pcr_dir().join(format!("{name}-state.json"));
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

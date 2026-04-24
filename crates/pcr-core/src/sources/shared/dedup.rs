//! In-memory dedup set keyed by (session_id, content_hash). Mirrors
//! `cli/internal/sources/shared/dedup.go`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct Deduplicator {
    inner: Arc<Mutex<HashMap<String, HashSet<String>>>>,
}

impl Deduplicator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_duplicate(&self, session_id: &str, hash: &str) -> bool {
        let Ok(guard) = self.inner.lock() else {
            return false;
        };
        guard
            .get(session_id)
            .map(|set| set.contains(hash))
            .unwrap_or(false)
    }

    pub fn mark(&self, session_id: &str, hash: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard
                .entry(session_id.to_string())
                .or_default()
                .insert(hash.to_string());
        }
    }
}

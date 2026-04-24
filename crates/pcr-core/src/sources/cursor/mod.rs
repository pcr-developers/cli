//! Cursor capture source. Full port of
//! `cli/internal/sources/cursor/*.go`:
//!
//! - [`db`] reads Cursor's internal `state.vscdb` SQLite database with a 60 s cache.
//! - [`diff_tracker`] polls registered projects every 3 s and records `diff_events`.
//! - [`session_state_watcher`] polls composer rows every 2 s for mode/model/context changes.
//! - [`watcher`] (the PromptScanner) runs a 20 s poll + fsnotify fast-path over
//!   `~/.cursor/projects/<slug>/agent-transcripts/`, extracts bubbles from SQLite,
//!   computes per-turn attribution, and saves drafts.

pub mod db;
pub mod diff_tracker;
pub mod session_state_watcher;
pub mod watcher;

use std::path::PathBuf;

use crate::sources::CaptureSource;

pub struct Source;

impl CaptureSource for Source {
    fn name(&self) -> &'static str {
        "Cursor"
    }
    fn start(&self, user_id: &str) {
        let dir = cursor_projects_dir();
        watcher::run(user_id, &dir);
    }
}

/// `~/.cursor/projects/`.
pub fn cursor_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(".cursor")
        .join("projects")
}

/// Force-sync the most recent N sessions. Called by `pcr bundle` and
/// `pcr log` to pull in late-arriving prompts.
pub fn force_sync(user_id: &str, max_files: usize) {
    watcher::force_sync(user_id, max_files);
}

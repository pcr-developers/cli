//! VS Code Copilot Chat capture source. Full port of
//! `cli/internal/sources/vscode/*.go`:
//!
//! - [`workspace`] — discovers VS Code workspaces via `~/Library/Application Support/Code/User/workspaceStorage`
//! - [`watcher`]  — fsnotify over the Copilot chat transcript files, with 1 s debounce
//! - [`parser`]   — parses Copilot's chat transcript JSONL into `ParsedExchange`s
//! - [`empty_window`] — handles the "window with no workspace" global-storage format

pub mod empty_window;
pub mod parser;
pub mod watcher;
pub mod workspace;

use std::path::PathBuf;

use crate::display;
use crate::sources::CaptureSource;

pub struct Source;

impl CaptureSource for Source {
    fn name(&self) -> &'static str {
        "VS Code"
    }
    fn start(&self, user_id: &str) {
        let dir = workspace::workspace_storage_dir();
        display::print_watcher_ready("VS Code", &dir.display().to_string());
        watcher::run(user_id, &dir);
    }
}

/// Base `User/workspaceStorage` directory per-platform.
pub fn default_workspace_storage() -> PathBuf {
    workspace::workspace_storage_dir()
}

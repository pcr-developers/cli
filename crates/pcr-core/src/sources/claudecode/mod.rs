//! Claude Code capture source. Full port of
//! `cli/internal/sources/claudecode/*.go`:
//!
//! - [`parser`] reads JSONL transcripts and extracts user-prompt turns with
//!   their tool_use / tool_result blocks, thinking chunks, and token counts.
//! - [`watcher`] runs fsnotify over `~/.claude/projects/*/` with a 1 s
//!   write-debounce, enriches or saves each new turn, and prints live
//!   capture lines.
//! - [`hook`] handles `pcr hook` — the Claude Code Stop-hook that prompts
//!   the user via `/dev/tty` to bundle any new drafts.

pub mod hook;
pub mod parser;
pub mod watcher;

use std::path::PathBuf;

use crate::sources::CaptureSource;

pub struct Source;

impl CaptureSource for Source {
    fn name(&self) -> &'static str {
        "Claude Code"
    }
    fn start(&self, user_id: &str) {
        let dir = claude_projects_dir();
        watcher::run(user_id, &dir);
    }
}

/// `~/.claude/projects/` — where Claude Code drops per-workspace session
/// JSONL transcripts.
pub fn claude_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(".claude")
        .join("projects")
}

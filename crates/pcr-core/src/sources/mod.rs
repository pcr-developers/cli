//! Capture sources (Cursor, Claude Code, VS Code). Mirrors
//! `cli/internal/sources/`.
//!
//! Each source implements a `start(user_id)` entry that runs a blocking
//! watcher loop. The [`all`] registry enumerates them for `pcr start`.

pub mod claudecode;
pub mod cursor;
pub mod shared;
pub mod vscode;

/// Common trait for a capture source.
pub trait CaptureSource: Send + Sync {
    /// Display name for UI purposes ("Claude Code", "Cursor", "VS Code").
    fn name(&self) -> &'static str;
    /// Start the blocking watcher loop.
    fn start(&self, user_id: &str);
}

pub fn all() -> Vec<Box<dyn CaptureSource>> {
    vec![
        Box::new(claudecode::Source),
        Box::new(cursor::Source),
        Box::new(vscode::Source),
    ]
}

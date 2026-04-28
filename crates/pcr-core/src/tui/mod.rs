//! ratatui-based full-screen TUI for interactive commands (`start`, `show`,
//! `status`, `help`). Gated by [`crate::agent::is_tui_eligible`] — any non-TTY,
//! `CI`, `NO_COLOR`, or explicit `--plain`/`--json` flag falls back to
//! line output.
//!
//! Architecture:
//!
//! - [`app`] — terminal lifecycle (raw mode, alt screen, panic-safe restore).
//! - [`events`] — unified event stream merging keyboard, ticks, and the
//!   global display-sink so watcher writes never bypass the TUI.
//! - [`theme`] — single source of truth for colors, glyphs, and styles
//!   shared across every screen.
//! - [`widgets`] — reusable building blocks (status row, event log,
//!   sparkline pulse, segmented bar, etc).
//! - [`screens`] — per-command full-screen UIs.

pub mod app;
pub mod events;
pub mod screens;
pub mod theme;
pub mod widgets;

pub use app::{restore_terminal, setup_terminal, Term};
pub use events::{Event, EventSource};

/// Cross-screen navigation target. `pcr start`, `pcr show`, and `pcr
/// bundle` all share a Tab / Left / Right cycle so the user can flip
/// between the live dashboard, the drafts list, and the bundles list
/// without re-running a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTarget {
    /// Stay on the current screen.
    Stay,
    /// Quit the TUI entirely.
    Quit,
    /// Jump to the live `pcr start` dashboard.
    Start,
    /// Jump to the drafts list.
    Drafts,
    /// Jump to the bundles list.
    Bundles,
    /// Quit the TUI and run `pcr push` against every sealed bundle.
    PushAfterExit,
}

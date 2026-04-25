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

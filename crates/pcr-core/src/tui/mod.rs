//! ratatui-based full-screen TUI for interactive commands (`start`, `show`,
//! `status`). Gated by [`crate::agent::is_tui_eligible`] — any non-TTY,
//! `CI`, `NO_COLOR`, or explicit `--plain`/`--json` flag falls back to
//! line output.
//!
//! The module is designed so that every screen can be driven off a single
//! shared event loop in [`app`]. Individual screens live in [`screens`].

pub mod app;
pub mod events;
pub mod screens;
pub mod widgets;

pub use app::{restore_terminal, setup_terminal, Term};
pub use events::{Event, EventSource};

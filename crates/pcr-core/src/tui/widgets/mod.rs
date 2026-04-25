//! Reusable ratatui widgets shared across screens. Every widget here uses
//! [`crate::tui::theme`] for colors and glyphs so the dashboard, browser,
//! and status views feel like one coherent system.

pub mod event_log;
pub mod header_bar;
pub mod project_row;
pub mod segmented_bar;
pub mod source_row;
pub mod sparkline;

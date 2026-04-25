//! Shared visual vocabulary for every ratatui screen. The CLI ANSI codes in
//! [`crate::display`] and the dashboard Tailwind tokens both consume the
//! same semantic palette so the product feels like one design language
//! whether you're in a terminal or a browser.

use ratatui::style::{Color, Modifier, Style};

/// Brand cyan — used for the PCR.dev logo, accent strokes, key affordances.
pub const ACCENT: Color = Color::Rgb(96, 196, 219);
/// Subtle gray for chrome, separators, secondary metadata.
pub const CHROME: Color = Color::Rgb(110, 118, 129);
/// Light text for primary content.
pub const TEXT: Color = Color::Rgb(228, 230, 235);
/// Dim text for secondary content.
pub const DIM: Color = Color::Rgb(140, 145, 153);

// Semantic status colors — match `display::Color` and Tailwind tokens.
pub const SUCCESS: Color = Color::Rgb(110, 217, 130); // emerald
pub const PENDING: Color = Color::Rgb(232, 195, 102); // yellow
pub const DANGER: Color = Color::Rgb(232, 102, 102); // red
pub const INFO: Color = Color::Rgb(120, 192, 232); // soft cyan

/// Single-glyph status indicators that match the line-mode display.
pub mod glyphs {
    /// Successful capture / sync.
    pub const SUCCESS: &str = "●";
    /// Pending / draft / open bundle.
    pub const PENDING: &str = "◎";
    /// Empty / inactive.
    pub const EMPTY: &str = "○";
    /// Error / warning.
    pub const ERROR: &str = "⚠";
    /// User prompt / input chevron.
    pub const PROMPT: &str = "❯";
    /// Active focus pointer (selected row).
    pub const POINTER: &str = "▸";
    /// Unselected list-item bullet.
    pub const BULLET: &str = "·";
    /// Live activity tick / pulse.
    pub const PULSE: &str = "◉";
    /// Section separator dot.
    pub const SEP: &str = "·";
}

/// Style helpers — keep ratatui boilerplate out of screen code.
pub fn accent() -> Style {
    Style::default().fg(ACCENT)
}

pub fn accent_bold() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn chrome() -> Style {
    Style::default().fg(CHROME)
}

pub fn dim() -> Style {
    Style::default().fg(DIM)
}

pub fn text() -> Style {
    Style::default().fg(TEXT)
}

pub fn text_bold() -> Style {
    Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
}

pub fn success() -> Style {
    Style::default().fg(SUCCESS)
}

pub fn pending() -> Style {
    Style::default().fg(PENDING)
}

pub fn danger() -> Style {
    Style::default().fg(DANGER)
}

pub fn info() -> Style {
    Style::default().fg(INFO)
}

/// Color a status glyph based on the implied state name.
pub fn glyph_for(state: &str) -> (&'static str, Style) {
    match state {
        "ready" | "active" | "captured" => (glyphs::SUCCESS, success()),
        "starting" | "initializing" => (glyphs::PENDING, pending()),
        "waiting" | "missing" | "draft" | "open" => (glyphs::PENDING, pending()),
        "empty" | "idle" => (glyphs::EMPTY, dim()),
        "error" | "failed" => (glyphs::ERROR, danger()),
        _ => (glyphs::BULLET, dim()),
    }
}

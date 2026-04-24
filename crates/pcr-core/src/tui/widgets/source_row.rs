//! Single-line per-source status row rendered on the `start` dashboard.
//!
//! Shows: source name, watcher state (ready/starting/error), last event
//! timestamp, count of drafts captured since launch.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;

#[derive(Debug, Clone)]
pub struct SourceRowData {
    pub name: &'static str,
    pub state: &'static str,
    pub last_event: String,
    pub captures: u64,
}

pub fn render(frame: &mut Frame, area: Rect, data: &SourceRowData) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(16),
            Constraint::Length(12),
            Constraint::Min(10),
            Constraint::Length(12),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(data.name).style(Style::default().add_modifier(Modifier::BOLD)),
        cols[0],
    );
    frame.render_widget(Paragraph::new(data.state), cols[1]);
    frame.render_widget(Paragraph::new(data.last_event.as_str()), cols[2]);
    frame.render_widget(Paragraph::new(format!("{} drafts", data.captures)), cols[3]);
}

/// Marker trait so this module appears in the public API for future
/// widget evolution (avoids "unused import" noise if a screen stops using
/// this widget temporarily).
pub trait _Marker: Widget {}

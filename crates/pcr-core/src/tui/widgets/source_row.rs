//! Single-line source-status row for the `pcr start` watcher panel.
//!
//! Layout (single line, dynamic):
//! `<glyph> <name 12c>  <state 9c>  <dir flexible>          <count right-aligned>`
//!
//! Glyph color reflects state. The dir is right-truncated with a leading
//! ellipsis when it doesn't fit (`…/Library/Application Support/Code`).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::display::SourceState;
use crate::tui::theme::{self, glyphs};

#[derive(Debug, Clone)]
pub struct SourceRowData {
    pub name: &'static str,
    pub state: SourceState,
    /// Subtitle for the right column (e.g. "12 sessions", "3 workspaces",
    /// "0 captures yet"). Free-form so each source picks the most
    /// informative metric.
    pub subtitle: String,
}

pub fn render(frame: &mut Frame, area: Rect, data: &SourceRowData) {
    let (glyph, glyph_style) = match &data.state {
        SourceState::Initializing => (glyphs::PENDING, theme::pending()),
        SourceState::Ready { .. } => (glyphs::SUCCESS, theme::success()),
        SourceState::Missing { .. } => (glyphs::PENDING, theme::pending()),
        SourceState::Errored { .. } => (glyphs::ERROR, theme::danger()),
    };

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(2),  // glyph
            Constraint::Length(13), // name
            Constraint::Length(10), // state label
            Constraint::Min(10),    // dir
            Constraint::Length(20), // subtitle (right-aligned)
        ])
        .split(area);

    frame.render_widget(Paragraph::new(Span::styled(glyph, glyph_style)), cols[0]);
    frame.render_widget(
        Paragraph::new(Span::styled(data.name, theme::text_bold())),
        cols[1],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(data.state.label(), theme::dim())),
        cols[2],
    );

    let dir = match &data.state {
        SourceState::Ready { dir } | SourceState::Missing { dir } => dir.as_str(),
        _ => "",
    };
    let dir_text = ellipsize_left(dir, cols[3].width as usize);
    frame.render_widget(
        Paragraph::new(Span::styled(dir_text, theme::chrome())),
        cols[3],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            data.subtitle.clone(),
            theme::dim(),
        )))
        .alignment(ratatui::layout::Alignment::Right),
        cols[4],
    );
}

/// Truncate `s` from the left with a leading `…` so the trailing path
/// component is preserved (`/Users/.../Cursor/projects` → `…ts`).
fn ellipsize_left(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let take = max - 1;
    let chars: Vec<char> = s.chars().collect();
    let start = chars.len().saturating_sub(take);
    let tail: String = chars[start..].iter().collect();
    format!("…{tail}")
}

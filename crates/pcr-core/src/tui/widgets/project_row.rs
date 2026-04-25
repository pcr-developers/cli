//! Single-line project row for the `pcr start` projects panel.
//!
//! Layout: `<glyph> <name 18c>  <branch 14c>  <draft count>  <pipeline mini>`

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::tui::theme::{self, glyphs};

#[derive(Debug, Clone)]
pub struct ProjectRowData {
    pub name: String,
    pub branch: String,
    pub draft_count: u64,
    pub staged_count: u64,
    pub bundle_count: u64,
    /// True when the project has had any activity in the current session.
    pub has_recent_activity: bool,
    /// True when this row is the currently focused one in the panel.
    pub focused: bool,
}

pub fn render(frame: &mut Frame, area: Rect, data: &ProjectRowData) {
    let (glyph, glyph_style) = if data.has_recent_activity {
        (glyphs::SUCCESS, theme::success())
    } else if data.draft_count + data.staged_count + data.bundle_count > 0 {
        (glyphs::PENDING, theme::pending())
    } else {
        (glyphs::EMPTY, theme::dim())
    };

    let pointer = if data.focused {
        Span::styled(glyphs::POINTER, theme::accent())
    } else {
        Span::raw(" ")
    };

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(2),  // pointer
            Constraint::Length(2),  // glyph
            Constraint::Length(20), // name
            Constraint::Length(16), // branch
            Constraint::Length(12), // draft pipe
            Constraint::Min(8),     // counts
        ])
        .split(area);

    frame.render_widget(Paragraph::new(pointer), cols[0]);
    frame.render_widget(Paragraph::new(Span::styled(glyph, glyph_style)), cols[1]);
    frame.render_widget(
        Paragraph::new(Span::styled(
            ellipsize_right(&data.name, cols[2].width as usize),
            theme::text_bold(),
        )),
        cols[2],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            ellipsize_right(&data.branch, cols[3].width as usize),
            theme::chrome(),
        )),
        cols[3],
    );

    // Mini pipeline: drafts ▸ staged ▸ bundled
    let pipe = format!(
        "{} {} {} {} {}",
        data.draft_count,
        glyphs::SEP,
        data.staged_count,
        glyphs::SEP,
        data.bundle_count,
    );
    frame.render_widget(Paragraph::new(Span::styled(pipe, theme::dim())), cols[4]);

    let total = data.draft_count + data.staged_count + data.bundle_count;
    let counts = if total > 0 {
        format!("{total} unbundled")
    } else {
        "—".to_string()
    };
    frame.render_widget(Paragraph::new(Span::styled(counts, theme::dim())), cols[5]);
}

fn ellipsize_right(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let take = max - 1;
    let head: String = s.chars().take(take).collect();
    format!("{head}…")
}

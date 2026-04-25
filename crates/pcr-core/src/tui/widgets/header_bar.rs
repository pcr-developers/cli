//! Sticky header rendered at the top of every full-screen TUI view.
//!
//! Layout: `[brand] [version] [user] · spacer · [clock]`
//!
//! Always one line tall. Resilient to narrow terminals — drops the user
//! and version chips before truncating the brand.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::tui::theme::{self, glyphs};

#[derive(Debug, Clone)]
pub struct HeaderBar {
    pub version: String,
    pub user: Option<String>,
    pub command: &'static str,
    pub clock: String,
}

impl HeaderBar {
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(10)])
            .split(area);

        // Left side — brand, command badge, version, user
        let mut left: Vec<Span<'_>> = vec![
            Span::styled("PCR", theme::accent_bold()),
            Span::styled(".dev", theme::accent()),
            Span::raw("  "),
            Span::styled(format!("· {} ·", self.command), theme::dim()),
            Span::raw("  "),
            Span::styled(format!("v{}", self.version), theme::chrome()),
        ];
        if let Some(u) = &self.user {
            if area.width > 60 {
                left.push(Span::raw("  "));
                left.push(Span::styled(glyphs::SUCCESS, theme::success()));
                left.push(Span::raw(" "));
                left.push(Span::styled(u.clone(), theme::text()));
            }
        } else if area.width > 60 {
            left.push(Span::raw("  "));
            left.push(Span::styled("not signed in", theme::pending()));
        }

        frame.render_widget(
            Paragraph::new(Line::from(left)).style(Style::default()),
            cols[0],
        );

        // Right side — clock
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                self.clock.clone(),
                theme::chrome(),
            )))
            .alignment(ratatui::layout::Alignment::Right),
            cols[1],
        );
    }

    /// Draw the header bordered by a single bottom rule. Useful when the
    /// header sits flush against another panel.
    pub fn render_with_rule(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::BOTTOM)
            .border_style(theme::chrome());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.render(frame, inner);
    }
}

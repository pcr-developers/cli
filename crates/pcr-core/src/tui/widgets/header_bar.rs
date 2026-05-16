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
            // Strip a leading `v` if the version string already has one
            // — release CI passes `v0.2.3` (from the git tag) while local
            // dev gets `0.2.3` from CARGO_PKG_VERSION. Without this we'd
            // render "vv0.2.3".
            Span::styled(
                format!("v{}", self.version.trim_start_matches('v')),
                theme::chrome(),
            ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Render `bar` onto an 80×3 test backend and return the flattened
    /// row-major text content (no styles). Styles are intentionally
    /// dropped — asserting on `Cell` styling makes tests fragile to
    /// theme tweaks. The cell glyphs alone catch the regressions we
    /// actually care about (missing version, brand, user, clock).
    fn render_to_text(bar: &HeaderBar, width: u16) -> String {
        let backend = TestBackend::new(width, 3);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, 1);
                bar.render(frame, area);
            })
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_brand_command_version_and_user() {
        let bar = HeaderBar {
            version: "0.2.8".into(),
            user: Some("dev@example.com".into()),
            command: "status",
            clock: "12:34:56".into(),
        };
        let text = render_to_text(&bar, 80);
        assert!(text.contains("PCR"), "brand missing: {text:?}");
        assert!(text.contains(".dev"), "tld missing: {text:?}");
        assert!(text.contains("status"), "command missing: {text:?}");
        assert!(text.contains("v0.2.8"), "version missing: {text:?}");
        assert!(text.contains("dev@example.com"), "user missing: {text:?}");
        assert!(text.contains("12:34:56"), "clock missing: {text:?}");
    }

    #[test]
    fn version_string_strips_leading_v_so_we_never_render_double() {
        // Release CI passes the version straight from the git tag
        // (`v0.2.8`). Without the strip in `render`, the header would
        // print `vv0.2.8`. This regression was a real bug — keep it
        // pinned.
        let bar = HeaderBar {
            version: "v0.2.8".into(),
            user: None,
            command: "status",
            clock: "00:00:00".into(),
        };
        let text = render_to_text(&bar, 80);
        assert!(text.contains("v0.2.8"));
        assert!(!text.contains("vv0.2.8"), "doubled v: {text:?}");
    }

    #[test]
    fn shows_not_signed_in_when_user_is_none_and_wide_enough() {
        let bar = HeaderBar {
            version: "0.2.8".into(),
            user: None,
            command: "status",
            clock: "00:00:00".into(),
        };
        let text = render_to_text(&bar, 80);
        assert!(
            text.contains("not signed in"),
            "anonymous label missing: {text:?}"
        );
    }
}

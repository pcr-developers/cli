//! Scrolling, color-coded event log used at the bottom of dashboard
//! screens. Holds a bounded `VecDeque<Entry>` so memory stays flat even
//! during a long-running `pcr start`.

use std::collections::VecDeque;

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::display::DisplayEvent;
use crate::tui::theme::{self, glyphs};

/// Log entry — pre-rendered into ratatui spans so render is O(visible).
pub struct Entry {
    pub line: Line<'static>,
}

pub struct EventLog {
    pub title: &'static str,
    pub capacity: usize,
    pub entries: VecDeque<Entry>,
    pub verbose: bool,
}

impl EventLog {
    pub fn new(title: &'static str, capacity: usize) -> Self {
        Self {
            title,
            capacity,
            entries: VecDeque::with_capacity(capacity),
            verbose: false,
        }
    }

    pub fn push_event(&mut self, ev: &DisplayEvent) {
        // Drop verbose events when verbose mode is off — same gating as
        // line mode's `display::is_verbose` check.
        if matches!(ev, DisplayEvent::Verbose { .. }) && !self.verbose {
            return;
        }
        let line = format_event(ev);
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry { line });
    }

    /// Render the log inside a bordered box. Last entries are most recent;
    /// they appear at the bottom (typical log convention).
    pub fn render(&self, frame: &mut Frame, area: Rect, status_label: &str) {
        let title = format!(" {} · {} ", self.title, status_label);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::chrome())
            .title(Line::from(Span::styled(title, theme::dim())));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Show only as many lines as fit; truncate from the front so the
        // newest entries are always visible at the bottom.
        let height = inner.height as usize;
        let start = self.entries.len().saturating_sub(height);
        let lines: Vec<Line<'_>> = self
            .entries
            .iter()
            .skip(start)
            .map(|e| e.line.clone())
            .collect();

        if lines.is_empty() {
            let placeholder = Line::from(Span::styled(
                "no events yet — capture something in your editor and watch this fill up",
                theme::dim(),
            ));
            frame.render_widget(
                Paragraph::new(vec![placeholder]).wrap(Wrap { trim: false }),
                inner,
            );
        } else {
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
    }
}

fn format_event(ev: &DisplayEvent) -> Line<'static> {
    match ev {
        DisplayEvent::Banner {
            version,
            project_count,
            ..
        } => Line::from(vec![
            Span::styled(glyphs::SUCCESS, theme::success()),
            Span::raw("  "),
            Span::styled(format!("PCR.dev v{version} started"), theme::text_bold()),
            Span::raw("  "),
            Span::styled(format!("watching {project_count} project(s)"), theme::dim()),
        ]),
        DisplayEvent::SourceState { source, state } => {
            let (glyph, style) = theme::glyph_for(state.label());
            Line::from(vec![
                Span::styled(glyph, style),
                Span::raw("  "),
                Span::styled(source.to_string(), theme::text_bold()),
                Span::raw("  "),
                Span::styled(state.label(), theme::dim()),
            ])
        }
        DisplayEvent::Captured {
            project_name,
            prompt_text,
            timestamp,
            tool_summary,
            exchange_count,
            ..
        } => {
            let mut spans = vec![
                Span::styled(timestamp.clone(), theme::chrome()),
                Span::raw("  "),
                Span::styled(glyphs::SUCCESS, theme::success()),
                Span::raw("  "),
                Span::styled(project_name.clone(), theme::text_bold()),
                Span::raw("  "),
                Span::styled(format!("\"{}\"", clip(prompt_text, 50)), theme::text()),
            ];
            if !tool_summary.is_empty() {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(tool_summary.clone(), theme::accent()));
            }
            if *exchange_count > 1 {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("({} exchanges)", exchange_count),
                    theme::dim(),
                ));
            }
            Line::from(spans)
        }
        DisplayEvent::Drafted {
            project_name,
            prompt_text,
            timestamp,
            ..
        } => Line::from(vec![
            Span::styled(timestamp.clone(), theme::chrome()),
            Span::raw("  "),
            Span::styled(glyphs::PENDING, theme::pending()),
            Span::raw("  "),
            Span::styled(project_name.clone(), theme::text_bold()),
            Span::raw("  "),
            Span::styled(format!("\"{}\"", clip(prompt_text, 50)), theme::text()),
            Span::raw("  "),
            Span::styled("(local only — not signed in)", theme::dim()),
        ]),
        DisplayEvent::Verbose {
            source,
            msg,
            timestamp,
        } => Line::from(vec![
            Span::styled(timestamp.clone(), theme::chrome()),
            Span::raw("  "),
            Span::styled(format!("~ {}", source), theme::dim()),
            Span::raw("  "),
            Span::styled(msg.clone(), theme::dim()),
        ]),
        DisplayEvent::Error {
            context,
            msg,
            timestamp,
        } => Line::from(vec![
            Span::styled(timestamp.clone(), theme::chrome()),
            Span::raw("  "),
            Span::styled(glyphs::ERROR, theme::danger()),
            Span::raw("  "),
            Span::styled(format!("{}: ", context), theme::danger()),
            Span::styled(msg.clone(), theme::text()),
        ]),
        DisplayEvent::Hint { msg } => Line::from(vec![
            Span::styled("  →  ", theme::accent()),
            Span::styled(msg.clone(), theme::dim()),
        ]),
        DisplayEvent::Line { msg } => Line::from(Span::styled(msg.clone(), theme::text())),
    }
}

fn clip(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let take = max.saturating_sub(1);
    let head: String = chars.iter().take(take).collect();
    format!("{}…", head.trim_end())
}

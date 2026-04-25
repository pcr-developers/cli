//! Horizontal segmented bar — used in `pcr status` to show the
//! draft → staged → bundled → pushed pipeline at a glance.
//!
//! Each segment renders as a labeled block of background-colored cells.
//! Empty segments collapse to a single `·` so the bar always has visual
//! weight even when only one bucket is populated.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::tui::theme;

#[derive(Debug, Clone)]
pub struct Segment {
    pub label: &'static str,
    pub count: u64,
    pub color: ratatui::style::Color,
}

pub struct SegmentedBar<'a> {
    pub segments: &'a [Segment],
}

impl<'a> SegmentedBar<'a> {
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let total: u64 = self.segments.iter().map(|s| s.count).sum();
        if total == 0 {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "no drafts yet — run `pcr start` and send a prompt",
                    theme::dim(),
                ))),
                area,
            );
            return;
        }

        // Render as a single line of inline-colored chunks.
        // Width is allocated proportionally to the counts so each segment
        // is visible even at small counts (min 1 cell per non-empty segment).
        let usable = area.width as u64;
        let mut spans: Vec<Span<'_>> = Vec::with_capacity(self.segments.len() * 3);
        for (i, seg) in self.segments.iter().enumerate() {
            if seg.count == 0 {
                continue;
            }
            let weight = (seg.count * usable / total).max(seg.label.len() as u64 + 4);
            let label = format!(" {} {} ", seg.count, seg.label);
            // Pad label to the allocated weight (not exact pixel-perfect — close enough for terminals).
            let padded = if (label.chars().count() as u64) < weight {
                let mut p = label;
                while (p.chars().count() as u64) < weight {
                    p.push(' ');
                }
                p
            } else {
                label
            };
            spans.push(Span::styled(
                padded,
                Style::default().fg(theme::TEXT).bg(seg.color),
            ));
            if i < self.segments.len() - 1 {
                spans.push(Span::styled(" ", theme::dim()));
            }
        }

        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

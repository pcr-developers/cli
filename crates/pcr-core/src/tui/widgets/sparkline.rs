//! Pulse sparkline — a thin horizontal indicator that animates with each
//! captured event so users can tell at a glance "yes, capture is live."
//!
//! Renders one of `▁▂▃▄▅▆▇█` per recent activity bucket. Quiet periods
//! drop to `▁`; bursts climb to `█`. We keep a fixed-width ring of the
//! last N buckets so the visual width is stable.

use std::collections::VecDeque;

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::tui::theme;

/// Fixed-width activity sparkline.
pub struct Pulse {
    buckets: VecDeque<u32>,
    capacity: usize,
}

impl Pulse {
    pub fn new(capacity: usize) -> Self {
        Self {
            buckets: VecDeque::from(vec![0u32; capacity]),
            capacity,
        }
    }

    /// Bump the most-recent bucket. Call this whenever a capture lands.
    pub fn tick(&mut self) {
        if let Some(last) = self.buckets.back_mut() {
            *last = last.saturating_add(1);
        }
    }

    /// Roll the window forward — call once per second so the rightmost
    /// bucket represents the current second and the leftmost falls off.
    pub fn advance(&mut self) {
        self.buckets.push_back(0);
        while self.buckets.len() > self.capacity {
            self.buckets.pop_front();
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let max = self.buckets.iter().copied().max().unwrap_or(0);
        let glyphs = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let line: String = self
            .buckets
            .iter()
            .map(|n| {
                if *n == 0 || max == 0 {
                    '▁'
                } else {
                    let idx = ((*n as usize * 7) / (max as usize).max(1)).min(7);
                    glyphs[idx]
                }
            })
            .collect();
        let style = if max == 0 {
            theme::dim()
        } else {
            Style::default().fg(theme::ACCENT)
        };
        frame.render_widget(Paragraph::new(Line::from(Span::styled(line, style))), area);
    }
}

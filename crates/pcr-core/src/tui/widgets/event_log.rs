//! Scrolling event log widget (bottom pane of `start`, full pane of
//! `show`). Wraps a bounded VecDeque and renders it inside a ratatui
//! `Paragraph` with wrap/scroll.

use std::collections::VecDeque;

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

pub struct EventLog {
    pub title: &'static str,
    pub capacity: usize,
    pub lines: VecDeque<String>,
}

impl EventLog {
    pub fn new(title: &'static str, capacity: usize) -> Self {
        Self {
            title,
            capacity,
            lines: VecDeque::with_capacity(capacity),
        }
    }
    pub fn push(&mut self, line: String) {
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let lines: Vec<Line<'_>> = self
            .lines
            .iter()
            .map(|s| Line::from(Span::raw(s.clone())))
            .collect();
        let block = Block::default().borders(Borders::ALL).title(self.title);
        let para = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(para, area);
    }
}

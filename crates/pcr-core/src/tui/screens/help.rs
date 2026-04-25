//! `pcr help` — interactive command index.
//!
//! Two panes: the command list on the left, the formatted help entry on
//! the right. `j/k` to move, `enter` to focus the right pane (for scroll),
//! `q` to quit.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::help::{HelpEntry, HELP};
use crate::tui::app::{restore_terminal, setup_terminal};
use crate::tui::events::{Event, EventSource};
use crate::tui::theme::{self, glyphs};
use crate::tui::widgets::header_bar::HeaderBar;
use crate::util::time::local_hms;
use crate::VERSION;

pub fn run() -> Result<()> {
    let mut term = setup_terminal()?;
    let events = EventSource::spawn(Duration::from_millis(500));
    let mut focus = 0usize;
    let mut state = ListState::default();
    state.select(Some(0));

    loop {
        term.draw(|f| draw(f, focus, &mut state))?;
        match events.rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Key(KeyEvent { code, .. })) => match code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Down | KeyCode::Char('j') => {
                    focus = (focus + 1).min(HELP.len() - 1);
                    state.select(Some(focus));
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    focus = focus.saturating_sub(1);
                    state.select(Some(focus));
                }
                KeyCode::Home | KeyCode::Char('g') => {
                    focus = 0;
                    state.select(Some(0));
                }
                KeyCode::End | KeyCode::Char('G') => {
                    focus = HELP.len() - 1;
                    state.select(Some(focus));
                }
                _ => {}
            },
            Ok(_) => {}
            Err(_) => {}
        }
    }

    restore_terminal()?;
    Ok(())
}

fn draw(frame: &mut ratatui::Frame, focus: usize, list_state: &mut ListState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(10),   // body
            Constraint::Length(1), // footer
        ])
        .split(area);

    HeaderBar {
        version: VERSION.to_string(),
        user: None,
        command: "help",
        clock: local_hms(),
    }
    .render(frame, chunks[0]);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(40)])
        .split(chunks[1]);

    draw_command_list(frame, cols[0], focus, list_state);
    draw_entry(frame, cols[1], &HELP[focus]);
    draw_footer(frame, chunks[2]);
}

fn draw_command_list(
    frame: &mut ratatui::Frame,
    area: Rect,
    focus: usize,
    list_state: &mut ListState,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(" Commands ", theme::dim())));

    let items: Vec<ListItem<'_>> = HELP
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let pointer = if i == focus { glyphs::POINTER } else { " " };
            ListItem::new(Line::from(vec![
                Span::styled(pointer, theme::accent()),
                Span::raw(" "),
                Span::styled(h.command, theme::text_bold()),
            ]))
        })
        .collect();

    frame.render_stateful_widget(List::new(items).block(block), area, list_state);
}

fn draw_entry(frame: &mut ratatui::Frame, area: Rect, entry: &HelpEntry) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(
            format!(" pcr {} — {} ", entry.command, entry.short),
            theme::dim(),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let inner = inner.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(Span::styled(entry.purpose, theme::text())));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "WHEN TO USE",
        theme::accent_bold(),
    )));
    for wrapped in wrap_paragraph(entry.when_to_use, inner.width as usize) {
        lines.push(Line::from(Span::styled(wrapped, theme::text())));
    }
    lines.push(Line::from(""));

    if !entry.examples.is_empty() {
        lines.push(Line::from(Span::styled("EXAMPLES", theme::accent_bold())));
        for (cmd, desc) in entry.examples {
            lines.push(Line::from(vec![
                Span::styled("  $ ", theme::dim()),
                Span::styled(cmd.to_string(), theme::text_bold()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("      ", theme::dim()),
                Span::styled(desc.to_string(), theme::dim()),
            ]));
        }
        lines.push(Line::from(""));
    }

    if !entry.see_also.is_empty() {
        lines.push(Line::from(Span::styled("SEE ALSO", theme::accent_bold())));
        let see_also = entry
            .see_also
            .iter()
            .map(|s| format!("pcr {s}"))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(Line::from(Span::styled(see_also, theme::text())));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(vec![
        Span::styled("More: ", theme::dim()),
        Span::styled(
            format!("https://pcr.dev/docs/{}", entry.command),
            theme::accent(),
        ),
    ]));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_footer(frame: &mut ratatui::Frame, area: Rect) {
    let hints = vec![
        Span::styled("j/k", theme::accent()),
        Span::styled(" move  ", theme::dim()),
        Span::styled("g/G", theme::accent()),
        Span::styled(" top/bottom  ", theme::dim()),
        Span::styled("q", theme::accent()),
        Span::styled(" quit", theme::dim()),
    ];
    frame.render_widget(Paragraph::new(Line::from(hints)), area);
}

/// Wrap a paragraph string at `width` characters preserving word boundaries.
fn wrap_paragraph(text: &str, width: usize) -> Vec<String> {
    if width < 10 {
        return vec![text.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
            continue;
        }
        if current.len() + 1 + word.len() > width {
            out.push(std::mem::take(&mut current));
            current.push_str(word);
        } else {
            current.push(' ');
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

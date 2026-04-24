//! `pcr show` full-screen draft browser.
//!
//! Renders a two-pane layout: numbered draft list on the left, full text
//! + tool calls + changed files on the right. `j`/`k` to navigate,
//! `enter` to open, `q` to quit.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use std::time::Duration;

use crate::store::DraftRecord;
use crate::tui::app::{restore_terminal, setup_terminal};
use crate::tui::events::{Event, EventSource};

pub fn run(drafts: Vec<DraftRecord>) -> Result<()> {
    let mut term = setup_terminal()?;
    let events = EventSource::spawn(Duration::from_millis(500));
    let mut idx: usize = 0;

    loop {
        term.draw(|f| {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
                .split(f.area());

            let items: Vec<ListItem<'_>> = drafts
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let prefix = if i == idx { "» " } else { "  " };
                    let preview =
                        crate::util::text::prompt_preview(&d.prompt_text, 50);
                    ListItem::new(format!("{prefix}[{}] {preview}", i + 1))
                })
                .collect();
            f.render_widget(
                List::new(items).block(Block::default().borders(Borders::ALL).title("Drafts")),
                cols[0],
            );

            let current = drafts.get(idx);
            let right = match current {
                Some(d) => format!(
                    "source: {}\nmodel:  {}\nbranch: {}\ncaptured: {}\n\nPROMPT\n{}\n\nRESPONSE\n{}",
                    d.source,
                    if d.model.is_empty() { "—" } else { d.model.as_str() },
                    if d.branch_name.is_empty() { "—" } else { d.branch_name.as_str() },
                    d.captured_at,
                    d.prompt_text,
                    if d.response_text.is_empty() { "—" } else { d.response_text.as_str() },
                ),
                None => "No draft selected.".to_string(),
            };
            f.render_widget(
                Paragraph::new(right)
                    .wrap(Wrap { trim: false })
                    .block(Block::default().borders(Borders::ALL).title("Detail")),
                cols[1],
            );
        })?;

        match events.rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                ..
            }))
            | Ok(Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            })) => break,
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('j'),
                ..
            }))
            | Ok(Event::Key(KeyEvent {
                code: KeyCode::Down,
                ..
            })) => {
                if idx + 1 < drafts.len() {
                    idx += 1;
                }
            }
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('k'),
                ..
            }))
            | Ok(Event::Key(KeyEvent {
                code: KeyCode::Up, ..
            })) => {
                if idx > 0 {
                    idx -= 1;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            _ => {}
        }
    }

    restore_terminal()?;
    Ok(())
}

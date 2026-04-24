//! `pcr start` dashboard — renders the per-source status rows plus a
//! scrolling event log.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::app::{restore_terminal, setup_terminal};
use crate::tui::events::{Event, EventSource};
use crate::tui::widgets::{
    event_log::EventLog,
    source_row::{render as render_source_row, SourceRowData},
};
use crate::VERSION;

pub fn run(project_count: usize) -> Result<()> {
    let mut term = setup_terminal()?;
    let events = EventSource::spawn(Duration::from_millis(500));
    let mut log = EventLog::new("Events", 200);
    let sources = [
        SourceRowData {
            name: "Claude Code",
            state: "ready",
            last_event: String::from("—"),
            captures: 0,
        },
        SourceRowData {
            name: "Cursor",
            state: "ready",
            last_event: String::from("—"),
            captures: 0,
        },
        SourceRowData {
            name: "VS Code",
            state: "ready",
            last_event: String::from("—"),
            captures: 0,
        },
    ];

    loop {
        term.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(5),
                    Constraint::Min(5),
                    Constraint::Length(1),
                ])
                .split(f.area());
            let banner = Paragraph::new(format!(
                "PCR.dev v{VERSION}  —  live capture stream  —  {project_count} project(s) registered"
            ))
            .block(Block::default().borders(Borders::ALL).title("pcr start"));
            f.render_widget(banner, chunks[0]);

            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1); 3])
                .split(chunks[1].inner(ratatui::layout::Margin { vertical: 1, horizontal: 1 }));
            for (i, s) in sources.iter().enumerate() {
                render_source_row(f, rows[i], s);
            }

            log.render(f, chunks[2]);

            f.render_widget(
                Paragraph::new("q to quit · --plain for line mode"),
                chunks[3],
            );
        })?;

        match events.rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                ..
            }))
            | Ok(Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }))
            | Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            })) => break,
            Ok(Event::Tick(_)) => {}
            Ok(Event::Key(_)) => {}
            Ok(Event::Custom(kind, msg)) => log.push(format!("{kind}: {msg}")),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }

    restore_terminal()?;
    Ok(())
}

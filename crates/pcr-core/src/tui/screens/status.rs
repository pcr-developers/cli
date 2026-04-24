//! `pcr status` compact TUI. Single-screen overview of auth, projects,
//! bundles, drafts. `q` to quit.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::auth;
use crate::projects;
use crate::store;
use crate::tui::app::{restore_terminal, setup_terminal};
use crate::tui::events::{Event, EventSource};

pub fn run() -> Result<()> {
    let mut term = setup_terminal()?;
    let events = EventSource::spawn(Duration::from_millis(500));

    let a = auth::load();
    let projs = projects::load();
    let unpushed = store::get_unpushed_commits().unwrap_or_default();
    let drafts =
        store::get_drafts_by_status(store::DraftStatus::Draft, &[], &[]).unwrap_or_default();

    loop {
        term.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(5),
                    Constraint::Length(5),
                    Constraint::Length(1),
                ])
                .split(f.area());
            let auth_line = match &a {
                Some(a) => format!("✓ Logged in (user: {})", a.user_id),
                None => "Not logged in — run `pcr login`".to_string(),
            };
            f.render_widget(
                Paragraph::new(auth_line)
                    .block(Block::default().borders(Borders::ALL).title("Auth")),
                chunks[0],
            );

            let items: Vec<ListItem<'_>> = if projs.is_empty() {
                vec![ListItem::new("No projects registered. Run `pcr init`.")]
            } else {
                projs
                    .iter()
                    .map(|p| ListItem::new(format!("{} — {}", p.name, p.path)))
                    .collect()
            };
            f.render_widget(
                List::new(items).block(Block::default().borders(Borders::ALL).title("Projects")),
                chunks[1],
            );

            let bundle_text = if unpushed.is_empty() {
                "Bundles: none — everything pushed".to_string()
            } else {
                let lines: Vec<String> = unpushed
                    .iter()
                    .map(|b| format!("{}  {}  ({})", b.bundle_status, b.message, b.id))
                    .collect();
                format!("Bundles:\n{}", lines.join("\n"))
            };
            let draft_text = if drafts.is_empty() {
                "Drafts: none".to_string()
            } else {
                format!("Drafts: {} unreviewed", drafts.len())
            };
            f.render_widget(
                Paragraph::new(format!("{bundle_text}\n{draft_text}"))
                    .wrap(Wrap { trim: false })
                    .block(Block::default().borders(Borders::ALL).title("State")),
                chunks[2],
            );
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
            })) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            _ => {}
        }
    }
    restore_terminal()?;
    Ok(())
}

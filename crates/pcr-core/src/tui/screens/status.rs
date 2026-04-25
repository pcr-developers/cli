//! `pcr status` — one-screen overview of auth, projects, and pipeline.
//!
//! Layout:
//!
//! ```text
//! ┌─ HEADER ────────────────────────────────────────────────────────────┐
//! │  AUTH                                                                │
//! │  ✓ Signed in as bhada@pcr.dev                                        │
//! │                                                                      │
//! │  PIPELINE                                                            │
//! │  [ 12 drafts ][ 3 staged ][ 2 bundled ][ 47 pushed ]                 │
//! │                                                                      │
//! │  PROJECTS · 7 registered                                             │
//! │  ▸ pcr-dev      main          5 drafts  ●  3 unbundled               │
//! │  ▸ cli          rust-port     2 drafts  ●  2 in open bundle          │
//! │  ▸ functions    main          0 drafts  ○                            │
//! │  …                                                                   │
//! │                                                                      │
//! │  NEXT                                                                │
//! │  → run `pcr bundle "name" --select all` to package drafts            │
//! │  → run `pcr push` once you have a sealed bundle                      │
//! └──────────────────────────────────────────────────────────────────────┘
//! ```

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::Color as RColor;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::auth;
use crate::projects;
use crate::store::{self, DraftStatus};
use crate::tui::app::{restore_terminal, setup_terminal};
use crate::tui::events::{Event, EventSource};
use crate::tui::theme::{self, glyphs};
use crate::tui::widgets::{
    header_bar::HeaderBar,
    project_row::{render as render_project_row, ProjectRowData},
    segmented_bar::{Segment, SegmentedBar},
};
use crate::util::time::local_hms;
use crate::VERSION;

pub fn run() -> Result<()> {
    let mut term = setup_terminal()?;
    let events = EventSource::spawn(Duration::from_millis(500));

    loop {
        let snapshot = StatusSnapshot::load();
        term.draw(|f| draw(f, &snapshot))?;

        match events.rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                ..
            }))
            | Ok(Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }))
            | Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('r'),
                ..
            })) => {
                if matches!(
                    events.rx.try_recv(),
                    Ok(Event::Key(KeyEvent {
                        code: KeyCode::Char('r'),
                        ..
                    }))
                ) {
                    continue;
                }
                break;
            }
            Ok(Event::Tick(_)) | Ok(Event::Display(_)) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            _ => {}
        }
    }

    restore_terminal()?;
    Ok(())
}

struct StatusSnapshot {
    user: Option<String>,
    projects: Vec<projects::Project>,
    project_counts: Vec<(projects::Project, u64, u64, u64)>, // drafts, staged, bundles
    pipeline: PipelineCounts,
}

#[derive(Default)]
struct PipelineCounts {
    drafts: u64,
    staged: u64,
    bundled: u64,
    pushed: u64,
}

impl StatusSnapshot {
    fn load() -> Self {
        let user = auth::load().map(|a| a.user_id);
        let projects = projects::load();
        let mut project_counts = Vec::with_capacity(projects.len());

        for p in &projects {
            let ids: &[String] = if p.project_id.is_empty() {
                &[]
            } else {
                std::slice::from_ref(&p.project_id)
            };
            let names = std::slice::from_ref(&p.name);
            let drafts = store::get_drafts_by_status(DraftStatus::Draft, ids, names)
                .map(|v| v.len() as u64)
                .unwrap_or(0);
            let staged = store::get_drafts_by_status(DraftStatus::Staged, ids, names)
                .map(|v| v.len() as u64)
                .unwrap_or(0);
            let bundles = store::list_commits(Some(false), ids, &[])
                .map(|v| v.len() as u64)
                .unwrap_or(0);
            project_counts.push((p.clone(), drafts, staged, bundles));
        }

        // Sort by total descending so the most-active projects float up.
        project_counts.sort_by_key(|(p, d, s, b)| (std::cmp::Reverse(d + s + b), p.name.clone()));

        let drafts = store::get_drafts_by_status(DraftStatus::Draft, &[], &[])
            .map(|v| v.len() as u64)
            .unwrap_or(0);
        let staged = store::get_drafts_by_status(DraftStatus::Staged, &[], &[])
            .map(|v| v.len() as u64)
            .unwrap_or(0);
        let bundled = store::list_commits(Some(false), &[], &[])
            .map(|v| v.iter().map(|c| c.items.len() as u64).sum::<u64>())
            .unwrap_or(0);
        let pushed = store::list_commits(Some(true), &[], &[])
            .map(|v| v.iter().map(|c| c.items.len() as u64).sum::<u64>())
            .unwrap_or(0);

        Self {
            user,
            projects,
            project_counts,
            pipeline: PipelineCounts {
                drafts,
                staged,
                bundled,
                pushed,
            },
        }
    }
}

fn draw(frame: &mut ratatui::Frame, snap: &StatusSnapshot) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(3), // auth
            Constraint::Length(4), // pipeline
            Constraint::Min(6),    // projects
            Constraint::Length(5), // next-action
            Constraint::Length(1), // footer
        ])
        .split(area);

    HeaderBar {
        version: VERSION.to_string(),
        user: snap.user.clone(),
        command: "status",
        clock: local_hms(),
    }
    .render(frame, chunks[0]);

    draw_auth(frame, chunks[1], snap);
    draw_pipeline(frame, chunks[2], snap);
    draw_projects(frame, chunks[3], snap);
    draw_next_action(frame, chunks[4], snap);
    draw_footer(frame, chunks[5]);
}

fn draw_auth(frame: &mut ratatui::Frame, area: Rect, snap: &StatusSnapshot) {
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(" Auth ", theme::dim())));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let inner = inner.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });

    let line = match &snap.user {
        Some(u) => Line::from(vec![
            Span::styled(glyphs::SUCCESS, theme::success()),
            Span::raw("  "),
            Span::styled("Signed in as ", theme::dim()),
            Span::styled(u.clone(), theme::text_bold()),
        ]),
        None => Line::from(vec![
            Span::styled(glyphs::ERROR, theme::pending()),
            Span::raw("  "),
            Span::styled("Not signed in.  ", theme::pending()),
            Span::styled("→ run ", theme::dim()),
            Span::styled("`pcr login`", theme::accent_bold()),
        ]),
    };
    frame.render_widget(Paragraph::new(line), inner);
}

fn draw_pipeline(frame: &mut ratatui::Frame, area: Rect, snap: &StatusSnapshot) {
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(" Pipeline ", theme::dim())));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let inner = inner.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });

    let segments: [Segment; 4] = [
        Segment {
            label: "drafts",
            count: snap.pipeline.drafts,
            color: RColor::Rgb(80, 80, 95),
        },
        Segment {
            label: "staged",
            count: snap.pipeline.staged,
            color: theme::PENDING,
        },
        Segment {
            label: "bundled",
            count: snap.pipeline.bundled,
            color: theme::INFO,
        },
        Segment {
            label: "pushed",
            count: snap.pipeline.pushed,
            color: theme::SUCCESS,
        },
    ];
    let bar_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    SegmentedBar {
        segments: &segments,
    }
    .render(frame, bar_area);
}

fn draw_projects(frame: &mut ratatui::Frame, area: Rect, snap: &StatusSnapshot) {
    let title = format!(" Projects · {} registered ", snap.projects.len());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(title, theme::dim())));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let inner = inner.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });

    if snap.projects.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  No projects registered yet.",
                    theme::pending(),
                )),
                Line::from(Span::styled(
                    "  → cd into any git repo and run  `pcr init`",
                    theme::dim(),
                )),
            ]),
            inner,
        );
        return;
    }

    let visible = snap.project_counts.len().min(inner.height as usize);
    let constraints: Vec<Constraint> =
        std::iter::repeat_n(Constraint::Length(1), visible).collect();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (i, (p, drafts, staged, bundles)) in snap.project_counts.iter().take(visible).enumerate() {
        render_project_row(
            frame,
            rows[i],
            &ProjectRowData {
                name: p.name.clone(),
                branch: branch_for(&p.path),
                draft_count: *drafts,
                staged_count: *staged,
                bundle_count: *bundles,
                has_recent_activity: false,
                focused: false,
            },
        );
    }
}

fn branch_for(path: &str) -> String {
    // Detached HEAD comes back as the literal "HEAD"; `get_branch`
    // normalizes that to empty string.
    crate::sources::shared::git::get_branch(path)
}

fn draw_next_action(frame: &mut ratatui::Frame, area: Rect, snap: &StatusSnapshot) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(" Next ", theme::dim())));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let inner = inner.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let lines: Vec<Line<'_>> = next_actions(snap)
        .into_iter()
        .map(|s| {
            Line::from(vec![
                Span::styled("→ ", theme::accent()),
                Span::styled(s, theme::text()),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn next_actions(snap: &StatusSnapshot) -> Vec<String> {
    let mut hints: Vec<String> = Vec::new();
    if snap.user.is_none() {
        hints.push("sign in: pcr login".into());
    }
    if snap.projects.is_empty() {
        hints.push("register a project: cd <repo> && pcr init".into());
    } else if snap.pipeline.drafts == 0 && snap.pipeline.staged == 0 && snap.pipeline.bundled == 0 {
        hints.push(
            "capture your first prompt: pcr start  (then send a prompt in your editor)".into(),
        );
    } else if snap.pipeline.drafts > 0 || snap.pipeline.staged > 0 {
        hints.push("package drafts into a bundle: pcr bundle \"name\" --select all".into());
    } else if snap.pipeline.bundled > 0 {
        hints.push("ship the bundle for review: pcr push".into());
    }
    if hints.is_empty() {
        hints.push("everything is up to date  ✓".into());
    }
    hints
}

fn draw_footer(frame: &mut ratatui::Frame, area: Rect) {
    let hints = vec![
        Span::styled("r", theme::accent()),
        Span::styled(" refresh  ", theme::dim()),
        Span::styled("q", theme::accent()),
        Span::styled(" quit", theme::dim()),
    ];
    frame.render_widget(Paragraph::new(Line::from(hints)), area);
}

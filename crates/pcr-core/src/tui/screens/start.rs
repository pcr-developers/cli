//! `pcr start` live dashboard.
//!
//! Five-section layout (top to bottom): header, watchers panel, projects
//! panel, events log, footer keybinds. State is driven by a single
//! [`DashboardState`] struct that the event loop mutates in place; the
//! render pass is pure read-only.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::auth;
use crate::display::{self, DisplayEvent, SourceState};
use crate::projects;
use crate::store::{self, DraftStatus};
use crate::tui::app::{restore_terminal, setup_terminal};
use crate::tui::events::{Event, EventSource};
use crate::tui::theme::{self, glyphs};
use crate::tui::widgets::{
    event_log::EventLog,
    header_bar::HeaderBar,
    project_row::{render as render_project_row, ProjectRowData},
    source_row::{render as render_source_row, SourceRowData},
    sparkline::Pulse,
};
use crate::util::time::local_hms;
use crate::VERSION;

const PROJECT_REFRESH_TICKS: u32 = 4; // refresh project counts every 2s (tick=500ms)

/// Mutable state for the dashboard. Render reads, event loop writes.
struct DashboardState {
    sources: BTreeMap<String, SourceState>,
    /// Per-project name → activity flag for the current session.
    activity: BTreeMap<String, bool>,
    /// Per-project draft / staged / bundled counts. Refreshed every 2s.
    counts: Vec<ProjectCount>,
    /// Selected index inside the project panel for keyboard nav.
    project_focus: usize,
    /// True when the user has toggled verbose mode on with `v`.
    verbose: bool,
    /// True when the user pressed `p` to pause incoming events (events
    /// are still queued — we just stop drawing new ones in real time).
    paused: bool,
    /// Live activity sparkline.
    pulse: Pulse,
    /// Tick counter for periodic project-count refreshes.
    tick_counter: u32,
    /// Total drafts captured this session (across all sources).
    captured_session: u64,
}

#[derive(Clone, Default)]
struct ProjectCount {
    name: String,
    branch: String,
    drafts: u64,
    staged: u64,
    bundles: u64,
}

impl DashboardState {
    fn new() -> Self {
        Self {
            sources: BTreeMap::new(),
            activity: BTreeMap::new(),
            counts: Vec::new(),
            project_focus: 0,
            verbose: false,
            paused: false,
            pulse: Pulse::new(40),
            tick_counter: 0,
            captured_session: 0,
        }
    }

    fn ingest(&mut self, ev: &DisplayEvent) {
        match ev {
            DisplayEvent::SourceState { source, state } => {
                self.sources.insert(source.clone(), state.clone());
            }
            DisplayEvent::Captured { project_name, .. } => {
                if !project_name.is_empty() {
                    self.activity.insert(project_name.clone(), true);
                }
                self.pulse.tick();
                self.captured_session += 1;
            }
            DisplayEvent::Drafted { project_name, .. } => {
                if !project_name.is_empty() {
                    self.activity.insert(project_name.clone(), true);
                }
                self.pulse.tick();
                self.captured_session += 1;
            }
            _ => {}
        }
    }

    fn refresh_counts(&mut self) {
        let projs = projects::load();
        let mut out: Vec<ProjectCount> = Vec::with_capacity(projs.len());
        for p in &projs {
            // Empty project_id (anonymous capture) means "no filter".
            let id_filter: &[String] = if p.project_id.is_empty() {
                &[]
            } else {
                std::slice::from_ref(&p.project_id)
            };
            let name_filter = std::slice::from_ref(&p.name);
            let drafts = store::get_drafts_by_status(DraftStatus::Draft, id_filter, name_filter)
                .map(|v| v.len() as u64)
                .unwrap_or(0);
            let staged = store::get_drafts_by_status(DraftStatus::Staged, id_filter, name_filter)
                .map(|v| v.len() as u64)
                .unwrap_or(0);
            // Bundle count = unpushed commits whose project_id matches.
            let bundles = store::list_commits(Some(false), id_filter, &[])
                .map(|v| v.len() as u64)
                .unwrap_or(0);
            out.push(ProjectCount {
                name: p.name.clone(),
                branch: shortest_branch_for_project(&p.path),
                drafts,
                staged,
                bundles,
            });
        }
        // Sort by activity then by total count then by name.
        out.sort_by(|a, b| {
            let aa = self.activity.get(&a.name).copied().unwrap_or(false);
            let bb = self.activity.get(&b.name).copied().unwrap_or(false);
            bb.cmp(&aa)
                .then((b.drafts + b.staged + b.bundles).cmp(&(a.drafts + a.staged + a.bundles)))
                .then(a.name.cmp(&b.name))
        });
        self.counts = out;
        self.project_focus = self.project_focus.min(self.counts.len().saturating_sub(1));
    }

    fn total_unbundled(&self) -> u64 {
        self.counts.iter().map(|p| p.drafts + p.staged).sum()
    }

    fn total_bundles(&self) -> u64 {
        self.counts.iter().map(|p| p.bundles).sum()
    }
}

fn shortest_branch_for_project(path: &str) -> String {
    // Detached HEAD becomes empty string via `get_branch`'s normalization.
    crate::sources::shared::git::get_branch(path)
}

pub fn run(_project_count: usize) -> Result<()> {
    let mut term = setup_terminal()?;
    let events = EventSource::spawn(Duration::from_millis(500));
    let mut log = EventLog::new("Events", 500);
    let mut state = DashboardState::new();
    state.refresh_counts();

    let user = auth::load().map(|a| a.user_id);

    loop {
        term.draw(|f| draw(f, &state, &log, user.as_deref()))?;

        match events.rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Key(k)) => {
                if !handle_key(k, &mut state, &mut log) {
                    break;
                }
            }
            Ok(Event::Tick(_)) => {
                state.tick_counter = state.tick_counter.wrapping_add(1);
                state.pulse.advance();
                if state.tick_counter % PROJECT_REFRESH_TICKS == 0 {
                    state.refresh_counts();
                }
            }
            Ok(Event::Display(de)) => {
                state.ingest(&de);
                if !state.paused {
                    log.push_event(&de);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }

    restore_terminal()?;
    Ok(())
}

fn handle_key(k: KeyEvent, state: &mut DashboardState, log: &mut EventLog) -> bool {
    match (k.code, k.modifiers) {
        (KeyCode::Char('q'), _)
        | (KeyCode::Esc, _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return false,
        (KeyCode::Char('v'), _) => {
            state.verbose = !state.verbose;
            log.verbose = state.verbose;
            display::set_verbose(state.verbose);
        }
        (KeyCode::Char('p'), _) => {
            state.paused = !state.paused;
        }
        (KeyCode::Char('r'), _) => {
            state.refresh_counts();
        }
        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
            if !state.counts.is_empty() {
                state.project_focus = (state.project_focus + 1).min(state.counts.len() - 1);
            }
        }
        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
            state.project_focus = state.project_focus.saturating_sub(1);
        }
        _ => {}
    }
    true
}

fn draw(frame: &mut ratatui::Frame, state: &DashboardState, log: &EventLog, user: Option<&str>) {
    let area = frame.area();

    // Five-row layout. Watchers panel + projects panel get min heights so
    // narrow terminals collapse the events log first.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                                          // header
            Constraint::Length(1), // separator / pulse line
            Constraint::Length(state.sources.len().clamp(3, 5) as u16 + 3), // watchers
            Constraint::Length(state.counts.len().min(8) as u16 + 3), // projects
            Constraint::Min(6),    // events log
            Constraint::Length(1), // footer keybinds
        ])
        .split(area);

    // Header
    HeaderBar {
        version: VERSION.to_string(),
        user: user.map(|s| s.to_string()),
        command: "start",
        clock: local_hms(),
    }
    .render(frame, chunks[0]);

    // Pulse / status line under the header
    draw_pulse_line(frame, chunks[1], state);

    // Watchers panel
    draw_watchers(frame, chunks[2], state);

    // Projects panel
    draw_projects(frame, chunks[3], state);

    // Events log
    let status = if state.paused {
        "paused".to_string()
    } else if state.verbose {
        "verbose".to_string()
    } else {
        format!("{} captured this session", state.captured_session)
    };
    log.render(frame, chunks[4], &status);

    // Footer
    draw_footer(frame, chunks[5], state);
}

fn draw_pulse_line(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(20), Constraint::Length(40)])
        .split(area);

    let summary = if state.captured_session == 0 {
        "Capture is live. Send a prompt in your editor to test it.".to_string()
    } else {
        format!(
            "Capturing — {} exchange{} this session, {} unbundled across {} project{}",
            state.captured_session,
            if state.captured_session == 1 { "" } else { "s" },
            state.total_unbundled(),
            state.counts.len(),
            if state.counts.len() == 1 { "" } else { "s" },
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(summary, theme::dim()),
        ])),
        cols[0],
    );
    state.pulse.render(frame, cols[1]);
}

fn draw_watchers(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(" Watchers ", theme::dim())));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.sources.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  starting capture sources…",
                theme::dim(),
            ))),
            inner.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
        );
        return;
    }

    let inner = inner.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let constraints: Vec<Constraint> =
        std::iter::repeat_n(Constraint::Length(1), state.sources.len()).collect();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (i, (name, st)) in state.sources.iter().enumerate() {
        let subtitle = match st {
            SourceState::Ready { .. } => format!("ready · {}", name_subtitle(name)),
            SourceState::Initializing => "starting".to_string(),
            SourceState::Missing { .. } => "waiting for tool".to_string(),
            SourceState::Errored { msg } => format!("error: {}", clip(msg, 30)),
        };
        // SAFETY of the &'static lifetime: the watcher names are
        // hardcoded literals ("Cursor", "Claude Code", "VS Code"); the
        // BTreeMap stores owned Strings, so we leak nothing — we just
        // need a &'static for the SourceRowData. We use a leak-free
        // mapping below.
        let static_name: &'static str = match name.as_str() {
            "Cursor" => "Cursor",
            "Claude Code" => "Claude Code",
            "VS Code" => "VS Code",
            _ => "Source",
        };
        render_source_row(
            frame,
            rows[i],
            &SourceRowData {
                name: static_name,
                state: st.clone(),
                subtitle,
            },
        );
    }
}

fn name_subtitle(name: &str) -> &'static str {
    match name {
        "Cursor" => "fsnotify + 20s scan",
        "Claude Code" => "fsnotify + 1s debounce",
        "VS Code" => "workspace storage",
        _ => "watching",
    }
}

fn draw_projects(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let title = format!(
        " Projects · {} registered · {} unbundled · {} bundle{} ",
        state.counts.len(),
        state.total_unbundled(),
        state.total_bundles(),
        if state.total_bundles() == 1 { "" } else { "s" },
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(title, theme::dim())));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.counts.is_empty() {
        let inner = inner.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("no projects registered yet", theme::pending())),
                Line::from(""),
                Line::from(Span::styled(
                    "  → run  `pcr init`  inside any git repo",
                    theme::dim(),
                )),
            ]),
            inner,
        );
        return;
    }

    let inner = inner.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let visible = state.counts.len().min(inner.height as usize);
    let constraints: Vec<Constraint> =
        std::iter::repeat_n(Constraint::Length(1), visible).collect();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (i, p) in state.counts.iter().take(visible).enumerate() {
        render_project_row(
            frame,
            rows[i],
            &ProjectRowData {
                name: p.name.clone(),
                branch: p.branch.clone(),
                draft_count: p.drafts,
                staged_count: p.staged,
                bundle_count: p.bundles,
                has_recent_activity: state.activity.get(&p.name).copied().unwrap_or(false),
                focused: i == state.project_focus,
            },
        );
    }
}

fn draw_footer(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let mut hints: Vec<Span<'_>> = vec![
        Span::styled("↑↓/jk", theme::accent()),
        Span::styled(" project  ", theme::dim()),
        Span::styled("v", theme::accent()),
        Span::styled(
            if state.verbose {
                " verbose on  "
            } else {
                " verbose  "
            },
            theme::dim(),
        ),
        Span::styled("p", theme::accent()),
        Span::styled(
            if state.paused {
                " resume  "
            } else {
                " pause  "
            },
            theme::dim(),
        ),
        Span::styled("r", theme::accent()),
        Span::styled(" refresh  ", theme::dim()),
        Span::styled("q", theme::accent()),
        Span::styled(" quit", theme::dim()),
    ];
    if state.captured_session > 0 {
        hints.insert(0, Span::styled("  ", theme::dim()));
        hints.insert(
            0,
            Span::styled(
                format!("{} {}", glyphs::SUCCESS, state.captured_session),
                theme::success(),
            ),
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(hints)).alignment(Alignment::Left),
        area,
    );
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

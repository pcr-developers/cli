//! `pcr show` — three-pane draft browser.
//!
//! Layout:
//!
//! ```text
//! ┌─ HEADER ────────────────────────────────────────────────────────────┐
//! │ DRAFTS ▼      │ PROMPT                              │ CHANGED FILES │
//! │  1 ▸ pcr-dev  │ "fix the bug in render"             │ src/page.tsx  │
//! │  2   cli      │                                     │ src/main.rs   │
//! │  3   func     │ RESPONSE                            │               │
//! │              │ Done — applied 2 edits.             │ TOOL CALLS    │
//! │              │                                     │ Write × 2     │
//! │              │ METADATA                            │ Read  × 5     │
//! │              │ branch · main                       │               │
//! │              │ source · cursor                     │               │
//! │              │ model  · claude-sonnet-4-6          │               │
//! │              │ mode   · agent                      │               │
//! │              │ when   · 14:08                      │               │
//! └─────────────────────────────────────────────────────────────────────┘
//!  j/k move · enter open file · c copy prompt · b bundle · d delete · ?
//! ```

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::store::DraftRecord;
use crate::tui::app::{restore_terminal, setup_terminal};
use crate::tui::events::{Event, EventSource};
use crate::tui::theme::{self, glyphs};
use crate::tui::widgets::header_bar::HeaderBar;
use crate::util::time::{fmt_time, local_hms};
use crate::VERSION;

pub fn run(drafts: Vec<DraftRecord>) -> Result<()> {
    let mut term = setup_terminal()?;
    let events = EventSource::spawn(Duration::from_millis(500));
    let mut state = ShowState {
        drafts,
        focus: 0,
        list_state: ListState::default(),
        copy_flash: None,
    };
    state.list_state.select(Some(0));

    loop {
        term.draw(|f| draw(f, &state))?;

        match events.rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Key(k)) => {
                if !handle_key(k, &mut state) {
                    break;
                }
            }
            Ok(Event::Tick(_)) => {
                // Tick down the copy-flash banner so it disappears after a moment.
                if let Some((_, ref mut ttl)) = state.copy_flash {
                    *ttl = ttl.saturating_sub(1);
                    if *ttl == 0 {
                        state.copy_flash = None;
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            _ => {}
        }
    }

    restore_terminal()?;
    Ok(())
}

struct ShowState {
    drafts: Vec<DraftRecord>,
    focus: usize,
    list_state: ListState,
    /// (message, ticks_remaining) — flash banner that confirms keyboard actions.
    copy_flash: Option<(String, u32)>,
}

fn handle_key(k: KeyEvent, state: &mut ShowState) -> bool {
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => return false,
        KeyCode::Down | KeyCode::Char('j') => {
            if !state.drafts.is_empty() {
                state.focus = (state.focus + 1).min(state.drafts.len() - 1);
                state.list_state.select(Some(state.focus));
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.focus = state.focus.saturating_sub(1);
            state.list_state.select(Some(state.focus));
        }
        KeyCode::Home | KeyCode::Char('g') => {
            state.focus = 0;
            state.list_state.select(Some(0));
        }
        KeyCode::End | KeyCode::Char('G') => {
            if !state.drafts.is_empty() {
                state.focus = state.drafts.len() - 1;
                state.list_state.select(Some(state.focus));
            }
        }
        KeyCode::Char('c') => {
            // Copy the prompt to the system clipboard if available, otherwise just flash.
            if let Some(d) = state.drafts.get(state.focus) {
                let copied = copy_to_clipboard(&d.prompt_text);
                state.copy_flash = Some((
                    if copied {
                        format!("Copied prompt #{} to clipboard", state.focus + 1)
                    } else {
                        format!(
                            "Could not access clipboard (#{} prompt is selected)",
                            state.focus + 1
                        )
                    },
                    4,
                ));
            }
        }
        KeyCode::Char('b') => {
            state.copy_flash = Some((
                "To bundle this draft, run: pcr bundle \"name\" --select <number>".into(),
                6,
            ));
        }
        KeyCode::Char('d') => {
            // Delete the focused draft from the local store and drop it
            // from the in-memory list. Cursor is moved to the nearest
            // surviving sibling. No confirmation modal — the user is
            // explicitly choosing this and the action is local-only
            // (nothing was pushed); if they want it back, the original
            // session in Cursor / Claude Code will re-capture it on the
            // next watcher pass.
            if let Some(d) = state.drafts.get(state.focus).cloned() {
                let display_idx = state.focus + 1;
                match crate::store::delete_drafts(std::slice::from_ref(&d.id)) {
                    Ok(()) => {
                        state.drafts.remove(state.focus);
                        if state.drafts.is_empty() {
                            state.focus = 0;
                            state.list_state.select(None);
                        } else {
                            if state.focus >= state.drafts.len() {
                                state.focus = state.drafts.len() - 1;
                            }
                            state.list_state.select(Some(state.focus));
                        }
                        state.copy_flash = Some((format!("Deleted draft #{display_idx}"), 4));
                    }
                    Err(e) => {
                        state.copy_flash = Some((format!("Delete failed: {e}"), 6));
                    }
                }
            }
        }
        KeyCode::Char('?') => {
            state.copy_flash = Some((
                "j/k move · g/G top/bottom · c copy · b bundle hint · d delete · q quit".into(),
                8,
            ));
        }
        _ => {}
    }
    true
}

fn draw(frame: &mut ratatui::Frame, state: &ShowState) {
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
        command: "show",
        clock: local_hms(),
    }
    .render(frame, chunks[0]);

    if state.drafts.is_empty() {
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("  No drafts yet.", theme::pending())),
            Line::from(""),
            Line::from(Span::styled(
                "  → run `pcr start` and send a prompt in your editor.",
                theme::dim(),
            )),
        ])
        .alignment(Alignment::Left);
        frame.render_widget(empty, chunks[1]);
        draw_footer(frame, chunks[2], state);
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(28), // drafts list
            Constraint::Min(40),    // detail
            Constraint::Length(28), // changed files / tools
        ])
        .split(chunks[1]);

    draw_list(frame, cols[0], state);
    draw_detail(frame, cols[1], state);
    draw_sidebar(frame, cols[2], state);
    draw_footer(frame, chunks[2], state);
}

fn draw_list(frame: &mut ratatui::Frame, area: Rect, state: &ShowState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(
            format!(" Drafts · {} ", state.drafts.len()),
            theme::dim(),
        )));

    // Each row is `▸ NNN G preview` with a fixed prefix width:
    //   pointer(1) + space(1) + index(3) + space(1) + glyph(1) + space(1) = 8
    // Clamp the preview text to whatever's left of the inner column width so
    // long prompts don't soft-wrap onto the next ListItem and visually
    // contaminate adjacent rows.
    const PREFIX_WIDTH: usize = 8;
    const MIN_PREVIEW_WIDTH: usize = 8;
    let inner_width = (area.width as usize).saturating_sub(2); // minus borders
    let preview_max = inner_width
        .saturating_sub(PREFIX_WIDTH)
        .max(MIN_PREVIEW_WIDTH);

    let items: Vec<ListItem<'_>> = state
        .drafts
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let pointer = if i == state.focus {
                glyphs::POINTER
            } else {
                " "
            };
            let preview = crate::util::text::prompt_preview(&d.prompt_text, preview_max);
            let (g, gs) = source_glyph(&d.source);
            ListItem::new(Line::from(vec![
                Span::styled(pointer, theme::accent()),
                Span::raw(" "),
                Span::styled(format!("{:>3}", i + 1), theme::chrome()),
                Span::raw(" "),
                Span::styled(g, gs),
                Span::raw(" "),
                Span::styled(preview, theme::text()),
            ]))
        })
        .collect();

    let mut ls = state.list_state.clone();
    let widget = List::new(items).block(block);
    frame.render_stateful_widget(widget, area, &mut ls);
}

fn source_glyph(source: &str) -> (&'static str, ratatui::style::Style) {
    match source {
        "cursor" => ("C", theme::accent()),
        "claude-code" => ("K", theme::pending()),
        "vscode" => ("V", theme::info()),
        _ => ("?", theme::dim()),
    }
}

fn draw_detail(frame: &mut ratatui::Frame, area: Rect, state: &ShowState) {
    let Some(d) = state.drafts.get(state.focus) else {
        return;
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(
            format!(" Detail · #{} of {} ", state.focus + 1, state.drafts.len()),
            theme::dim(),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let inner = inner.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 2,
    });

    let mut lines: Vec<Line<'_>> = Vec::new();

    // Section: PROMPT
    lines.push(Line::from(vec![
        Span::styled(glyphs::PROMPT, theme::accent()),
        Span::raw(" "),
        Span::styled("PROMPT", theme::accent_bold()),
    ]));
    for line in d.prompt_text.lines().take(20) {
        lines.push(Line::from(Span::styled(line.to_string(), theme::text())));
    }
    if d.prompt_text.lines().count() > 20 {
        lines.push(Line::from(Span::styled("  …", theme::dim())));
    }
    lines.push(Line::from(""));

    // Section: RESPONSE
    if !d.response_text.is_empty() {
        lines.push(Line::from(Span::styled("RESPONSE", theme::accent_bold())));
        let chars_max = 800;
        let resp = if d.response_text.chars().count() > chars_max {
            let head: String = d.response_text.chars().take(chars_max).collect();
            format!("{head}…")
        } else {
            d.response_text.clone()
        };
        for line in resp.lines() {
            lines.push(Line::from(Span::styled(line.to_string(), theme::text())));
        }
        lines.push(Line::from(""));
    }

    // Section: METADATA
    lines.push(Line::from(Span::styled("METADATA", theme::accent_bold())));
    let meta_rows: &[(&str, String)] = &[
        ("source", d.source.clone()),
        (
            "model",
            if d.model.is_empty() {
                "—".into()
            } else {
                d.model.clone()
            },
        ),
        (
            "branch",
            if d.branch_name.is_empty() {
                "—".into()
            } else {
                d.branch_name.clone()
            },
        ),
        (
            "mode",
            d.file_context
                .as_ref()
                .and_then(|m| {
                    m.get("cursor_mode")
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                })
                .unwrap_or_else(|| "—".into()),
        ),
        ("captured", fmt_time(&d.captured_at)),
        (
            "project",
            if d.project_name.is_empty() {
                "—".into()
            } else {
                d.project_name.clone()
            },
        ),
    ];
    for (k, v) in meta_rows {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<8}", k), theme::dim()),
            Span::raw("  "),
            Span::styled(v.clone(), theme::text()),
        ]));
    }

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, inner);
}

fn draw_sidebar(frame: &mut ratatui::Frame, area: Rect, state: &ShowState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let Some(d) = state.drafts.get(state.focus) else {
        return;
    };

    // Top: changed files
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(" Changed files ", theme::dim())));
    let inner = block.inner(chunks[0]);
    frame.render_widget(block, chunks[0]);

    let changed: Vec<String> = d
        .file_context
        .as_ref()
        .and_then(|m| m.get("changed_files").and_then(|v| v.as_array().cloned()))
        .map(|a| {
            a.into_iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let lines: Vec<Line<'_>> = if changed.is_empty() {
        vec![Line::from(Span::styled(
            "  (no file changes recorded)",
            theme::dim(),
        ))]
    } else {
        changed
            .iter()
            .map(|f| {
                Line::from(vec![
                    Span::styled("  ", theme::dim()),
                    Span::styled(glyphs::BULLET, theme::accent()),
                    Span::raw(" "),
                    Span::styled(short_path(f), theme::text()),
                ])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);

    // Bottom: tool calls
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(" Tool calls ", theme::dim())));
    let inner = block.inner(chunks[1]);
    frame.render_widget(block, chunks[1]);

    let summary = crate::display::summarize_tools(&d.tool_calls);
    let lines = if summary.is_empty() {
        vec![Line::from(Span::styled("  (no tools used)", theme::dim()))]
    } else {
        summary
            .split("  ")
            .map(|t| {
                Line::from(vec![
                    Span::styled("  ", theme::dim()),
                    Span::styled(glyphs::SEP, theme::accent()),
                    Span::raw(" "),
                    Span::styled(t.to_string(), theme::text()),
                ])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_footer(frame: &mut ratatui::Frame, area: Rect, state: &ShowState) {
    if let Some((msg, _)) = &state.copy_flash {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("  {} ", glyphs::SUCCESS), theme::success()),
                Span::styled(msg.clone(), theme::text()),
            ])),
            area,
        );
        return;
    }
    let hints = vec![
        Span::styled("j/k", theme::accent()),
        Span::styled(" move  ", theme::dim()),
        Span::styled("g/G", theme::accent()),
        Span::styled(" top/bottom  ", theme::dim()),
        Span::styled("c", theme::accent()),
        Span::styled(" copy  ", theme::dim()),
        Span::styled("d", theme::accent()),
        Span::styled(" delete  ", theme::dim()),
        Span::styled("b", theme::accent()),
        Span::styled(" bundle hint  ", theme::dim()),
        Span::styled("?", theme::accent()),
        Span::styled(" help  ", theme::dim()),
        Span::styled("q", theme::accent()),
        Span::styled(" quit", theme::dim()),
    ];
    frame.render_widget(Paragraph::new(Line::from(hints)), area);
}

fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.split('/').collect();
    if parts.len() <= 3 {
        return p.to_string();
    }
    let tail = &parts[parts.len() - 3..];
    format!("…/{}", tail.join("/"))
}

/// Best-effort clipboard copy. Uses `pbcopy` on macOS, `xclip` / `wl-copy`
/// on Linux, `clip.exe` on Windows. Returns false if no clipboard tool is
/// available — the caller flashes a message either way.
fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[cfg(target_os = "macos")]
    let candidates: &[&[&str]] = &[&["pbcopy"]];
    #[cfg(target_os = "linux")]
    let candidates: &[&[&str]] = &[
        &["wl-copy"],
        &["xclip", "-selection", "clipboard"],
        &["xsel", "-bi"],
    ];
    #[cfg(target_os = "windows")]
    let candidates: &[&[&str]] = &[&["clip.exe"]];
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let candidates: &[&[&str]] = &[];

    for argv in candidates {
        let Ok(mut child) = Command::new(argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(text.as_bytes()).is_ok() {
                drop(stdin);
                if let Ok(status) = child.wait() {
                    if status.success() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

//! `pcr show` — three-pane draft browser.
//!
//! Layout:
//!
//! ```text
//! ┌─ HEADER ────────────────────────────────────────────────────────────┐
//! │ DRAFTS ▼              │ PROMPT                      │ CHANGED FILES │
//! │ ✓ 1 ▸ pcr-dev  fix t  │ "fix the bug in render"     │ src/page.tsx  │
//! │   2   cli      add r  │                             │ src/main.rs   │
//! │ ✓ 3   docs     wire   │ RESPONSE                    │               │
//! │                     │ Done — applied 2 edits.       │ TOOL CALLS    │
//! │                     │                               │ Write × 2     │
//! │                     │ METADATA                      │ Read  × 5     │
//! │                     │ branch · main                 │               │
//! │                     │ source · cursor               │               │
//! │                     │ model  · claude-sonnet-4-6    │               │
//! │                     │ mode   · agent                │               │
//! │                     │ when   · 14:08                │               │
//! └─────────────────────────────────────────────────────────────────────┘
//!  j/k move · space select · a select-all · enter bundle · d delete · q
//! ```

use std::collections::HashSet;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::commands::helpers::{current_branch, draft_ids};
use crate::commands::project_context::resolve;
use crate::store::{self, DraftRecord};
use crate::tui::app::{restore_terminal, setup_terminal};
use crate::tui::events::{Event, EventSource};
use crate::tui::theme::{self, glyphs};
use crate::tui::widgets::header_bar::HeaderBar;
use crate::util::id::generate_hex_id;
use crate::util::time::{fmt_time, local_hms};
use crate::VERSION;

pub fn run(drafts: Vec<DraftRecord>) -> Result<()> {
    run_focused(drafts, 0)
}

/// Open the show TUI focused on a specific draft index (0-based). Used by
/// `pcr show <n>` to land on the requested draft instead of the first one,
/// and by `pcr bundle` to focus on the most recent draft. Out-of-range
/// indices are clamped to the last valid row.
pub fn run_focused(drafts: Vec<DraftRecord>, initial_focus: usize) -> Result<()> {
    run_focused_with_hidden(drafts, initial_focus, 0)
}

/// Same as `run_focused`, but lets the caller report how many drafts
/// were filtered out before opening the TUI. Used by the recency cap
/// in `pcr show` / `pcr bundle` so the footer can hint that the list
/// is truncated and how to see everything.
pub fn run_focused_with_hidden(
    drafts: Vec<DraftRecord>,
    initial_focus: usize,
    hidden_count: usize,
) -> Result<()> {
    let mut term = setup_terminal()?;
    let events = EventSource::spawn(Duration::from_millis(500));
    let focus = if drafts.is_empty() {
        0
    } else {
        initial_focus.min(drafts.len() - 1)
    };
    let mut state = ShowState {
        drafts,
        focus,
        list_state: ListState::default(),
        copy_flash: None,
        hidden_count,
        selected: HashSet::new(),
        prompt: None,
    };
    state.list_state.select(Some(focus));

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
    /// Drafts filtered out *before* the TUI opened (e.g. by the recency
    /// cap in `pcr show` / `pcr bundle`). Surfaced in the footer so the
    /// user knows the list isn't the complete history and how to widen it.
    hidden_count: usize,
    /// Draft IDs the user has marked with Space. Bundling the focused
    /// row alone is the fallback when this set is empty, so casual
    /// users never need to learn multi-select to ship a single draft.
    selected: HashSet<String>,
    /// Inline name prompt shown when the user presses Enter / `b`. While
    /// `Some`, all key input goes to the modal (text input, Enter to
    /// confirm, Esc to cancel) instead of list navigation.
    prompt: Option<NamePrompt>,
}

/// Modal name input shown over the list when the user is about to
/// create a bundle. We render an overlay box near the bottom so the
/// list and detail view stay visible underneath — handy for spelling
/// the name from what's on screen.
struct NamePrompt {
    /// Current text the user has typed.
    buf: String,
    /// Snapshot of the draft IDs that will go into the bundle when the
    /// user confirms. Captured at prompt-open time so subsequent `j/k`
    /// (which we still allow during the prompt? — no, we don't) can't
    /// shift the target set behind the prompt's back.
    targets: Vec<String>,
}

fn handle_key(k: KeyEvent, state: &mut ShowState) -> bool {
    // Modal-first. While the name prompt is open, the list is frozen
    // and every keystroke goes into the prompt buffer (or controls it).
    if state.prompt.is_some() {
        return handle_prompt_key(k, state);
    }
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
        KeyCode::Char(' ') => {
            // Toggle multi-select on the focused draft. Auto-advance so
            // the user can hold space-j-space-j to mark a contiguous run.
            if let Some(d) = state.drafts.get(state.focus) {
                if state.selected.contains(&d.id) {
                    state.selected.remove(&d.id);
                } else {
                    state.selected.insert(d.id.clone());
                }
            }
            if !state.drafts.is_empty() && state.focus + 1 < state.drafts.len() {
                state.focus += 1;
                state.list_state.select(Some(state.focus));
            }
        }
        KeyCode::Char('a') => {
            // Toggle "select all visible" / "clear all". If anything is
            // selected, the first press clears (matches the gmail-style
            // mental model). Otherwise selects every visible draft.
            if state.selected.is_empty() {
                for d in &state.drafts {
                    state.selected.insert(d.id.clone());
                }
                state.copy_flash = Some((
                    format!("Selected all {} visible drafts", state.drafts.len()),
                    3,
                ));
            } else {
                let n = state.selected.len();
                state.selected.clear();
                state.copy_flash = Some((format!("Cleared {n} selection{}", plural(n)), 3));
            }
        }
        KeyCode::Enter | KeyCode::Char('b') => {
            // Open the inline name prompt. If the user has multi-
            // selected with Space, those are the targets; otherwise
            // we fall back to the focused row so single-draft bundling
            // is a two-keystroke flow (Enter, type name, Enter).
            let targets: Vec<String> = if state.selected.is_empty() {
                match state.drafts.get(state.focus) {
                    Some(d) => vec![d.id.clone()],
                    None => Vec::new(),
                }
            } else {
                // Preserve list order, not insertion order, so the
                // bundle's draft sequence reads top-to-bottom on push.
                state
                    .drafts
                    .iter()
                    .filter(|d| state.selected.contains(&d.id))
                    .map(|d| d.id.clone())
                    .collect()
            };
            if targets.is_empty() {
                state.copy_flash = Some((
                    "No drafts to bundle — nothing selected and list empty.".into(),
                    4,
                ));
            } else {
                state.prompt = Some(NamePrompt {
                    buf: String::new(),
                    targets,
                });
            }
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
                state.selected.remove(&d.id);
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
                "j/k move · space select · a select-all · enter bundle · c copy · d delete · ? help · q quit  ·  source/branch shown in the detail pane"
                    .into(),
                10,
            ));
        }
        _ => {}
    }
    true
}

/// Key dispatch while the name prompt is on screen. Returns false to
/// quit the TUI (we never quit from inside the prompt — Esc just
/// dismisses the prompt itself, matching how every other modal in
/// Cursor / VS Code works).
fn handle_prompt_key(k: KeyEvent, state: &mut ShowState) -> bool {
    let Some(prompt) = state.prompt.as_mut() else {
        return true;
    };
    match k.code {
        KeyCode::Esc => {
            state.prompt = None;
        }
        KeyCode::Backspace => {
            prompt.buf.pop();
        }
        KeyCode::Enter => {
            let name = prompt.buf.trim().to_string();
            if name.is_empty() {
                state.copy_flash = Some(("Bundle name can't be empty — Esc to cancel.".into(), 4));
                return true;
            }
            let targets = std::mem::take(&mut prompt.targets);
            // Drop the prompt before calling out so the success flash
            // renders on a clean footer.
            state.prompt = None;
            match create_bundle_from_targets(&name, &targets) {
                Ok(count) => {
                    // Drop bundled drafts from the in-memory list so the
                    // user can immediately see they've moved out of the
                    // draft pool. The store has already migrated them to
                    // the bundle in the same transaction.
                    let bundled: HashSet<String> = targets.into_iter().collect();
                    state.drafts.retain(|d| !bundled.contains(&d.id));
                    state.selected.retain(|id| !bundled.contains(id));
                    if state.drafts.is_empty() {
                        state.focus = 0;
                        state.list_state.select(None);
                    } else {
                        if state.focus >= state.drafts.len() {
                            state.focus = state.drafts.len() - 1;
                        }
                        state.list_state.select(Some(state.focus));
                    }
                    state.copy_flash = Some((
                        format!(
                            "Bundled {count} draft{} as {name:?} — run `pcr push` to ship",
                            plural(count)
                        ),
                        8,
                    ));
                }
                Err(e) => {
                    state.copy_flash = Some((format!("Bundle failed: {e}"), 8));
                }
            }
        }
        KeyCode::Char(c) => {
            // Treat Ctrl-U as line-clear; everything else printable just
            // appends. We don't intercept Ctrl-C — the global terminal
            // SIGINT handler still tears the TUI down cleanly.
            if k.modifiers.contains(KeyModifiers::CONTROL) && (c == 'u' || c == 'U') {
                prompt.buf.clear();
            } else if !k.modifiers.contains(KeyModifiers::CONTROL) {
                // Soft cap so a runaway paste can't blow the terminal width.
                if prompt.buf.chars().count() < 120 {
                    prompt.buf.push(c);
                }
            }
        }
        _ => {}
    }
    true
}

/// Tiny pluralizer helper local to this file — `crate::util::text::plural`
/// returns "s" or "" but we want it inline without the import dance.
fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Create a sealed bundle from the given draft IDs, mirroring the
/// non-interactive `pcr bundle "name" --select` codepath. Returns the
/// number of drafts actually bundled or a user-facing error string.
fn create_bundle_from_targets(name: &str, draft_ids_in: &[String]) -> Result<usize, String> {
    if draft_ids_in.is_empty() {
        return Err("nothing to bundle".into());
    }
    // Pull the same draft pool the non-interactive path sees so we can
    // map the IDs back into full records and inherit the same project
    // attribution rules.
    let ctx = resolve();
    let drafts = store::get_drafts_by_status(store::DraftStatus::Draft, &ctx.ids, &ctx.names)
        .map_err(|e| e.to_string())?;
    let staged = store::get_staged_drafts().map_err(|e| e.to_string())?;
    let mut pool: Vec<DraftRecord> = drafts;
    pool.extend(staged);

    let id_set: HashSet<&String> = draft_ids_in.iter().collect();
    let selected: Vec<DraftRecord> = pool
        .into_iter()
        .filter(|d| id_set.contains(&d.id))
        .collect();
    if selected.is_empty() {
        return Err("none of the selected drafts are still available".into());
    }

    let project_id = ctx.ids.first().cloned().unwrap_or_default();
    let project_name = ctx.name.clone();
    let branch = current_branch();
    let sha = format!("bundle-{}", generate_hex_id());

    store::create_commit(
        name,
        &sha,
        &draft_ids(&selected),
        &project_id,
        &project_name,
        &branch,
        "closed",
        ctx.single_repo,
    )
    .map_err(|e| e.to_string())?;
    Ok(selected.len())
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

    // Modal overlay last so it paints on top of the list / detail.
    if state.prompt.is_some() {
        draw_name_prompt(frame, chunks[1], state);
    }
}

/// Centered overlay that takes user input for the bundle name.
/// Rendered on top of the body chunk after the list / detail / sidebar
/// are drawn, with `Clear` to wipe whatever was underneath the box.
fn draw_name_prompt(frame: &mut ratatui::Frame, body: Rect, state: &ShowState) {
    let Some(prompt) = state.prompt.as_ref() else {
        return;
    };
    // Box: 60 cols wide (or body width), 5 rows tall, centered horizontally,
    // anchored a little above body center so the user's eyes find it fast.
    let width = 64.min(body.width.saturating_sub(4));
    let height = 7u16;
    let x = body.x + body.width.saturating_sub(width) / 2;
    let y = body.y + body.height.saturating_sub(height) / 3;
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::accent())
        .title(Line::from(Span::styled(
            format!(
                " Bundle name · {} draft{} ",
                prompt.targets.len(),
                plural(prompt.targets.len())
            ),
            theme::accent_bold(),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // hint
            Constraint::Length(1), // input
            Constraint::Length(1), // spacer
            Constraint::Length(1), // controls
        ])
        .split(inner.inner(ratatui::layout::Margin {
            vertical: 0,
            horizontal: 1,
        }));

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Name your bundle (e.g. \"auth fix\"):",
            theme::dim(),
        ))),
        chunks[0],
    );

    // Input line with a trailing block cursor so the user can see where
    // they're typing. We append `▌` rather than relying on terminal
    // cursor positioning — much simpler and works under tmux/screen.
    let cursor_style = Style::default().add_modifier(Modifier::SLOW_BLINK);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", theme::accent()),
            Span::styled(prompt.buf.clone(), theme::text()),
            Span::styled("▌", cursor_style),
        ])),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("enter", theme::accent()),
            Span::styled(" confirm   ", theme::dim()),
            Span::styled("esc", theme::accent()),
            Span::styled(" cancel   ", theme::dim()),
            Span::styled("ctrl-u", theme::accent()),
            Span::styled(" clear", theme::dim()),
        ])),
        chunks[3],
    );
}

fn draw_list(frame: &mut ratatui::Frame, area: Rect, state: &ShowState) {
    // Each row is `M ▸ NNN  REPO  preview` with a budget that adapts
    // to column width. The repo column is more useful at-a-glance than
    // the source label was — across multi-repo work it's the strongest
    // signal for what a draft is *about*. If a row has no project
    // attribution, we leave the repo slot blank rather than print "—",
    // since the column is already narrow and a placeholder steals
    // preview width without saying anything.
    //
    // Width budget:
    //   mark(1) + space(1) + pointer(1) + space(1) + index(3) + space(1) = 8
    //   repo column = REPO_WIDTH (8) when any draft in view has a repo
    //                 name, else 0 to give the preview the full column
    //   space(1) between repo and preview if repo is shown
    const FIXED_PREFIX: usize = 8;
    const REPO_WIDTH: usize = 8;
    const MIN_PREVIEW_WIDTH: usize = 8;
    let inner_width = (area.width as usize).saturating_sub(2); // minus borders
    let any_repo = state.drafts.iter().any(|d| !d.project_name.is_empty());
    let repo_block = if any_repo { REPO_WIDTH + 1 } else { 0 };
    let preview_max = inner_width
        .saturating_sub(FIXED_PREFIX + repo_block)
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
            let mark = if state.selected.contains(&d.id) {
                "✓"
            } else {
                " "
            };
            let preview = crate::util::text::prompt_preview(&d.prompt_text, preview_max);
            let mut spans = vec![
                Span::styled(mark, theme::success()),
                Span::raw(" "),
                Span::styled(pointer, theme::accent()),
                Span::raw(" "),
                Span::styled(format!("{:>3}", i + 1), theme::chrome()),
                Span::raw(" "),
            ];
            if any_repo {
                let repo = truncate_for_column(&d.project_name, REPO_WIDTH);
                spans.push(Span::styled(
                    format!("{:<width$}", repo, width = REPO_WIDTH),
                    theme::pending(),
                ));
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(preview, theme::text()));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let mut ls = state.list_state.clone();
    // Title now shows the selection count when something is marked, so
    // the user has constant feedback on how many drafts will go into a
    // bundle if they hit Enter.
    let title = if state.selected.is_empty() {
        format!(" Drafts · {} ", state.drafts.len())
    } else {
        format!(
            " Drafts · {}  ·  ✓ {} selected ",
            state.drafts.len(),
            state.selected.len()
        )
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Line::from(Span::styled(title, theme::dim())));
    let widget = List::new(items).block(block);
    frame.render_stateful_widget(widget, area, &mut ls);
}

/// Truncate `s` to fit in `width` display columns, appending `…` when
/// it has to drop characters. Approximates display width by `chars()`
/// — fine for ASCII project names and adequate for the BMP characters
/// that show up in real-world repo slugs. The list is monospaced so
/// any cell that's too narrow just gets a trailing `…`, never wrapped.
fn truncate_for_column(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= width {
        return s.to_string();
    }
    if width == 1 {
        return "…".into();
    }
    let head: String = s.chars().take(width - 1).collect();
    format!("{head}…")
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
        // Distinguish "agent ran with no tools" (claude-code, vscode)
        // from "tool calls aren't captured for this source at all"
        // (cursor — its bubble store doesn't expose structured tool
        // events the way the other watchers do). Saying "no tools used"
        // for a cursor draft was actively misleading: the agent often
        // *did* use tools, we just don't see them.
        let msg = match d.source.as_str() {
            "cursor" => "  (tool calls aren't captured for cursor)",
            _ => "  (no tools used)",
        };
        vec![Line::from(Span::styled(msg, theme::dim()))]
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
    let mut hints = vec![
        Span::styled("j/k", theme::accent()),
        Span::styled(" move  ", theme::dim()),
        Span::styled("space", theme::accent()),
        Span::styled(" select  ", theme::dim()),
        Span::styled("a", theme::accent()),
        Span::styled(" all  ", theme::dim()),
        Span::styled("enter", theme::accent()),
        Span::styled(" bundle  ", theme::dim()),
        Span::styled("c", theme::accent()),
        Span::styled(" copy  ", theme::dim()),
        Span::styled("d", theme::accent()),
        Span::styled(" delete  ", theme::dim()),
        Span::styled("?", theme::accent()),
        Span::styled(" help  ", theme::dim()),
        Span::styled("q", theme::accent()),
        Span::styled(" quit", theme::dim()),
    ];
    // When the recency cap hid older drafts, surface the count + the
    // exact command to widen the view. Otherwise the user just sees
    // "Drafts · 100" and assumes that's everything they have.
    if state.hidden_count > 0 {
        hints.push(Span::styled("   ", theme::dim()));
        hints.push(Span::styled(
            format!("({} older hidden — --all to view)", state.hidden_count),
            theme::pending(),
        ));
    }
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

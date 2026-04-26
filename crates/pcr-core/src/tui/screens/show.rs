//! `pcr show` — interactive draft browser.
//!
//! Three panes (drafts list, detail, optional sidebar with changed files
//! and tool calls). Selection lives on row index color: blue = focused,
//! green = selected, green wins when both apply. `b` opens a bundle
//! name prompt for the current selection (or the focused row).
//! After a successful bundle, `p` exits the TUI and runs `pcr push`.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
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

/// What the caller should do after the TUI exits.
pub enum ShowOutcome {
    Quit,
    PushAfterExit,
}

pub fn run(drafts: Vec<DraftRecord>) -> Result<ShowOutcome> {
    run_focused(drafts, 0)
}

/// Open the browser focused on `initial_focus` (0-based, clamped).
pub fn run_focused(drafts: Vec<DraftRecord>, initial_focus: usize) -> Result<ShowOutcome> {
    run_focused_with_hidden(drafts, initial_focus, 0)
}

/// Like `run_focused` but advertises `hidden_count` older drafts in the
/// footer so the user knows the recency cap truncated the list.
pub fn run_focused_with_hidden(
    drafts: Vec<DraftRecord>,
    initial_focus: usize,
    hidden_count: usize,
) -> Result<ShowOutcome> {
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
        push_armed: false,
        outcome: ShowOutcome::Quit,
        nav_dir: NavDir::Down,
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
                // Tick down the flash banner; expiring it also disarms
                // the post-bundle `p` shortcut so it can't fire silently
                // long after the confirmation has scrolled off.
                if let Some((_, ref mut ttl)) = state.copy_flash {
                    *ttl = ttl.saturating_sub(1);
                    if *ttl == 0 {
                        state.copy_flash = None;
                        state.push_armed = false;
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            _ => {}
        }
    }

    restore_terminal()?;
    Ok(state.outcome)
}

struct ShowState {
    drafts: Vec<DraftRecord>,
    focus: usize,
    list_state: ListState,
    /// `(message, ticks_remaining)` — transient footer banner.
    copy_flash: Option<(String, u32)>,
    /// Drafts hidden by the recency cap before the TUI opened.
    hidden_count: usize,
    /// Draft IDs marked for the next bundle.
    selected: HashSet<String>,
    /// Active modal prompt; freezes list navigation while `Some`.
    prompt: Option<Modal>,
    /// Whether the post-bundle `p` push shortcut is currently live.
    push_armed: bool,
    outcome: ShowOutcome,
    /// Last navigation direction; drives the auto-advance after a
    /// select so it follows the user's scroll instead of always
    /// jumping down.
    nav_dir: NavDir,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NavDir {
    Up,
    Down,
}

/// Inline text-input modal overlaid on the list.
struct Modal {
    kind: ModalKind,
    buf: String,
    /// Draft IDs the modal will act on. Snapshotted at open so the
    /// target set can't shift if focus changes behind the overlay.
    /// Unused by `RangeSelect`, which reads `state.drafts` at confirm.
    targets: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModalKind {
    /// `b`: prompt for a bundle name; on confirm bundle `targets`.
    Bundle,
    /// `:`: prompt for a `1-5,8,12-15` range; on confirm union those
    /// indices into `state.selected`.
    RangeSelect,
}

fn handle_key(k: KeyEvent, state: &mut ShowState) -> bool {
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
            state.nav_dir = NavDir::Down;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.focus = state.focus.saturating_sub(1);
            state.list_state.select(Some(state.focus));
            state.nav_dir = NavDir::Up;
        }
        KeyCode::Home | KeyCode::Char('g') => {
            state.focus = 0;
            state.list_state.select(Some(0));
            state.nav_dir = NavDir::Down;
        }
        KeyCode::End | KeyCode::Char('G') => {
            if !state.drafts.is_empty() {
                state.focus = state.drafts.len() - 1;
                state.list_state.select(Some(state.focus));
            }
            state.nav_dir = NavDir::Up;
        }
        KeyCode::Char('c') => {
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
        KeyCode::Char(' ') | KeyCode::Enter => {
            toggle_focused(state);
            advance_focus(state);
        }
        KeyCode::Char('a') => {
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
        KeyCode::Char('b') => {
            // Multi-select if anything's marked, else just the focused row.
            // Iterate `state.drafts` (not the HashSet) so the bundle's
            // draft sequence reads top-to-bottom on push.
            let targets: Vec<String> = if state.selected.is_empty() {
                match state.drafts.get(state.focus) {
                    Some(d) => vec![d.id.clone()],
                    None => Vec::new(),
                }
            } else {
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
                state.prompt = Some(Modal {
                    kind: ModalKind::Bundle,
                    buf: String::new(),
                    targets,
                });
            }
        }
        KeyCode::Char(':') => {
            if !state.drafts.is_empty() {
                state.prompt = Some(Modal {
                    kind: ModalKind::RangeSelect,
                    buf: String::new(),
                    targets: Vec::new(),
                });
            }
        }
        KeyCode::Char('J') => {
            state.nav_dir = NavDir::Down;
            toggle_focused(state);
            advance_focus(state);
        }
        KeyCode::Char('K') => {
            state.nav_dir = NavDir::Up;
            toggle_focused(state);
            advance_focus(state);
        }
        KeyCode::Char('p') if state.push_armed => {
            // Hand control back to the caller so it can run `pcr push`
            // with the terminal already restored.
            state.outcome = ShowOutcome::PushAfterExit;
            return false;
        }
        KeyCode::Char('d') => delete_focused(state),
        KeyCode::Char('?') => {
            state.copy_flash = Some((
                "j/k move · enter/space select · J/K select+move · : range (1-5,8) · a all · b bundle · p push · c copy · d delete · q quit"
                    .into(),
                14,
            ));
        }
        _ => {}
    }
    true
}

fn toggle_focused(state: &mut ShowState) {
    if let Some(d) = state.drafts.get(state.focus) {
        if !state.selected.insert(d.id.clone()) {
            state.selected.remove(&d.id);
        }
    }
}

/// Delete the focused draft from the store and drop it from the list.
/// No confirmation: the action is local-only and the original session
/// will re-capture the prompt on the next watcher pass if needed.
fn delete_focused(state: &mut ShowState) {
    let Some(d) = state.drafts.get(state.focus).cloned() else {
        return;
    };
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

/// Key dispatch while a modal prompt is on screen. Esc / empty-buf q
/// dismiss the prompt; the TUI itself never quits from inside a modal.
fn handle_prompt_key(k: KeyEvent, state: &mut ShowState) -> bool {
    let Some(prompt) = state.prompt.as_mut() else {
        return true;
    };
    match k.code {
        KeyCode::Esc => {
            let kind = prompt.kind;
            state.prompt = None;
            state.copy_flash = Some((cancel_flash(kind).into(), 3));
        }
        KeyCode::Backspace => {
            prompt.buf.pop();
        }
        KeyCode::Enter => {
            let kind = prompt.kind;
            let buf = prompt.buf.trim().to_string();
            match kind {
                ModalKind::Bundle => confirm_bundle_modal(state, buf),
                ModalKind::RangeSelect => confirm_range_select_modal(state, buf),
            }
        }
        KeyCode::Char(c) => {
            // q only cancels when the buffer is empty; once the user
            // has typed anything it becomes a literal so bundle names
            // containing "q" stay reachable.
            if k.modifiers.contains(KeyModifiers::CONTROL) && (c == 'u' || c == 'U') {
                prompt.buf.clear();
            } else if (c == 'q' || c == 'Q') && prompt.buf.is_empty() {
                let kind = prompt.kind;
                state.prompt = None;
                state.copy_flash = Some((cancel_flash(kind).into(), 3));
            } else if !k.modifiers.contains(KeyModifiers::CONTROL)
                && prompt.buf.chars().count() < 120
            {
                prompt.buf.push(c);
            }
        }
        _ => {}
    }
    true
}

fn cancel_flash(kind: ModalKind) -> &'static str {
    match kind {
        ModalKind::Bundle => "Bundle cancelled.",
        ModalKind::RangeSelect => "Range select cancelled.",
    }
}

fn confirm_bundle_modal(state: &mut ShowState, name: String) {
    if name.is_empty() {
        state.copy_flash = Some(("Bundle name can't be empty — Esc to cancel.".into(), 4));
        return;
    }
    let targets = state
        .prompt
        .as_mut()
        .map(|p| std::mem::take(&mut p.targets))
        .unwrap_or_default();
    state.prompt = None;
    match create_bundle_from_targets(&name, &targets) {
        Ok(count) => {
            // Drop bundled drafts from the in-memory list — the store
            // has already moved them out of the draft pool.
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
            state.push_armed = true;
            state.copy_flash = Some((
                format!(
                    "Bundled {count} draft{} as {name:?} — press p to push, q to quit",
                    plural(count)
                ),
                12,
            ));
        }
        Err(e) => {
            state.copy_flash = Some((format!("Bundle failed: {e}"), 8));
        }
    }
}

fn confirm_range_select_modal(state: &mut ShowState, expr: String) {
    if expr.is_empty() {
        state.copy_flash = Some(("Empty range — Esc to cancel.".into(), 3));
        return;
    }
    let total = state.drafts.len();
    let indices = if expr.eq_ignore_ascii_case("all") {
        (0..total).collect::<Vec<_>>()
    } else {
        crate::util::text::parse_selection_indices(&expr, total)
    };
    state.prompt = None;
    if indices.is_empty() {
        state.copy_flash = Some((
            format!("No drafts matched {expr:?} (valid: 1-{total}, e.g. 1-5,8,12)"),
            6,
        ));
        return;
    }
    let mut added = 0usize;
    for i in &indices {
        if let Some(d) = state.drafts.get(*i) {
            if state.selected.insert(d.id.clone()) {
                added += 1;
            }
        }
    }
    state.copy_flash = Some((
        format!(
            "Selected {added} new draft{} from range {expr:?} (total ✓ {})",
            plural(added),
            state.selected.len()
        ),
        5,
    ));
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Strip / normalize control bytes so each remaining `char` occupies
/// exactly one display column. Without this, tabs and ANSI escapes
/// throw off `Paragraph`'s wrap calculation and lines spill past the
/// right edge as mid-word fragments.
fn sanitize_for_display(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\t' => out.push_str("    "),
            '\u{1b}' => skip_escape_sequence(&mut chars),
            '\r' | '\u{0b}' | '\u{0c}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Consume an ANSI escape sequence after an ESC has already been read.
/// Handles CSI (`ESC [ … final`) and OSC (`ESC ] … BEL` or `ESC \`);
/// any other introducer leaves the next char to be processed normally.
fn skip_escape_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.peek() {
        Some(&'[') => {
            chars.next();
            for p in chars.by_ref() {
                if matches!(p, '\u{40}'..='\u{7e}') {
                    break;
                }
            }
        }
        Some(&']') => {
            chars.next();
            while let Some(p) = chars.next() {
                if p == '\u{07}' {
                    break;
                }
                if p == '\u{1b}' && chars.peek() == Some(&'\\') {
                    chars.next();
                    break;
                }
            }
        }
        _ => {}
    }
}

/// Move focus one row in the last navigation direction, clamped.
fn advance_focus(state: &mut ShowState) {
    if state.drafts.is_empty() {
        return;
    }
    match state.nav_dir {
        NavDir::Down => {
            if state.focus + 1 < state.drafts.len() {
                state.focus += 1;
            }
        }
        NavDir::Up => {
            state.focus = state.focus.saturating_sub(1);
        }
    }
    state.list_state.select(Some(state.focus));
}

/// Create a sealed bundle from `draft_ids_in`, mirroring the
/// `pcr bundle "name" --select` codepath. Returns the number of
/// drafts actually bundled.
fn create_bundle_from_targets(name: &str, draft_ids_in: &[String]) -> Result<usize, String> {
    if draft_ids_in.is_empty() {
        return Err("nothing to bundle".into());
    }
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

    // Defense against `Paragraph` + `Wrap` leaving cells unwritten when
    // the layout reflows between frames; clears the whole frame so
    // ratatui's diff overwrites anything stale.
    frame.render_widget(Clear, area);

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

    // Reflow to a 2-column layout when the sidebar would be empty so
    // the detail pane absorbs those 28 columns.
    let show_sidebar = focused_has_sidebar_content(state);
    let cols = if show_sidebar {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(28), // drafts list
                Constraint::Min(40),    // detail
                Constraint::Length(28), // changed files / tools
            ])
            .split(chunks[1])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(28), // drafts list
                Constraint::Min(40),    // detail (gets the sidebar's columns)
            ])
            .split(chunks[1])
    };

    draw_list(frame, cols[0], state);
    draw_detail(frame, cols[1], state);
    if show_sidebar {
        draw_sidebar(frame, cols[2], state);
    }
    draw_footer(frame, chunks[2], state);

    if state.prompt.is_some() {
        draw_name_prompt(frame, chunks[1], state);
    }
}

/// Centered modal overlay for the active prompt.
fn draw_name_prompt(frame: &mut ratatui::Frame, body: Rect, state: &ShowState) {
    let Some(prompt) = state.prompt.as_ref() else {
        return;
    };
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

    let title = match prompt.kind {
        ModalKind::Bundle => format!(
            " Bundle name · {} draft{} ",
            prompt.targets.len(),
            plural(prompt.targets.len())
        ),
        ModalKind::RangeSelect => " Select range · e.g. 1-5,8,12-15 ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::accent())
        .title(Line::from(Span::styled(title, theme::accent_bold())));
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

    let hint = match prompt.kind {
        ModalKind::Bundle => "Name your bundle (e.g. \"auth fix\"):",
        ModalKind::RangeSelect => "Type draft numbers — comma + dash, e.g. 1-5,8,12-15 (or `all`):",
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, theme::dim()))),
        chunks[0],
    );

    // Trailing block cursor instead of terminal cursor positioning so
    // the modal renders identically under tmux / screen.
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
            Span::styled("esc/q", theme::accent()),
            Span::styled(" cancel   ", theme::dim()),
            Span::styled("ctrl-u", theme::accent()),
            Span::styled(" clear", theme::dim()),
        ])),
        chunks[3],
    );
}

fn draw_list(frame: &mut ratatui::Frame, area: Rect, state: &ShowState) {
    // Row format: `NNN preview`. State lives on the index color
    // (focused = blue, selected = green; selection wins when both
    // apply). No mark / pointer column.
    const FIXED_PREFIX: usize = 4; // 3-wide index + 1 separator space
    const MIN_PREVIEW_WIDTH: usize = 8;
    let inner_width = (area.width as usize).saturating_sub(2); // minus borders
    let preview_max = inner_width
        .saturating_sub(FIXED_PREFIX)
        .max(MIN_PREVIEW_WIDTH);

    let items: Vec<ListItem<'_>> = state
        .drafts
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let is_focused = i == state.focus;
            let is_selected = state.selected.contains(&d.id);
            let preview = crate::util::text::prompt_preview(&d.prompt_text, preview_max);

            let bold_white = Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD);
            let (idx_style, preview_style) = match (is_selected, is_focused) {
                (true, _) => (
                    Style::default()
                        .fg(theme::SUCCESS)
                        .add_modifier(Modifier::BOLD),
                    bold_white,
                ),
                (false, true) => (
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                    theme::text(),
                ),
                (false, false) => (theme::chrome(), theme::text()),
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!("{:>3}", i + 1), idx_style),
                Span::raw(" "),
                Span::styled(preview, preview_style),
            ]))
        })
        .collect();

    let mut ls = state.list_state.clone();
    // Center the focused row in the viewport, clamped at the ends.
    let visible_rows = (area.height as usize).saturating_sub(2);
    if visible_rows > 0 && state.drafts.len() > visible_rows {
        let half = visible_rows / 2;
        let max_offset = state.drafts.len().saturating_sub(visible_rows);
        let desired = state.focus.saturating_sub(half).min(max_offset);
        *ls.offset_mut() = desired;
    }
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
        lines.push(Line::from(Span::styled(
            sanitize_for_display(line),
            theme::text(),
        )));
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
            lines.push(Line::from(Span::styled(
                sanitize_for_display(line),
                theme::text(),
            )));
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

    // Pre-clear the Paragraph target — see `draw` for context.
    frame.render_widget(Clear, inner);
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, inner);
}

/// Whether the focused draft has anything to put in the right sidebar.
/// Must mirror `draw_sidebar`'s data lookups.
fn focused_has_sidebar_content(state: &ShowState) -> bool {
    let Some(d) = state.drafts.get(state.focus) else {
        return false;
    };
    let has_changed_files = d
        .file_context
        .as_ref()
        .and_then(|m| m.get("changed_files").and_then(|v| v.as_array()))
        .is_some_and(|a| !a.is_empty());
    let has_tool_calls = !crate::display::summarize_tools(&d.tool_calls).is_empty();
    has_changed_files || has_tool_calls
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
        // Cursor doesn't expose tool calls; everything else does, so
        // "no tools used" is only accurate for those sources.
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
        Span::styled("enter", theme::accent()),
        Span::styled(" select  ", theme::dim()),
        Span::styled("J/K", theme::accent()),
        Span::styled(" range  ", theme::dim()),
        Span::styled(":", theme::accent()),
        Span::styled(" 1-5,8  ", theme::dim()),
        Span::styled("a", theme::accent()),
        Span::styled(" all  ", theme::dim()),
        Span::styled("b", theme::accent()),
        Span::styled(" bundle  ", theme::dim()),
        Span::styled("d", theme::accent()),
        Span::styled(" delete  ", theme::dim()),
        Span::styled("?", theme::accent()),
        Span::styled(" help  ", theme::dim()),
        Span::styled("q", theme::accent()),
        Span::styled(" quit", theme::dim()),
    ];
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

/// Best-effort clipboard copy via `pbcopy` / `wl-copy` / `xclip` /
/// `xsel` / `clip.exe`. Returns false when no tool was available.
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

#[cfg(test)]
mod sanitize_tests {
    use super::sanitize_for_display;

    #[test]
    fn tabs_become_four_spaces() {
        assert_eq!(sanitize_for_display("a\tb\tc"), "a    b    c");
    }

    #[test]
    fn ansi_csi_sequences_are_stripped() {
        assert_eq!(sanitize_for_display("\u{1b}[0mhello"), "hello");
        assert_eq!(sanitize_for_display("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(sanitize_for_display("a\u{1b}[10;5Hb"), "ab");
    }

    #[test]
    fn carriage_returns_and_form_feeds_are_stripped() {
        assert_eq!(sanitize_for_display("a\rb\u{0c}c\u{0b}d"), "abcd");
    }

    #[test]
    fn other_control_chars_are_stripped() {
        assert_eq!(sanitize_for_display("a\u{0}b\u{7}c"), "abc");
    }

    #[test]
    fn normal_text_is_unchanged() {
        let input = "Hello, world! — 你好 🚀";
        assert_eq!(sanitize_for_display(input), input);
    }

    #[test]
    fn tab_separated_table_row_has_no_tabs() {
        let input = "GitHub Copilot Reviews\tAI reviews diffs\tPCR puts humans in the loop";
        let out = sanitize_for_display(input);
        assert!(!out.contains('\t'));
        assert!(out.starts_with("GitHub Copilot Reviews    "));
        assert!(out.ends_with("PCR puts humans in the loop"));
    }
}

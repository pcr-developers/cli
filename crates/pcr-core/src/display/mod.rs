//! Colored line-based output **and** structured event broadcasting.
//!
//! The display module has two output paths:
//!
//! 1. **Line mode (default)** — every `print_*` writes to stderr. Output is
//!    byte-compatible with the Go CLI when colors are enabled, and downgrades
//!    cleanly when stdout/stderr isn't a TTY, `NO_COLOR` is set, etc. This is
//!    what scripts, CI, and agentic IDEs see.
//!
//! 2. **TUI mode** — `pcr start` (and other ratatui commands) calls
//!    [`install_sink`] before entering the alternate screen. Once a sink is
//!    installed, every `print_*` routes its content through that
//!    [`mpsc::Sender<DisplayEvent>`] **instead of** stderr. The TUI's main
//!    loop drains the channel and renders events into widgets without any
//!    bytes ever reaching the terminal directly.
//!
//! This split is what stops watcher threads from corrupting the alternate
//! screen — pre-refactor, every `display::print_watcher_ready` call from a
//! background thread interleaved raw ANSI into ratatui's framebuffer.

pub mod events;
pub mod sink;

pub use events::{DisplayEvent, SourceState};
pub use sink::{install_sink, sink_active, take_sink, with_sink};

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::agent;
use crate::util::time::local_hms;

/// Global "print verbose watcher events" flag. Set by `pcr start --verbose`.
static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const MAGENTA: &str = "\x1b[35m";
const GRAY: &str = "\x1b[90m";

fn c(code: &str, text: &str) -> String {
    if agent::colors_enabled(agent::OutputMode::Auto) {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

// ─── Line-mode writers ───────────────────────────────────────────────────────
//
// Each writer first checks for an installed sink; if one is present the
// content goes through the structured [`DisplayEvent`] path, otherwise it
// falls through to stderr (the historical Go-compatible output).

/// Startup banner. Matches `display.PrintStartupBanner` in line mode; when a
/// sink is active it sends a [`DisplayEvent::Banner`] so the TUI can render
/// it inside its own header instead of leaking ANSI into the alt screen.
pub fn print_startup_banner(version: &str, build_time: &str, project_count: usize) {
    if with_sink(|tx| {
        let _ = tx.send(DisplayEvent::Banner {
            version: version.to_string(),
            build_time: build_time.to_string(),
            project_count,
        });
    }) {
        return;
    }

    let w = 56;
    let line: String = "─".repeat(w);
    let mut version_str = format!("v{version}");
    if !build_time.is_empty() {
        version_str.push_str(&format!(" built {build_time}"));
    }
    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(
        err,
        "\n{} {} — live capture stream",
        c(&format!("{CYAN}{BOLD}"), "PCR.dev"),
        c(GRAY, &version_str)
    );
    let _ = writeln!(err, "{}", c(GRAY, &line));
    if project_count == 0 {
        let _ = writeln!(
            err,
            "{}{}",
            c(YELLOW, "  ⚠  No projects registered."),
            c(GRAY, " Run `pcr init` in a project directory.")
        );
    } else {
        let plural = if project_count == 1 { "" } else { "s" };
        let msg = format!(
            "  Watching {project_count} project{plural} — new exchanges appear below as they happen."
        );
        let _ = writeln!(err, "{}", c(GRAY, &msg));
    }
    let _ = writeln!(err, "{}", c(GRAY, &line));
    let _ = writeln!(err);
}

#[derive(Debug, Default)]
pub struct CaptureDisplayOptions<'a> {
    pub project_name: &'a str,
    pub session_id: &'a str,
    pub branch: &'a str,
    pub model: &'a str,
    pub prompt_text: &'a str,
    pub tool_calls: &'a [serde_json::Value],
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub exchange_count: u64,
    pub project_url: &'a str,
}

/// Count tools by name. Matches `display.SummarizeTools`.
pub fn summarize_tools(tool_calls: &[serde_json::Value]) -> String {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for tc in tool_calls {
        let name = tc
            .get("tool")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        *counts.entry(name).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .map(|(name, cnt)| {
            if cnt > 1 {
                format!("{name}×{cnt}")
            } else {
                name
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn shrink_preview(text: &str, n: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= n {
        return text.to_string();
    }
    let take = n.saturating_sub(1);
    let mut s: String = chars.iter().take(take).collect();
    s = s.trim_end().to_string();
    s.push('…');
    s
}

/// Print a successfully-captured exchange. Routes to the sink in TUI mode,
/// otherwise renders the canonical Go-compatible block to stderr.
pub fn print_captured(opts: &CaptureDisplayOptions<'_>) {
    if with_sink(|tx| {
        let _ = tx.send(DisplayEvent::Captured {
            project_name: opts.project_name.to_string(),
            branch: opts.branch.to_string(),
            model: opts.model.to_string(),
            prompt_text: opts.prompt_text.to_string(),
            tool_summary: summarize_tools(opts.tool_calls),
            input_tokens: opts.input_tokens,
            output_tokens: opts.output_tokens,
            exchange_count: opts.exchange_count,
            project_url: opts.project_url.to_string(),
            timestamp: local_hms(),
        });
    }) {
        return;
    }

    let ts = local_hms();
    let branch_str = if opts.branch.is_empty() {
        String::new()
    } else {
        c(GRAY, &format!(" [{}]", opts.branch))
    };
    let model_str = if opts.model.is_empty() {
        String::new()
    } else {
        c(GRAY, &format!("  {}", opts.model))
    };
    let header = format!(
        "  {}{}{}  {}",
        c(BOLD, opts.project_name),
        branch_str,
        model_str,
        c(GRAY, &ts)
    );

    let preview = shrink_preview(opts.prompt_text, 80);
    let prompt_line = format!(
        "  {} {}\n",
        c(CYAN, "❯"),
        c(BOLD, &format!("\"{preview}\""))
    );

    let tool_line = if opts.tool_calls.is_empty() {
        String::new()
    } else {
        format!("    {}\n", c(MAGENTA, &summarize_tools(opts.tool_calls)))
    };

    let mut token_line = String::new();
    if opts.input_tokens > 0 || opts.output_tokens > 0 {
        let mut parts = Vec::new();
        if opts.input_tokens > 0 {
            parts.push(format!("{} in", opts.input_tokens));
        }
        if opts.output_tokens > 0 {
            parts.push(format!("{} out", opts.output_tokens));
        }
        token_line = format!(
            "    {}\n",
            c(GRAY, &format!("tokens: {}", parts.join(" · ")))
        );
    }

    let sync_msg = if opts.exchange_count == 1 {
        "1 exchange synced".to_string()
    } else {
        format!("{} exchanges synced", opts.exchange_count)
    };
    let url_part = if opts.project_url.is_empty() {
        String::new()
    } else {
        c(GRAY, &format!("  →  {}", opts.project_url))
    };
    let sync_line = format!("  {} {}{}\n", c(GREEN, "✓"), c(GRAY, &sync_msg), url_part);

    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(err, "{header}");
    let _ = write!(err, "{prompt_line}{tool_line}{token_line}{sync_line}");
    let _ = writeln!(err);
}

#[derive(Debug, Default)]
pub struct DraftDisplayOptions<'a> {
    pub project_name: &'a str,
    pub branch: &'a str,
    pub prompt_text: &'a str,
    pub exchange_count: u64,
}

/// Print a locally-saved draft.
pub fn print_drafted(opts: &DraftDisplayOptions<'_>) {
    if with_sink(|tx| {
        let _ = tx.send(DisplayEvent::Drafted {
            project_name: opts.project_name.to_string(),
            branch: opts.branch.to_string(),
            prompt_text: opts.prompt_text.to_string(),
            exchange_count: opts.exchange_count,
            timestamp: local_hms(),
        });
    }) {
        return;
    }

    let ts = local_hms();
    let branch_str = if opts.branch.is_empty() {
        String::new()
    } else {
        c(GRAY, &format!(" [{}]", opts.branch))
    };
    let header = format!(
        "  {}{}  {}",
        c(BOLD, opts.project_name),
        branch_str,
        c(GRAY, &ts)
    );
    let preview = shrink_preview(opts.prompt_text, 80);
    let prompt_line = format!(
        "  {} {}\n",
        c(CYAN, "❯"),
        c(BOLD, &format!("\"{preview}\""))
    );
    let count = if opts.exchange_count == 1 {
        "1 exchange".to_string()
    } else {
        format!("{} exchanges", opts.exchange_count)
    };
    let hint = format!("{count} saved locally — run 'pcr bundle \"name\" --select all' to bundle");
    let draft_line = format!("  {} {}\n", c(YELLOW, "◎"), c(GRAY, &hint));

    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(err, "{header}");
    let _ = write!(err, "{prompt_line}{draft_line}");
    let _ = writeln!(err);
}

/// Announce that a watcher is up and watching `dir`. In TUI mode this becomes
/// a [`SourceState::Ready`] update so the dashboard's watcher table reflects
/// real state instead of a hardcoded "ready" string.
pub fn print_watcher_ready(source_name: &str, dir: &str) {
    if with_sink(|tx| {
        let _ = tx.send(DisplayEvent::SourceState {
            source: source_name.to_string(),
            state: SourceState::Ready {
                dir: dir.to_string(),
            },
        });
    }) {
        return;
    }

    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(
        err,
        "  {}  {}",
        c(GRAY, &format!("◎  {source_name}")),
        c(DIM, dir)
    );
}

/// Announce that a watcher is initializing. Has no line-mode counterpart by
/// design — the legacy CLI didn't emit anything here, and we don't want to
/// introduce noise in plain mode.
pub fn print_watcher_initializing(source_name: &str) {
    let _ = with_sink(|tx| {
        let _ = tx.send(DisplayEvent::SourceState {
            source: source_name.to_string(),
            state: SourceState::Initializing,
        });
    });
}

/// Announce that a watcher discovered its target directory is missing. In
/// line mode we still emit the historical warning via [`print_error`].
pub fn print_watcher_missing(source_name: &str, dir: &str) {
    if with_sink(|tx| {
        let _ = tx.send(DisplayEvent::SourceState {
            source: source_name.to_string(),
            state: SourceState::Missing {
                dir: dir.to_string(),
            },
        });
    }) {
        return;
    }
    print_error(
        source_name,
        &format!("Directory not found: {dir}. Will activate when it appears."),
    );
}

pub fn print_verbose_event(source: &str, msg: &str) {
    if !is_verbose() {
        return;
    }

    let ts = local_hms();
    if with_sink(|tx| {
        let _ = tx.send(DisplayEvent::Verbose {
            source: source.to_string(),
            msg: msg.to_string(),
            timestamp: ts.clone(),
        });
    }) {
        return;
    }

    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(
        err,
        "  {}  {}  {}",
        c(GRAY, &ts),
        c(DIM, &format!("~  {source}")),
        c(DIM, msg)
    );
}

pub fn print_error(context: &str, msg: &str) {
    if with_sink(|tx| {
        let _ = tx.send(DisplayEvent::Error {
            context: context.to_string(),
            msg: msg.to_string(),
            timestamp: local_hms(),
        });
    }) {
        return;
    }

    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(err, "  {} {}", c(YELLOW, &format!("⚠  {context}:")), msg);
}

/// Append a "next action" hint after an error or empty-state message.
/// Phase 3 wires this into every command so users always know what to type.
pub fn print_hint(msg: &str) {
    if with_sink(|tx| {
        let _ = tx.send(DisplayEvent::Hint {
            msg: msg.to_string(),
        });
    }) {
        return;
    }

    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(err, "  {} {}", c(CYAN, "→"), c(DIM, msg));
}

pub fn eprintln(msg: &str) {
    if with_sink(|tx| {
        let _ = tx.send(DisplayEvent::Line {
            msg: msg.to_string(),
        });
    }) {
        return;
    }

    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(err, "{msg}");
}

pub fn eprint(msg: &str) {
    // Bare prompts (no newline) are interactive — they only make sense in
    // line mode. We never route these through the sink.
    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = write!(err, "{msg}");
    let _ = err.flush();
}

pub fn cstr(code_kind: Color, text: &str) -> String {
    let code = match code_kind {
        Color::Reset => RESET,
        Color::Bold => BOLD,
        Color::Dim => DIM,
        Color::Cyan => CYAN,
        Color::Green => GREEN,
        Color::Yellow => YELLOW,
        Color::Red => RED,
        Color::Magenta => MAGENTA,
        Color::Gray => GRAY,
    };
    c(code, text)
}

#[derive(Copy, Clone, Debug)]
pub enum Color {
    Reset,
    Bold,
    Dim,
    Cyan,
    Green,
    Yellow,
    Red,
    Magenta,
    Gray,
}

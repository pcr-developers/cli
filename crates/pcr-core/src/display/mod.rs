//! Colored line-based output. Mirrors `cli/internal/display/display.go`
//! byte-for-byte when `agent::colors_enabled(..)` is true — this is the
//! "plain" output path that agents, scripts, and CI see.
//!
//! All output goes to stderr so it never interferes with MCP stdio.

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
const MAGENTA: &str = "\x1b[35m";
const GRAY: &str = "\x1b[90m";

fn c(code: &str, text: &str) -> String {
    if agent::colors_enabled(agent::OutputMode::Auto) {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

/// Startup banner. Matches `display.PrintStartupBanner`.
pub fn print_startup_banner(version: &str, build_time: &str, project_count: usize) {
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

/// Print a successfully-captured exchange. Matches `display.PrintCaptured`.
pub fn print_captured(opts: &CaptureDisplayOptions<'_>) {
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

/// Print a locally-saved draft. Matches `display.PrintDrafted`.
pub fn print_drafted(opts: &DraftDisplayOptions<'_>) {
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

pub fn print_watcher_ready(source_name: &str, dir: &str) {
    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(
        err,
        "  {}  {}",
        c(GRAY, &format!("◎  {source_name}")),
        c(DIM, dir)
    );
}

pub fn print_verbose_event(source: &str, msg: &str) {
    if !is_verbose() {
        return;
    }
    let ts = local_hms();
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
    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(err, "  {} {}", c(YELLOW, &format!("⚠  {context}:")), msg);
}

pub fn eprintln(msg: &str) {
    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(err, "{msg}");
}

pub fn eprint(msg: &str) {
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
    Magenta,
    Gray,
}

//! Agent / TTY detection used to decide whether to render ratatui, emit
//! ANSI colors, or prompt interactively. Shared by every command so behavior
//! is consistent across the CLI.

use std::env;
use std::io::{self, IsTerminal};

/// Agent-friendly output mode requested on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputMode {
    /// Default: colored line output (byte-compatible with the Go CLI).
    /// May be upgraded to TUI when `[`is_tui_eligible`]` returns true.
    #[default]
    Auto,
    /// Force line output, disable any ratatui TUI.
    Plain,
    /// Machine-readable JSON on stdout (for agents / scripts).
    Json,
}

/// Returns true when stderr is attached to a real terminal. Output is always
/// written to stderr (so MCP stdio isn't interfered with), so terminal
/// detection mirrors that choice.
pub fn stderr_is_terminal() -> bool {
    io::stderr().is_terminal()
}

/// Returns true when stdin is attached to a real terminal.
pub fn stdin_is_terminal() -> bool {
    io::stdin().is_terminal()
}

/// Mirrors the Go helper `isInteractiveTerminal()` from `cli/cmd/helpers.go`.
/// A real interactive terminal means:
/// - `CURSOR_AGENT=1` is not set (Cursor's agent shell)
/// - `CURSOR_SANDBOX` is unset
/// - `TERM` is not `dumb`
/// - stdin is a TTY
pub fn is_interactive_terminal() -> bool {
    if env::var("CURSOR_AGENT").ok().as_deref() == Some("1") {
        return false;
    }
    if env::var("TERM").ok().as_deref() == Some("dumb") {
        return false;
    }
    if env::var("CURSOR_SANDBOX")
        .ok()
        .is_some_and(|v| !v.is_empty())
    {
        return false;
    }
    stdin_is_terminal()
}

/// Returns true when we should render ratatui full-screen UIs. This is the
/// one opt-in that separates "fancy TUI" from "line output":
///
/// - Any explicit `--plain` or `--json` flag disables TUI.
/// - `NO_COLOR` env disables TUI.
/// - `CI` env disables TUI.
/// - Non-TTY stderr disables TUI.
pub fn is_tui_eligible(mode: OutputMode) -> bool {
    if matches!(mode, OutputMode::Plain | OutputMode::Json) {
        return false;
    }
    if env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if env::var_os("CI").is_some() {
        return false;
    }
    if env::var("TERM").ok().as_deref() == Some("dumb") {
        return false;
    }
    stderr_is_terminal()
}

/// Whether to emit ANSI color escape sequences in line-mode output.
pub fn colors_enabled(mode: OutputMode) -> bool {
    if matches!(mode, OutputMode::Json) {
        return false;
    }
    if env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if env::var("TERM").ok().as_deref() == Some("dumb") {
        return false;
    }
    if env::var_os("FORCE_COLOR").is_some() {
        return true;
    }
    stderr_is_terminal()
}

/// Canonical tagging: are we running inside an agentic IDE shell? Useful
/// for commands that want to switch to non-interactive branches even when
/// stdin happens to look like a TTY.
pub fn is_agent_shell() -> bool {
    env::var("CURSOR_AGENT").ok().as_deref() == Some("1")
        || env::var("CLAUDECODE").ok().as_deref() == Some("1")
        || env::var("WINDSURF_AGENT").ok().as_deref() == Some("1")
        || env::var("ZED_AGENT").ok().as_deref() == Some("1")
}

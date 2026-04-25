//! `pcr help` — interactive command browser.
//!
//! Uses the ratatui [`crate::tui::screens::help`] screen when stderr is a
//! TTY; falls back to a plain-text dump of the help table when not (CI,
//! pipes, `--plain`, etc.).

use crate::agent::{self, OutputMode};
use crate::display;
use crate::exit::ExitCode;
use crate::help::HELP;

pub fn run(mode: OutputMode) -> ExitCode {
    if agent::is_tui_eligible(mode) {
        if let Err(e) = crate::tui::screens::help::run() {
            display::print_error("help", &e.to_string());
            return ExitCode::GenericError;
        }
        return ExitCode::Success;
    }
    // Line mode — print every entry one after the other.
    for entry in HELP {
        display::eprintln(&format!(
            "\n  pcr {}  —  {}\n  ────────────────────────────────────────────",
            entry.command, entry.short
        ));
        display::eprintln(&crate::help::render_plain(entry));
    }
    ExitCode::Success
}

//! `pcr help` — interactive command browser.
//!
//! TUI mode: ratatui [`crate::tui::screens::help`] screen with a command
//! list on the left and the formatted help entry on the right. Pressing
//! `Enter` on a command exits the help screen and dispatches to that
//! command — so `help → ↓ ↓ ↓ → Enter` is the same as `pcr status`.
//!
//! Plain / agent mode: dump every entry to stderr in line form. No Enter
//! affordance — there's no terminal to capture the keypress.

use crate::agent::{self, OutputMode};
use crate::commands;
use crate::display;
use crate::entry::{BundleArgs, GcArgs, InitArgs, PullArgs, StartArgs};
use crate::exit::ExitCode;
use crate::help::{self, Runnable, HELP};
use crate::tui::screens::help::Selection;

pub fn run(mode: OutputMode) -> ExitCode {
    if agent::is_tui_eligible(mode) {
        match crate::tui::screens::help::run() {
            Ok(Selection::Quit) => ExitCode::Success,
            Ok(Selection::Run(name)) => dispatch(mode, name),
            Err(e) => {
                display::print_error("help", &e.to_string());
                ExitCode::GenericError
            }
        }
    } else {
        // Line mode — every entry, one after another.
        for entry in HELP {
            display::eprintln(&format!(
                "\n  pcr {}  —  {}\n  ────────────────────────────────────────────",
                entry.command, entry.short
            ));
            display::eprintln(&help::render_plain(entry));
        }
        ExitCode::Success
    }
}

/// User pressed `Enter` on `name` inside the help TUI. Map it to a real
/// command invocation. Direct commands launch in-process; commands that
/// need positional args print the first example so the user can edit
/// it in their shell history.
fn dispatch(mode: OutputMode, name: &'static str) -> ExitCode {
    let entry = match help::entry(name) {
        Some(e) => e,
        // Should be impossible — the screen only emits commands from
        // the help table — but fail safely instead of unwrapping.
        None => return ExitCode::Success,
    };

    match entry.runnable {
        Runnable::Hidden => ExitCode::Success,
        Runnable::NeedsArgs => {
            // Show the first example to stderr so the user can copy it
            // out of their scrollback (or just hit ↑ in their shell).
            display::eprintln("");
            display::print_hint(&format!(
                "`pcr {}` needs an argument. Try one of:",
                entry.command
            ));
            for (cmd, desc) in entry.examples {
                display::eprintln(&format!("    $ {cmd}"));
                display::eprintln(&format!("        {desc}"));
            }
            ExitCode::Success
        }
        Runnable::Direct => match name {
            "login" => commands::login::run(mode),
            "logout" => commands::logout::run(mode),
            "init" => commands::init::run(mode, InitArgs::default()),
            "start" => commands::start::run(mode, StartArgs::default()),
            "mcp" => crate::mcp::run_stub(),
            "status" => commands::status::run(mode),
            "bundle" => commands::bundle::run(mode, BundleArgs::default()),
            "push" => commands::push::run(mode),
            "log" => commands::log::run(mode),
            "pull" => commands::pull::run(mode, PullArgs::default()),
            "gc" => commands::gc::run(mode, GcArgs::default()),
            // `show` is the only Direct-mismatch — gated by NeedsArgs above.
            // Any other unknown name falls through to a no-op success.
            _ => ExitCode::Success,
        },
    }
}

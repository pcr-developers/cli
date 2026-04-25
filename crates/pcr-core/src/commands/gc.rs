//! `pcr gc`. Mirrors `cli/cmd/gc.go`.

use std::path::PathBuf;

use crate::agent::OutputMode;
use crate::display;
use crate::entry::GcArgs;
use crate::exit::ExitCode;
use crate::sources::shared::git;
use crate::store;
use crate::util::text::plural;

pub fn run(_mode: OutputMode, args: GcArgs) -> ExitCode {
    if args.orphaned {
        let mut project_path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .to_string_lossy()
            .into_owned();
        let git_root = git::git_output(&["rev-parse", "--show-toplevel"]);
        if !git_root.is_empty() {
            project_path = git_root;
        }
        match store::gc_orphaned(std::path::Path::new(&project_path)) {
            Ok(0) => display::eprintln("PCR: No orphaned prompt bundles found."),
            Ok(n) => display::eprintln(&format!(
                "PCR: Deleted {n} orphaned prompt bundle{} (drafts restored).",
                plural(n as usize)
            )),
            Err(e) => {
                display::print_error("gc", &e.to_string());
                return ExitCode::GenericError;
            }
        }
        return ExitCode::Success;
    }

    if args.unpushed {
        match store::gc_unpushed() {
            Ok(0) => display::eprintln("PCR: No unpushed prompt bundles to discard."),
            Ok(n) => display::eprintln(&format!(
                "PCR: Discarded {n} unpushed prompt bundle{}.",
                plural(n as usize)
            )),
            Err(e) => {
                display::print_error("gc", &e.to_string());
                return ExitCode::GenericError;
            }
        }
        return ExitCode::Success;
    }

    if args.all_pushed {
        match store::gc_all_pushed() {
            Ok(0) => display::eprintln("PCR: No pushed records to clean up."),
            Ok(n) => display::eprintln(&format!(
                "PCR: Deleted {n} pushed prompt{} from local store.",
                plural(n as usize)
            )),
            Err(e) => {
                display::print_error("gc", &e.to_string());
                return ExitCode::GenericError;
            }
        }
        return ExitCode::Success;
    }

    let days = match args.older_than.as_deref() {
        None | Some("") => 30i64,
        Some(s) => {
            let raw = s.trim_end_matches('d');
            match raw.parse::<i64>() {
                Ok(n) if n > 0 => n,
                _ => {
                    display::print_error("gc", &format!("invalid --older-than value: {s:?}"));
                    display::print_hint("examples:  --older-than 30d   --older-than 7");
                    return ExitCode::Usage;
                }
            }
        }
    };

    match store::gc_pushed(days) {
        Ok(0) => display::eprintln(&format!("PCR: No pushed records older than {days} days.")),
        Ok(n) => display::eprintln(&format!(
            "PCR: Deleted {n} pushed prompt{} older than {days} days.",
            plural(n as usize)
        )),
        Err(e) => {
            display::print_error("gc", &e.to_string());
            return ExitCode::GenericError;
        }
    }
    ExitCode::Success
}

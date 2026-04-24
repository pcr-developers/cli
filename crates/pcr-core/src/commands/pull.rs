//! `pcr pull`. Mirrors `cli/cmd/pull.go`.

use crate::agent::{self, OutputMode};
use crate::auth;
use crate::display;
use crate::entry::PullArgs;
use crate::exit::ExitCode;
use crate::store;
use crate::supabase::{self, PromptRecord};
use crate::util::text::{parse_first_index, plural};

pub fn run(_mode: OutputMode, args: PullArgs) -> ExitCode {
    let Some(a) = auth::load() else {
        display::eprintln("not logged in — run `pcr login`");
        return ExitCode::AuthRequired;
    };

    let mut remote_id = args.remote_id.unwrap_or_default();

    if remote_id.is_empty() {
        let pushed = match store::list_pushed_commits() {
            Ok(v) => v,
            Err(e) => {
                display::print_error("pull", &e.to_string());
                return ExitCode::GenericError;
            }
        };
        if pushed.is_empty() {
            display::eprintln("PCR: No pushed prompt bundles found.");
            return ExitCode::Success;
        }
        display::eprintln("Pushed prompt bundles:\n");
        for (i, b) in pushed.iter().enumerate() {
            display::eprintln(&format!(
                "  [{}] {:?}  remote: {}",
                i + 1,
                b.message,
                b.remote_id
            ));
        }
        display::eprintln("");
        if !agent::is_interactive_terminal() {
            display::eprintln("PCR: Interactive mode not available in this terminal.");
            display::eprintln("     Use flags: pcr pull <bundle-id>");
            return ExitCode::InteractiveUnavailable;
        }
        display::eprint("Select bundle to pull [number]: ");
        let mut buf = String::new();
        if std::io::stdin().read_line(&mut buf).is_err() {
            display::eprintln("PCR: Nothing pulled.");
            return ExitCode::Success;
        }
        let Some(idx) = parse_first_index(buf.trim(), pushed.len()) else {
            display::eprintln("PCR: Nothing pulled.");
            return ExitCode::Success;
        };
        remote_id = pushed[idx].remote_id.clone();
    }

    if remote_id.is_empty() {
        display::print_error("pull", "no remote ID");
        return ExitCode::Usage;
    }

    let bundle = match supabase::pull_bundle(&a.token, &remote_id) {
        Ok(v) => v,
        Err(e) => {
            display::print_error("pull", &format!("failed to fetch bundle: {e}"));
            return ExitCode::Network;
        }
    };
    let items_raw = bundle
        .get("items")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let items: Vec<PromptRecord> = match serde_json::from_value(items_raw) {
        Ok(v) => v,
        Err(e) => {
            display::print_error("pull", &format!("failed to parse bundle items: {e}"));
            return ExitCode::GenericError;
        }
    };

    let mut restored = 0usize;
    for item in items {
        if store::save_draft(&item, &[], "", "").is_ok() {
            restored += 1;
        }
    }
    display::eprintln(&format!(
        "PCR: Restored {} prompt{} from prompt bundle {}",
        restored,
        plural(restored),
        remote_id
    ));
    ExitCode::Success
}

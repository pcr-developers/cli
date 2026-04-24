//! `pcr status`. Mirrors `cli/cmd/status.go`.

use crate::agent::{self, OutputMode};
use crate::auth;
use crate::display::{self, Color};
use crate::exit::ExitCode;
use crate::projects;
use crate::store;
use crate::util::text::plural;

pub fn run(mode: OutputMode) -> ExitCode {
    if matches!(mode, OutputMode::Json) {
        return run_json();
    }
    if agent::is_tui_eligible(mode) {
        if let Err(e) = crate::tui::screens::status::run() {
            display::print_error("status", &e.to_string());
            return ExitCode::GenericError;
        }
        return ExitCode::Success;
    }
    run_plain()
}

fn run_plain() -> ExitCode {
    let a = auth::load();
    if let Some(a) = &a {
        display::eprintln(&format!(
            "{} Logged in (user: {})",
            display::cstr(Color::Green, "✓"),
            a.user_id
        ));
    } else {
        display::eprintln("Not logged in — run `pcr login`");
    }

    let projs = projects::load();
    display::eprintln("");
    if projs.is_empty() {
        display::eprintln("No projects registered. Run `pcr init` in a project directory.");
    } else {
        display::eprintln(&display::cstr(Color::Bold, "Projects"));
        for p in &projs {
            let remote = if p.project_id.is_empty() {
                String::new()
            } else {
                format!(
                    "  {}",
                    display::cstr(Color::Dim, &format!("[remote: {}]", p.project_id))
                )
            };
            display::eprintln(&format!("  {}{}", p.name, remote));
            display::eprintln(&format!("  {}", display::cstr(Color::Dim, &p.path)));
        }
    }

    if let Ok(unpushed) = store::get_unpushed_commits() {
        display::eprintln("");
        if unpushed.is_empty() {
            display::eprintln(&format!(
                "{}  none — everything pushed",
                display::cstr(Color::Bold, "Bundles")
            ));
        } else {
            display::eprintln(&display::cstr(Color::Bold, "Bundles"));
            for b in &unpushed {
                let full = store::get_commit_with_items(&b.id).ok().flatten();
                let count = full.map(|c| c.items.len()).unwrap_or(0);
                if b.bundle_status == "open" {
                    display::eprintln(&format!(
                        "  {}  {}  {}",
                        display::cstr(Color::Yellow, "●"),
                        b.message,
                        display::cstr(Color::Dim, &format!("({count} prompt{})", plural(count)))
                    ));
                } else {
                    display::eprintln(&format!(
                        "  {}  {}  {}",
                        display::cstr(Color::Green, "✓"),
                        b.message,
                        display::cstr(
                            Color::Dim,
                            &format!("({count} prompt{} — sealed, ready to push)", plural(count))
                        )
                    ));
                }
            }
        }
    }

    if let Ok(drafts) = store::get_drafts_by_status(store::DraftStatus::Draft, &[], &[]) {
        display::eprintln("");
        if drafts.is_empty() {
            display::eprintln(&format!("{}  none", display::cstr(Color::Bold, "Drafts")));
        } else {
            display::eprintln(&format!(
                "{}  {} unreviewed — run `pcr bundle` to create a prompt bundle",
                display::cstr(Color::Bold, "Drafts"),
                drafts.len()
            ));
        }
    }

    ExitCode::Success
}

fn run_json() -> ExitCode {
    let a = auth::load();
    let projs = projects::load();
    let unpushed = store::get_unpushed_commits().unwrap_or_default();
    let drafts =
        store::get_drafts_by_status(store::DraftStatus::Draft, &[], &[]).unwrap_or_default();
    let out = serde_json::json!({
        "logged_in": a.is_some(),
        "user_id": a.map(|a| a.user_id).unwrap_or_default(),
        "projects": projs,
        "unpushed_bundles": unpushed,
        "draft_count": drafts.len(),
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    ExitCode::Success
}

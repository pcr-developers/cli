//! `pcr log`. Mirrors `cli/cmd/log.go`.

use crate::agent::OutputMode;
use crate::commands::project_context::resolve;
use crate::display::{self, Color};
use crate::exit::ExitCode;
use crate::sources::shared::git;
use crate::store;
use crate::util::text::{plural, prompt_preview};
use crate::util::time::fmt_time;

pub fn run(mode: OutputMode) -> ExitCode {
    if matches!(mode, OutputMode::Json) {
        return run_json();
    }
    run_plain()
}

fn short_sha(sha: &str) -> String {
    if sha.starts_with("manual-") {
        return "[manual]".to_string();
    }
    if sha.len() >= 7 {
        return sha[..7].to_string();
    }
    sha.to_string()
}

fn run_plain() -> ExitCode {
    let ctx = resolve();
    let pushed = match store::list_commits(Some(true), &ctx.ids, &ctx.names) {
        Ok(v) => v,
        Err(e) => {
            display::print_error("log", &e.to_string());
            return ExitCode::GenericError;
        }
    };
    let unpushed = match store::list_commits(Some(false), &ctx.ids, &ctx.names) {
        Ok(v) => v,
        Err(e) => {
            display::print_error("log", &e.to_string());
            return ExitCode::GenericError;
        }
    };

    let (mut open_bundles, mut sealed_bundles) = (Vec::new(), Vec::new());
    for c in &unpushed {
        if c.bundle_status == "open" {
            open_bundles.push(c.clone());
        } else {
            sealed_bundles.push(c.clone());
        }
    }

    let drafts = store::get_drafts_by_status(store::DraftStatus::Draft, &ctx.ids, &ctx.names)
        .unwrap_or_default();

    let branch = git::git_output(&["rev-parse", "--abbrev-ref", "HEAD"]);
    if !ctx.name.is_empty() {
        if !branch.is_empty() && branch != "HEAD" {
            display::eprintln(&format!(
                "\n{}  {}",
                display::cstr(Color::Bold, &format!("\x1b[36m{}", ctx.name)),
                display::cstr(Color::Gray, &format!("({branch})"))
            ));
        } else {
            display::eprintln(&format!("\n{}", display::cstr(Color::Bold, &ctx.name)));
        }
        if ctx.ids.is_empty() {
            display::eprintln(&display::cstr(
                Color::Gray,
                "  (run `pcr init` to link remotely)",
            ));
        }
    }

    if pushed.is_empty() && unpushed.is_empty() && drafts.is_empty() {
        if ctx.name.is_empty() {
            display::eprintln(
                "PCR: Nothing to show — no project is registered for this directory.",
            );
            display::print_hint("run `pcr init` to register the current git repo");
        } else {
            display::eprintln("PCR: Nothing to show for this project yet.");
            display::print_hint("run `pcr start` and send a prompt in your editor to capture one");
        }
        return ExitCode::Success;
    }

    if !pushed.is_empty() {
        display::eprintln(&format!(
            "\n{}  ({})",
            display::cstr(Color::Green, "PUSHED"),
            pushed.len()
        ));
        for c in &pushed {
            display::eprintln(&format!(
                "  {} {}  {}",
                display::cstr(Color::Green, "✓"),
                short_sha(&c.head_sha),
                display::cstr(Color::Bold, &c.message)
            ));
            display::eprintln(&format!(
                "    {}",
                display::cstr(Color::Gray, &fmt_time(&c.pushed_at))
            ));
        }
    }

    if !open_bundles.is_empty() {
        display::eprintln(&format!(
            "\n{}  ({})",
            display::cstr(Color::Cyan, "OPEN BUNDLES"),
            open_bundles.len()
        ));
        for c in &open_bundles {
            let count = store::get_commit_with_items(&c.id)
                .ok()
                .flatten()
                .map(|f| f.items.len())
                .unwrap_or(0);
            display::eprintln(&format!(
                "  {} {}  {}  {}",
                display::cstr(Color::Cyan, "~"),
                short_sha(&c.head_sha),
                display::cstr(Color::Bold, &c.message),
                display::cstr(Color::Gray, &format!("({count} prompt{})", plural(count))),
            ));
        }
        display::eprintln("\n  Run `pcr bundle --list` to review · `pcr push` to push");
    }

    if !sealed_bundles.is_empty() {
        display::eprintln(&format!(
            "\n{}  ({})",
            display::cstr(Color::Yellow, "SEALED — ready to push"),
            sealed_bundles.len()
        ));
        for c in &sealed_bundles {
            let count = store::get_commit_with_items(&c.id)
                .ok()
                .flatten()
                .map(|f| f.items.len())
                .unwrap_or(0);
            display::eprintln(&format!(
                "  {} {}  {}  {}",
                display::cstr(Color::Yellow, "⊙"),
                short_sha(&c.head_sha),
                display::cstr(Color::Bold, &c.message),
                display::cstr(Color::Gray, &format!("({count} prompt{})", plural(count))),
            ));
        }
        display::eprintln("\n  Run `pcr push` to upload these to PCR.dev");
    }

    if !drafts.is_empty() {
        display::eprintln(&format!(
            "\n{}  ({})",
            display::cstr(Color::Cyan, "DRAFTS — not yet bundled"),
            drafts.len()
        ));
        for d in &drafts {
            let preview = prompt_preview(&d.prompt_text, 65);
            let date = d.captured_at.split('T').next().unwrap_or("");
            display::eprintln(&format!(
                "  {} {}  {}",
                display::cstr(Color::Gray, "◦"),
                display::cstr(Color::Bold, &preview),
                display::cstr(Color::Dim, date),
            ));
        }
        display::eprintln("\n  Run `pcr bundle` to create a prompt bundle");
    }

    display::eprintln("");
    ExitCode::Success
}

fn run_json() -> ExitCode {
    let ctx = resolve();
    let pushed = store::list_commits(Some(true), &ctx.ids, &ctx.names).unwrap_or_default();
    let unpushed = store::list_commits(Some(false), &ctx.ids, &ctx.names).unwrap_or_default();
    let drafts = store::get_drafts_by_status(store::DraftStatus::Draft, &ctx.ids, &ctx.names)
        .unwrap_or_default();
    let out = serde_json::json!({
        "project_name": ctx.name,
        "pushed": pushed,
        "unpushed": unpushed,
        "drafts": drafts,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    ExitCode::Success
}

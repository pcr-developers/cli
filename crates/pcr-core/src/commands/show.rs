//! `pcr show <n>` — full content of a specific draft. Mirrors `cli/cmd/show.go`.

use std::path::Path;

use crate::agent::{self, OutputMode};
use crate::commands::helpers::{cap_recent_drafts, DEFAULT_RECENT_DRAFTS_CAP};
use crate::commands::project_context::{load_proj_by_id, resolve};
use crate::display::{self, Color};
use crate::entry::ShowArgs;
use crate::exit::ExitCode;
use crate::projects;
use crate::store::{self, DraftRecord};
use crate::util::text::{plural, to_f64};
use crate::util::time::fmt_time;

fn short_file_path(path: &str, proj_by_id: &std::collections::BTreeMap<String, String>) -> String {
    let _ = proj_by_id;
    for p in projects::load() {
        if p.path.is_empty() {
            continue;
        }
        let prefix = format!("{}/", p.path);
        if let Some(rel) = path.strip_prefix(&prefix) {
            return format!("{}/{rel}", p.name);
        }
    }
    let cleaned: String = Path::new(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    let parts: Vec<&str> = cleaned.split('/').collect();
    let tail = if parts.len() > 3 {
        &parts[parts.len() - 3..]
    } else {
        &parts[..]
    };
    tail.join("/")
}

fn draft_cursor_mode(d: &DraftRecord) -> Option<String> {
    let fc = d.file_context.as_ref()?;
    fc.get("cursor_mode")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Mirrors `bundle.rs::filter_with_changed_files`: a passthrough today,
/// kept as a function so any future opt-in "hide agent turns with no
/// edits" filter has an obvious place to live.
fn filter_with_changed_files(drafts: Vec<DraftRecord>) -> Vec<DraftRecord> {
    drafts
}

pub fn run(mode: OutputMode, args: ShowArgs) -> ExitCode {
    let n: usize = match args.number.trim().parse() {
        Ok(n) if n >= 1 => n,
        _ => {
            display::print_error("show", &format!("invalid number {:?}", args.number));
            display::print_hint("draft numbers come from `pcr log` or `pcr bundle`");
            return ExitCode::Usage;
        }
    };

    let ctx = resolve();
    let drafts = store::get_drafts_by_status(store::DraftStatus::Draft, &ctx.ids, &ctx.names)
        .unwrap_or_default();
    let staged = store::get_staged_drafts().unwrap_or_default();
    let mut all = drafts;
    all.extend(staged);
    let all = filter_with_changed_files(all);

    if all.is_empty() {
        display::eprintln("PCR: No draft prompts.");
        display::print_hint("run `pcr start` and send a prompt in your editor to capture one");
        return ExitCode::Success;
    }
    if n > all.len() {
        display::print_error(
            "show",
            &format!(
                "draft #{n} doesn't exist — you have {} draft{} (1–{})",
                all.len(),
                plural(all.len()),
                all.len()
            ),
        );
        return ExitCode::NotFound;
    }

    let d = all[n - 1].clone();

    if matches!(mode, OutputMode::Json) {
        println!("{}", serde_json::to_string_pretty(&d).unwrap_or_default());
        return ExitCode::Success;
    }

    if agent::is_tui_eligible(mode) {
        // Cap the visible list to the most recent N drafts unless the
        // user explicitly asked for the full history with `--all`. The
        // requested `n` is 1-based against the *full* list; if it falls
        // inside the hidden tail we expand to show everything so the
        // requested draft is reachable. This avoids the "I asked for
        // #200 but the list only has 100 items" confusion.
        let total = all.len();
        let (display_drafts, hidden, focus_in_view) = if args.all || n > total {
            (all, 0, n - 1)
        } else {
            let (capped, hidden) = cap_recent_drafts(all, DEFAULT_RECENT_DRAFTS_CAP);
            // `cap_recent_drafts` keeps the newest tail. The requested
            // draft index needs to be re-anchored against the kept slice.
            let focus = if n > hidden { n - 1 - hidden } else { 0 };
            (capped, hidden, focus)
        };
        match crate::tui::screens::show::run_focused_with_hidden(
            display_drafts,
            focus_in_view,
            hidden,
        ) {
            Ok(crate::tui::screens::show::ShowOutcome::PushAfterExit) => {
                return crate::commands::push::run(mode);
            }
            Ok(_) => return ExitCode::Success,
            Err(e) => {
                display::print_error("show", &e.to_string());
                return ExitCode::GenericError;
            }
        }
    }

    let proj_by_id = load_proj_by_id();
    let repo_name = |id: &str| {
        proj_by_id
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    };

    display::eprintln(&format!(
        "\n{}",
        display::cstr(Color::Bold, &format!("[{n}] Draft prompt"))
    ));
    display::eprintln(&display::cstr(
        Color::Gray,
        "─────────────────────────────────────────",
    ));

    let mut meta: Vec<String> = Vec::new();
    if !d.captured_at.is_empty() {
        meta.push(fmt_time(&d.captured_at));
    }
    if !d.source.is_empty() {
        meta.push(d.source.clone());
    }
    if let Some(mode) = draft_cursor_mode(&d) {
        meta.push(mode);
    }
    if d.source == "vscode" {
        if let Some(fc) = &d.file_context {
            if let Some(dur) = fc.get("response_duration_ms") {
                meta.push(format!("{:.1}s", to_f64(dur) / 1000.0));
            }
            if let Some(v) = fc.get("copilot_version") {
                meta.push(format!("copilot:{}", v));
            }
        }
    }
    if !d.model.is_empty() {
        meta.push(d.model.clone());
    }
    if !d.branch_name.is_empty() {
        meta.push(format!("branch:{}", d.branch_name));
    }
    if !meta.is_empty() {
        display::eprintln(&display::cstr(Color::Dim, &meta.join("  ·  ")));
    }

    let touched = d.touched_project_ids();
    if touched.len() > 1 {
        let names: Vec<String> = touched.iter().map(|id| repo_name(id)).collect();
        display::eprintln(&display::cstr(
            Color::Dim,
            &format!("repos: {}", names.join(", ")),
        ));
    } else if !d.project_name.is_empty() {
        display::eprintln(&display::cstr(
            Color::Dim,
            &format!("repo:  {}", d.project_name),
        ));
    }
    display::eprintln("");

    display::eprintln(&display::cstr(Color::Cyan, "PROMPT"));
    display::eprintln(&d.prompt_text);

    if let Some(fc) = &d.file_context {
        if let Some(arr) = fc.get("changed_files").and_then(|v| v.as_array()) {
            if !arr.is_empty() {
                display::eprintln(&format!(
                    "\n{}",
                    display::cstr(Color::Cyan, "CHANGED FILES")
                ));
                for f in arr {
                    let short = short_file_path(&format!("{}", f), &proj_by_id);
                    display::eprintln(&display::cstr(Color::Dim, &format!("  {short}")));
                }
            }
        }
    }

    if !d.response_text.is_empty() {
        display::eprintln(&format!("\n{}", display::cstr(Color::Green, "RESPONSE")));
        let mut resp = d.response_text.clone();
        if resp.chars().count() > 200 {
            let take: String = resp.chars().take(200).collect();
            resp = format!("{take}{}", display::cstr(Color::Dim, "…"));
        }
        display::eprintln(&resp);
    }

    if let Some(fc) = &d.file_context {
        if let Some(arr) = fc.get("relevant_files").and_then(|v| v.as_array()) {
            if !arr.is_empty() {
                display::eprintln(&format!(
                    "\n{}",
                    display::cstr(Color::Gray, "FILES IN CONTEXT")
                ));
                for f in arr {
                    let short = short_file_path(&format!("{}", f), &proj_by_id);
                    display::eprintln(&display::cstr(Color::Dim, &format!("  {short}")));
                }
            }
        }
    }
    display::eprintln("");
    ExitCode::Success
}

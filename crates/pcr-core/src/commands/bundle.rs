//! `pcr bundle`. Mirrors the workflows in `cli/cmd/bundle.go`. The most
//! user-facing flows (`--select`, `--add`, `--remove`, `--delete`,
//! `--list`) are fully ported here. The interactive no-`--select` picker
//! walks the same list-and-prompt flow as the Go version.

use std::collections::BTreeMap;

use crate::agent::{self, OutputMode};
use crate::commands::helpers::{current_branch, draft_ids, parse_selection};
use crate::commands::project_context::{load_proj_by_id, resolve, ProjectContext};
use crate::display::{self, Color};
use crate::entry::BundleArgs;
use crate::exit::ExitCode;
use crate::store::{self, DraftRecord};
use crate::util::id::generate_hex_id;
use crate::util::text::{plural, prompt_preview};
use crate::util::time::format_captured_at;

pub fn run(mode: OutputMode, args: BundleArgs) -> ExitCode {
    let name = args.name.join(" ").trim().to_string();

    if args.list {
        return run_bundle_list();
    }

    if args.delete {
        if name.is_empty() {
            display::print_error(
                "bundle",
                "--delete requires a bundle name: pcr bundle \"name\" --delete",
            );
            return ExitCode::Usage;
        }
        return run_bundle_delete(&name);
    }

    if args.remove {
        if name.is_empty() {
            display::print_error(
                "bundle",
                "--remove requires a bundle name: pcr bundle \"name\" --remove --select 1,2",
            );
            return ExitCode::Usage;
        }
        let Some(sel) = args.select.as_deref() else {
            display::print_error(
                "bundle",
                &format!("--remove requires --select: pcr bundle {name:?} --remove --select 1,2"),
            );
            return ExitCode::Usage;
        };
        return run_bundle_remove(&name, sel);
    }

    if args.add {
        if name.is_empty() {
            display::print_error(
                "bundle",
                "--add requires a bundle name: pcr bundle \"name\" --add --select 1-5",
            );
            return ExitCode::Usage;
        }
        let Some(sel) = args.select.as_deref() else {
            display::print_error(
                "bundle",
                &format!("--add requires --select: pcr bundle {name:?} --add --select 1-5"),
            );
            return ExitCode::Usage;
        };
        return run_bundle_add(&name, sel);
    }

    // `pcr bundle "name"` in a TTY opens the TUI on the Drafts view
    // with the name pre-seeded into the bundle modal. The user picks
    // drafts, presses `b` + enter, and the bundle is sealed without
    // them retyping the name. Plain / JSON modes keep `--select` for
    // CI scripts.
    if !name.is_empty() {
        if agent::is_tui_eligible(mode) {
            return run_bundle_browse_with_name(
                args.repo.as_deref(),
                args.all,
                Some(name.clone()),
                crate::tui::screens::show::InitialView::Drafts,
            );
        }
        if let Some(sel) = args.select.as_deref() {
            return run_bundle_create(&name, sel, args.repo.as_deref());
        }
        return run_bundle_show_hint(&name, args.repo.as_deref());
    }

    // No name, no flags — open the interactive draft browser when we
    // can. Dumping every draft (often hundreds) to stderr is hostile;
    // the TUI scrolls, lets the user inspect each draft alongside its
    // diff, and supports `d` to delete stale ones inline. Plain mode
    // keeps the historical line dump for scripts and agents.
    if agent::is_tui_eligible(mode) {
        return run_bundle_browse_with_name(
            args.repo.as_deref(),
            args.all,
            None,
            crate::tui::screens::show::InitialView::Bundles,
        );
    }
    run_bundle_overview(args.repo.as_deref())
}

fn run_bundle_browse_with_name(
    repo_filter: Option<&str>,
    show_all: bool,
    prefill_bundle_name: Option<String>,
    initial_view: crate::tui::screens::show::InitialView,
) -> ExitCode {
    browse_drafts_full(
        repo_filter,
        show_all,
        None,
        "bundle",
        initial_view,
        prefill_bundle_name,
    )
}

/// Shared TUI entrypoint for `pcr show [n]` and `pcr bundle` (no args).
///
/// `focus_number` is 1-based against the full draft list. If it falls
/// inside the hidden tail of the recency cap, the full list is shown
/// so the requested draft stays reachable. `caller` is the command
/// name used as a prefix in error messages.
pub fn browse_drafts(
    repo_filter: Option<&str>,
    show_all: bool,
    focus_number: Option<usize>,
    caller: &str,
) -> ExitCode {
    browse_drafts_with_view(
        repo_filter,
        show_all,
        focus_number,
        caller,
        crate::tui::screens::show::InitialView::Drafts,
    )
}

pub fn browse_drafts_with_view(
    repo_filter: Option<&str>,
    show_all: bool,
    focus_number: Option<usize>,
    caller: &str,
    initial_view: crate::tui::screens::show::InitialView,
) -> ExitCode {
    browse_drafts_full(repo_filter, show_all, focus_number, caller, initial_view, None)
}

pub fn browse_drafts_full(
    repo_filter: Option<&str>,
    show_all: bool,
    focus_number: Option<usize>,
    caller: &str,
    initial_view: crate::tui::screens::show::InitialView,
    prefill_bundle_name: Option<String>,
) -> ExitCode {
    sync_latest_prompts();
    let ctx = resolve();
    let proj_by_id = load_proj_by_id();
    let repo_filter = repo_filter.unwrap_or("");
    let drafts = match get_available_drafts(&ctx, repo_filter, &proj_by_id) {
        Ok(v) => v,
        Err(e) => {
            display::print_error(caller, &e.to_string());
            return ExitCode::GenericError;
        }
    };
    // No drafts is OK when we're opening on the Bundles view — the
    // user might just want to manage existing bundles.
    if drafts.is_empty() && initial_view == crate::tui::screens::show::InitialView::Drafts {
        display::eprintln("PCR: No draft prompts. Run `pcr start` to capture some.");
        return ExitCode::Success;
    }
    let total = drafts.len();
    if let Some(n) = focus_number {
        if n == 0 || n > total {
            display::print_error(
                caller,
                &format!(
                    "draft #{n} doesn't exist — you have {total} draft{} (1–{total})",
                    plural(total)
                ),
            );
            return ExitCode::NotFound;
        }
    }

    // Cap to the most recent N unless `--all` was passed or the focus
    // would otherwise land in the hidden tail.
    let want_all = show_all
        || focus_number
            .map(|n| n + crate::commands::helpers::DEFAULT_RECENT_DRAFTS_CAP <= total)
            .unwrap_or(false);
    let (display_drafts, hidden) = if want_all {
        (drafts, 0)
    } else {
        crate::commands::helpers::cap_recent_drafts(
            drafts,
            crate::commands::helpers::DEFAULT_RECENT_DRAFTS_CAP,
        )
    };

    // Default focus = newest (the list is captured_at ASC). An explicit
    // number is re-anchored against the kept slice.
    let focus = if display_drafts.is_empty() {
        0
    } else {
        let last = display_drafts.len() - 1;
        match focus_number {
            Some(n) => n.saturating_sub(1).saturating_sub(hidden).min(last),
            None => last,
        }
    };

    match crate::tui::screens::show::run_focused_with_view_and_prefill(
        display_drafts,
        focus,
        hidden,
        initial_view,
        prefill_bundle_name,
    ) {
        Ok(crate::tui::screens::show::ShowOutcome::PushAfterExit) => {
            crate::commands::push::run(crate::agent::OutputMode::Auto)
        }
        Ok(_) => ExitCode::Success,
        Err(e) => {
            display::print_error(caller, &e.to_string());
            ExitCode::GenericError
        }
    }
}

// ─── Core flows ────────────────────────────────────────────────────────────

/// Passthrough today. Reserved for a future opt-in filter that hides
/// agent turns flagged with `file_context.agent_no_edits = true`.
fn filter_with_changed_files(drafts: Vec<DraftRecord>) -> Vec<DraftRecord> {
    drafts
}

fn filter_by_repo(
    drafts: Vec<DraftRecord>,
    repo: &str,
    proj_by_id: &BTreeMap<String, String>,
) -> Vec<DraftRecord> {
    if repo.is_empty() {
        return drafts;
    }
    let mut target_id = String::new();
    for (id, name) in proj_by_id {
        if name.eq_ignore_ascii_case(repo) {
            target_id = id.clone();
            break;
        }
    }
    drafts
        .into_iter()
        .filter(|d| {
            if d.project_name.eq_ignore_ascii_case(repo) {
                return true;
            }
            if !target_id.is_empty() {
                return d.touched_project_ids().iter().any(|id| id == &target_id);
            }
            false
        })
        .collect()
}

fn get_available_drafts(
    ctx: &ProjectContext,
    repo_filter: &str,
    proj_by_id: &BTreeMap<String, String>,
) -> anyhow::Result<Vec<DraftRecord>> {
    if ctx.single_repo && !ctx.ids.is_empty() {
        let all = store::get_drafts_by_status(store::DraftStatus::Draft, &[], &[])?;
        let staged = store::get_staged_drafts()?;
        let mut merged = all;
        merged.extend(staged);
        let id_set: std::collections::HashSet<String> = ctx.ids.iter().cloned().collect();
        let name_set: std::collections::HashSet<String> =
            ctx.names.iter().map(|n| n.to_lowercase()).collect();
        let mut matched: Vec<DraftRecord> = Vec::new();
        for d in merged {
            let match_via_id = id_set.contains(&d.project_id);
            let match_via_name = name_set.contains(&d.project_name.to_lowercase());
            let match_via_touched = d
                .touched_project_ids()
                .into_iter()
                .any(|id| id_set.contains(&id));
            if match_via_id || match_via_name || match_via_touched {
                matched.push(d);
            }
        }
        let candidates = filter_with_changed_files(matched);
        let bundled = store::get_bundled_draft_ids_for_project(&ctx.ids[0]).unwrap_or_default();
        let _ = repo_filter;
        let _ = proj_by_id;
        if bundled.is_empty() {
            return Ok(candidates);
        }
        return Ok(candidates
            .into_iter()
            .filter(|d| !bundled.contains(&d.id))
            .collect());
    }

    let drafts = store::get_drafts_by_status(store::DraftStatus::Draft, &ctx.ids, &ctx.names)?;
    let staged = store::get_staged_drafts()?;
    let mut merged = drafts;
    merged.extend(staged);
    Ok(filter_with_changed_files(filter_by_repo(
        merged,
        repo_filter,
        proj_by_id,
    )))
}

fn run_bundle_create(name: &str, select_arg: &str, repo_filter: Option<&str>) -> ExitCode {
    let ctx = resolve();
    let proj_by_id = load_proj_by_id();
    let repo_filter = repo_filter.unwrap_or("");
    let all = match get_available_drafts(&ctx, repo_filter, &proj_by_id) {
        Ok(v) => v,
        Err(e) => {
            display::print_error("bundle", &e.to_string());
            return ExitCode::GenericError;
        }
    };
    if all.is_empty() {
        if !repo_filter.is_empty() || ctx.single_repo {
            let label = if !repo_filter.is_empty() {
                repo_filter
            } else {
                ctx.name.as_str()
            };
            display::eprintln(&format!(
                "PCR: No draft prompts attributed to repo {label:?}."
            ));
        } else {
            display::eprintln(
                "PCR: No draft prompts available. Run `pcr start` to capture prompts.",
            );
        }
        return ExitCode::Success;
    }

    let selected = if select_arg.eq_ignore_ascii_case("all") {
        all.clone()
    } else {
        parse_selection(select_arg, &all)
    };
    if selected.is_empty() {
        display::eprintln("PCR: No valid selection — nothing bundled.");
        display::print_hint("examples:  --select 1-5   --select 1,3,7   --select all");
        return ExitCode::Success;
    }

    let project_id = ctx.ids.first().cloned().unwrap_or_default();
    let project_name = ctx.name.clone();
    let branch = current_branch();
    let sha = format!("bundle-{}", generate_hex_id());

    if let Err(e) = store::create_commit(
        name,
        &sha,
        &draft_ids(&selected),
        &project_id,
        &project_name,
        &branch,
        "closed",
        ctx.single_repo,
    ) {
        display::print_error("bundle", &e.to_string());
        return ExitCode::GenericError;
    }

    display::eprintln(&format!(
        "PCR: Created prompt bundle {name:?} ({} prompt{}) — push with `pcr push`",
        selected.len(),
        plural(selected.len()),
    ));
    ExitCode::Success
}

fn run_bundle_add(name: &str, select_arg: &str) -> ExitCode {
    let bundle = match store::get_bundle_by_name(name) {
        Ok(Some(b)) => b,
        Ok(None) => {
            display::print_error(
                "bundle",
                &format!(
                    "no bundle named {name:?} — create it first with: pcr bundle {name:?} --select 1-5"
                ),
            );
            return ExitCode::NotFound;
        }
        Err(e) => {
            display::print_error("bundle", &e.to_string());
            return ExitCode::GenericError;
        }
    };
    let ctx = resolve();
    let drafts = store::get_drafts_by_status(store::DraftStatus::Draft, &ctx.ids, &ctx.names)
        .unwrap_or_default();
    let staged = store::get_staged_drafts().unwrap_or_default();
    let mut all = drafts;
    all.extend(staged);
    if all.is_empty() {
        display::eprintln("PCR: No draft prompts available to add.");
        return ExitCode::Success;
    }
    let selected = if select_arg.eq_ignore_ascii_case("all") {
        all.clone()
    } else {
        parse_selection(select_arg, &all)
    };
    if selected.is_empty() {
        display::eprintln("PCR: No valid selection — nothing added.");
        return ExitCode::Success;
    }
    if let Err(e) = store::add_drafts_to_bundle(&bundle.id, &draft_ids(&selected), ctx.single_repo)
    {
        display::print_error("bundle", &e.to_string());
        return ExitCode::GenericError;
    }
    display::eprintln(&format!(
        "PCR: Added {} prompt{} to {name:?} — push with `pcr push`",
        selected.len(),
        plural(selected.len())
    ));
    ExitCode::Success
}

fn run_bundle_remove(name: &str, select_arg: &str) -> ExitCode {
    let bundle = match store::get_bundle_by_name(name) {
        Ok(Some(b)) => b,
        Ok(None) => {
            display::print_error("bundle", &format!("no bundle named {name:?}"));
            return ExitCode::NotFound;
        }
        Err(e) => {
            display::print_error("bundle", &e.to_string());
            return ExitCode::GenericError;
        }
    };
    let full = match store::get_commit_with_items(&bundle.id) {
        Ok(Some(f)) => f,
        Ok(None) => {
            display::eprintln(&format!("PCR: Bundle {name:?} is empty."));
            return ExitCode::Success;
        }
        Err(e) => {
            display::print_error("bundle", &e.to_string());
            return ExitCode::GenericError;
        }
    };
    if full.items.is_empty() {
        display::eprintln(&format!("PCR: Bundle {name:?} is empty."));
        return ExitCode::Success;
    }
    let selected = if select_arg.eq_ignore_ascii_case("all") {
        full.items.clone()
    } else {
        parse_selection(select_arg, &full.items)
    };
    if selected.is_empty() {
        display::eprintln("PCR: No valid selection — nothing removed.");
        return ExitCode::Success;
    }
    if let Err(e) = store::remove_drafts_from_bundle(&bundle.id, &draft_ids(&selected)) {
        display::print_error("bundle", &e.to_string());
        return ExitCode::GenericError;
    }
    display::eprintln(&format!(
        "PCR: Removed {} prompt{} from {name:?} — they're back in drafts.",
        selected.len(),
        plural(selected.len())
    ));
    ExitCode::Success
}

fn run_bundle_delete(name: &str) -> ExitCode {
    let bundle = match store::get_bundle_by_name(name) {
        Ok(Some(b)) => b,
        Ok(None) => {
            display::print_error("bundle", &format!("no bundle named {name:?}"));
            return ExitCode::NotFound;
        }
        Err(e) => {
            display::print_error("bundle", &e.to_string());
            return ExitCode::GenericError;
        }
    };
    if let Err(e) = store::delete_bundle(&bundle.id) {
        display::print_error("bundle", &e.to_string());
        return ExitCode::GenericError;
    }
    display::eprintln(&format!(
        "PCR: Deleted bundle {name:?} — prompts returned to drafts."
    ));
    ExitCode::Success
}

fn run_bundle_list() -> ExitCode {
    let unpushed = match store::get_unpushed_commits() {
        Ok(v) => v,
        Err(e) => {
            display::print_error("bundle", &e.to_string());
            return ExitCode::GenericError;
        }
    };
    if unpushed.is_empty() {
        display::eprintln("PCR: No unpushed bundles — everything is pushed.");
        display::print_hint("create a new bundle: `pcr bundle \"name\" --select all`");
        return ExitCode::Success;
    }
    display::eprintln(&format!(
        "{}  ({})\n",
        display::cstr(Color::Bold, "Unpushed prompt bundles"),
        unpushed.len()
    ));
    for b in &unpushed {
        let count = store::get_commit_with_items(&b.id)
            .ok()
            .flatten()
            .map(|f| f.items.len())
            .unwrap_or(0);
        let (marker, status) = if b.bundle_status == "open" {
            (display::cstr(Color::Yellow, "~"), "open")
        } else {
            (display::cstr(Color::Green, "✓"), "sealed")
        };
        display::eprintln(&format!(
            "  {marker}  {}  {}",
            display::cstr(Color::Bold, &format!("{:?}", b.message)),
            display::cstr(
                Color::Dim,
                &format!("({count} prompt{}, {status})", plural(count))
            ),
        ));
    }
    display::eprintln("");
    display::eprintln(&format!(
        "  {}   push all sealed bundles",
        display::cstr(Color::Yellow, "pcr push")
    ));
    ExitCode::Success
}

fn render_draft_list(title: &str, all: &[DraftRecord], proj_by_id: &BTreeMap<String, String>) {
    display::eprintln(&format!(
        "{}  ({})\n",
        display::cstr(Color::Bold, title),
        all.len()
    ));
    for (idx, d) in all.iter().enumerate() {
        let date = format_captured_at(&d.captured_at);
        let touched = d.touched_project_ids();
        let badge = if touched.len() > 1 {
            let names: Vec<String> = touched
                .iter()
                .filter_map(|id| proj_by_id.get(id).cloned())
                .collect();
            if names.is_empty() {
                String::new()
            } else {
                format!(
                    " {}",
                    display::cstr(Color::Cyan, &format!("[{}]", names.join(",")))
                )
            }
        } else if !d.project_name.is_empty() {
            format!(
                " {}",
                display::cstr(Color::Cyan, &format!("[{}]", d.project_name))
            )
        } else {
            String::new()
        };
        let mode = d
            .file_context
            .as_ref()
            .and_then(|m| m.get("cursor_mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let mode_fmt = if mode.is_empty() {
            String::new()
        } else {
            format!(" {}", display::cstr(Color::Dim, mode))
        };
        display::eprintln(&format!(
            "  [{}] {}{badge}{mode_fmt} {:?}",
            idx + 1,
            display::cstr(Color::Dim, &date),
            prompt_preview(&d.prompt_text, 55)
        ));
    }
    display::eprintln("");
}

fn run_bundle_show_hint(name: &str, repo_filter: Option<&str>) -> ExitCode {
    sync_latest_prompts();
    let ctx = resolve();
    let proj_by_id = load_proj_by_id();
    let repo_filter = repo_filter.unwrap_or("");
    let all = get_available_drafts(&ctx, repo_filter, &proj_by_id).unwrap_or_default();
    if all.is_empty() {
        display::eprintln("PCR: No draft prompts available.");
        return ExitCode::Success;
    }
    let mut title = "Draft prompts".to_string();
    if !repo_filter.is_empty() {
        title.push_str(&format!("  (repo: {repo_filter})"));
    }
    render_draft_list(&title, &all, &proj_by_id);
    let repo_suffix = if repo_filter.is_empty() {
        String::new()
    } else {
        format!(" --repo {repo_filter}")
    };
    display::eprintln(&format!(
        "  {}",
        display::cstr(
            Color::Yellow,
            &format!("pcr bundle {name:?} --select 1-5{repo_suffix}")
        )
    ));
    display::eprintln(&format!(
        "  {}",
        display::cstr(
            Color::Yellow,
            &format!("pcr bundle {name:?} --select all{repo_suffix}")
        )
    ));
    ExitCode::Success
}

fn sync_latest_prompts() {
    // Mirror Go's `syncLatestPrompts`: pull in the 5 most recently modified
    // Cursor transcripts before showing the draft list.
    crate::display::eprint("Fetching latest prompts...\r");
    crate::sources::cursor::force_sync("", 5);
    crate::display::eprint("                          \r");
}

fn run_bundle_overview(repo_filter: Option<&str>) -> ExitCode {
    sync_latest_prompts();
    let ctx = resolve();
    let proj_by_id = load_proj_by_id();
    let repo_filter = repo_filter.unwrap_or("");
    let all = get_available_drafts(&ctx, repo_filter, &proj_by_id).unwrap_or_default();
    let unpushed = store::get_unpushed_commits().unwrap_or_default();

    if !all.is_empty() {
        let mut title = "Draft prompts".to_string();
        if !repo_filter.is_empty() {
            title.push_str(&format!("  (repo: {repo_filter})"));
        }
        render_draft_list(&title, &all, &proj_by_id);
    } else if !repo_filter.is_empty() {
        display::eprintln(&format!(
            "{}  0 for repo {repo_filter:?}\n",
            display::cstr(Color::Bold, "Drafts")
        ));
    } else {
        display::eprintln(&format!(
            "{}  0 — run `pcr start` to capture prompts\n",
            display::cstr(Color::Bold, "Drafts")
        ));
    }

    if !unpushed.is_empty() {
        display::eprintln(&format!(
            "{}  ({})\n",
            display::cstr(Color::Bold, "Unpushed prompt bundles"),
            unpushed.len()
        ));
        for b in &unpushed {
            let count = store::get_commit_with_items(&b.id)
                .ok()
                .flatten()
                .map(|f| f.items.len())
                .unwrap_or(0);
            let marker = if b.bundle_status == "open" {
                display::cstr(Color::Yellow, "~")
            } else {
                display::cstr(Color::Green, "✓")
            };
            display::eprintln(&format!(
                "  {marker}  {}  {}",
                display::cstr(Color::Bold, &format!("{:?}", b.message)),
                display::cstr(Color::Dim, &format!("({count} prompt{})", plural(count))),
            ));
        }
        display::eprintln("");
    }

    display::eprintln(&display::cstr(Color::Bold, "Usage:"));
    display::eprintln(&format!(
        "  {}            create bundle from drafts 1-5",
        display::cstr(Color::Yellow, "pcr bundle \"name\" --select 1-5")
    ));
    display::eprintln(&format!(
        "  {}            bundle all drafts",
        display::cstr(Color::Yellow, "pcr bundle \"name\" --select all")
    ));
    display::eprintln(&format!(
        "  {}  bundle only cli drafts",
        display::cstr(Color::Yellow, "pcr bundle \"name\" --select all --repo cli")
    ));
    display::eprintln(&format!(
        "  {}                                   push all sealed bundles",
        display::cstr(Color::Yellow, "pcr push")
    ));
    display::eprintln(&format!(
        "  {}                          see full text of a draft",
        display::cstr(Color::Dim, "pcr show <number>")
    ));
    ExitCode::Success
}

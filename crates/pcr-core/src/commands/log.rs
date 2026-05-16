//! `pcr log`. Mirrors `cli/cmd/log.go`.

use crate::agent::OutputMode;
use crate::commands::project_context::resolve;
use crate::display::{self, Color};
use crate::exit::ExitCode;
use crate::store::{self, DraftRecord};
use crate::util::text::{plural, prompt_preview};
use crate::util::time::fmt_time;

/// Map a draft's source string to a stable display label + color so the
/// drafts section reads like the dashboard's source chips. Anything we
/// don't recognise falls back to a neutral `…` glyph and dim styling so
/// future sources don't crash the formatter.
fn source_chip(source: &str) -> (&'static str, Color) {
    match source {
        "cursor" => ("cursor", Color::Cyan),
        "claude-code" | "claude" => ("claude", Color::Magenta),
        "vscode" | "vscode-copilot" | "copilot" => ("vscode", Color::Yellow),
        _ => ("source", Color::Gray),
    }
}

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

    let branch = crate::commands::helpers::current_branch();
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
        render_grouped_drafts(&drafts);
        display::eprintln("\n  Run `pcr bundle` to create a prompt bundle");
    }

    display::eprintln("");
    ExitCode::Success
}

/// Render the drafts list grouped by session. Drafts inside a session
/// are scrollable as a unit and align their HH:MM timestamps so the
/// reader can scan a long log the same way they'd skim `git log`.
/// Sessions are sorted by their newest draft's `captured_at` (newest
/// first) so the most-relevant context is at the top of the section.
fn render_grouped_drafts(drafts: &[DraftRecord]) {
    use std::collections::BTreeMap;

    if drafts.is_empty() {
        return;
    }

    // Bucket by `session_id`. Drafts with no session id (legacy
    // captures) get their own per-id bucket so they still render but
    // don't share a session header with unrelated rows.
    let mut buckets: BTreeMap<String, Vec<&DraftRecord>> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for d in drafts {
        let key = if d.session_id.is_empty() {
            format!("solo:{}", d.id)
        } else {
            d.session_id.clone()
        };
        if !buckets.contains_key(&key) {
            order.push(key.clone());
        }
        buckets.entry(key).or_default().push(d);
    }

    // Sort each bucket by captured_at ascending so the session header
    // can advertise the start time and the row order reads forward.
    for v in buckets.values_mut() {
        v.sort_by(|a, b| a.captured_at.cmp(&b.captured_at));
    }
    // Reorder bucket keys: newest-session-first, by the bucket's last
    // draft captured_at.
    order.sort_by(|a, b| {
        let aa = buckets
            .get(a)
            .and_then(|v| v.last())
            .map(|d| &d.captured_at);
        let bb = buckets
            .get(b)
            .and_then(|v| v.last())
            .map(|d| &d.captured_at);
        bb.cmp(&aa)
    });

    for key in order {
        let Some(items) = buckets.get(&key) else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        // Session header. Show source chip + N exchanges + start time.
        // Solo (legacy) buckets skip the header entirely so a single
        // unattributed draft doesn't get a "session of 1" decoration.
        let solo = key.starts_with("solo:");
        if !solo {
            let first = items[0];
            let (label, color) = source_chip(&first.source);
            let ts = first
                .captured_at
                .split('T')
                .next()
                .unwrap_or("")
                .to_string();
            let session_short: String = first.session_id.chars().take(8).collect();
            display::eprintln(&format!(
                "  {} {} {}",
                display::cstr(color, &format!("● {label}")),
                display::cstr(
                    Color::Dim,
                    &format!(
                        "session {session_short} · {} prompt{}",
                        items.len(),
                        plural(items.len()),
                    ),
                ),
                display::cstr(Color::Gray, &ts),
            ));
        }
        for d in items {
            let preview = prompt_preview(&d.prompt_text, 60);
            // HH:MM aligned right of the bullet so eyes can scan a
            // column of times. Falls back to the bare ISO date when
            // captured_at lacks a time component.
            let time = match d.captured_at.split_once('T') {
                Some((_, tail)) => tail
                    .split_once(':')
                    .map(|(h, rest)| {
                        let m = rest.split(':').next().unwrap_or("");
                        format!("{h}:{m}")
                    })
                    .unwrap_or_else(|| d.captured_at.clone()),
                None => d.captured_at.clone(),
            };
            let (_, color) = source_chip(&d.source);
            // Two-space indent under the session header so the visual
            // tree is unambiguous; solo rows keep the original indent
            // level so they don't look orphaned.
            let indent = if solo { "  " } else { "    " };
            display::eprintln(&format!(
                "{indent}{} {}  {}",
                display::cstr(color, "◦"),
                display::cstr(Color::Dim, &time),
                display::cstr(Color::Bold, &preview),
            ));
        }
    }
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

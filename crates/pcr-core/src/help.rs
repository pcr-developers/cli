//! Single source of truth for command help text. Both clap's `--help` /
//! `--long-help` output (Phase 3) and the interactive `pcr help` TUI
//! (Phase 2d) read from this table, so the two stay in lockstep.
//!
//! Each entry is intentionally written for two audiences at once:
//!
//! 1. **Newcomers** — opens with a one-sentence "what does this do" plus
//!    a short paragraph explaining when you'd reach for it.
//! 2. **Returning users** — every entry ends with worked examples and
//!    cross-refs so nobody has to grep the docs.

pub struct HelpEntry {
    pub command: &'static str,
    pub short: &'static str,
    pub purpose: &'static str,
    pub when_to_use: &'static str,
    pub examples: &'static [(&'static str, &'static str)],
    pub see_also: &'static [&'static str],
    /// What `Enter` should do for this command in the interactive
    /// `pcr help` TUI. See [`Runnable`] for the possible behaviours.
    pub runnable: Runnable,
}

/// What pressing `Enter` on a command in `pcr help` does.
///
/// The `pcr help` TUI uses this to decide whether it can launch a
/// command directly or whether it should hand the user back to their
/// shell with a copyable example.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Runnable {
    /// Safe to launch in-process with no arguments. `Enter` quits help
    /// and runs the command immediately (e.g. `pcr status` opens the
    /// status TUI; `pcr login` runs the interactive login flow).
    Direct,
    /// Command needs a positional argument that we can't infer.
    /// `Enter` exits help and prints the first example so the user can
    /// edit it in their shell. Used by `pcr show <n>`.
    NeedsArgs,
    /// Internal / hidden command. `Enter` is a no-op.
    Hidden,
}

pub const HELP: &[HelpEntry] = &[
    HelpEntry {
        command: "login",
        short: "Authenticate with PCR.dev",
        purpose: "Open your browser, paste a CLI token from Settings, and persist it under ~/.pcr-dev/auth.json.",
        when_to_use: "Run this once on each machine before `pcr push` or `pcr init`. If a push fails with 401, your token may have expired — run login again.",
        examples: &[
            ("pcr login", "interactive — opens https://pcr.dev/settings"),
            ("echo $TOKEN | pcr login", "scripted — read token from stdin"),
        ],
        see_also: &["logout", "status"],
        runnable: Runnable::Direct,
    },
    HelpEntry {
        command: "logout",
        short: "Remove saved credentials",
        purpose: "Delete ~/.pcr-dev/auth.json. Existing local drafts and bundles are kept.",
        when_to_use: "Switching accounts, lending your laptop, or rotating a leaked token.",
        examples: &[("pcr logout", "remove credentials")],
        see_also: &["login"],
        runnable: Runnable::Direct,
    },
    HelpEntry {
        command: "init",
        short: "Register a project",
        purpose: "Tag the current git repository as a tracked project so prompts captured against it get attributed correctly.",
        when_to_use: "Once per project. The first time you start working in a new repo, cd in and run `pcr init`.",
        examples: &[
            ("cd ~/code/my-app && pcr init", "register one repo"),
            ("cd ~/code && pcr init", "register every git repo found one level down"),
            ("pcr init --unregister", "stop tracking the current directory"),
        ],
        see_also: &["status", "start"],
        runnable: Runnable::Direct,
    },
    HelpEntry {
        command: "start",
        short: "Watch for prompts and capture them as drafts",
        purpose: "Run a foreground watcher that listens to Cursor, Claude Code, and VS Code Copilot session files. Each user prompt becomes a local draft you can later bundle and push.",
        when_to_use: "Keep `pcr start` running in a terminal alongside your editor. Press `q` to stop.",
        examples: &[
            ("pcr start", "watch with the default TUI"),
            ("pcr start --verbose", "show every diff/session/scan event in the log"),
            ("pcr start --plain", "no full-screen TUI — line output only (good for CI / agents)"),
        ],
        see_also: &["status", "log", "show"],
        runnable: Runnable::Direct,
    },
    HelpEntry {
        command: "status",
        short: "Snapshot of auth, projects, drafts, bundles",
        purpose: "One screen telling you whether you're signed in, which projects are registered, and where your drafts are in the pipeline.",
        when_to_use: "Run this any time you've lost track of state. Press `r` to refresh, `q` to exit.",
        examples: &[
            ("pcr status", "interactive overview"),
            ("pcr --json status", "machine-readable output for scripts"),
        ],
        see_also: &["log", "bundle", "push"],
        runnable: Runnable::Direct,
    },
    HelpEntry {
        command: "log",
        short: "Show captured prompts and bundles for the current repo",
        purpose: "Per-project view of pushed bundles, sealed bundles, open bundles, and unbundled drafts.",
        when_to_use: "When you want to remember what you've already shipped vs what's pending in the current repo.",
        examples: &[
            ("pcr log", "log for the current project"),
            ("pcr --json log", "JSON for tooling"),
        ],
        see_also: &["status", "show", "bundle"],
        runnable: Runnable::Direct,
    },
    HelpEntry {
        command: "show",
        short: "Open a draft in the full-screen browser",
        purpose: "Inspect one draft's full prompt, response, tool calls, changed files, and metadata.",
        when_to_use: "When `pcr log` shows something interesting and you want the full text. Numbers come from `pcr log` / `pcr bundle`.",
        examples: &[
            ("pcr show 3", "open draft #3"),
            ("pcr --plain show 3", "print to stderr instead of the TUI"),
        ],
        see_also: &["log", "bundle"],
        runnable: Runnable::NeedsArgs,
    },
    HelpEntry {
        command: "bundle",
        short: "Group drafts into a named, reviewable bundle",
        purpose: "Bundles are the unit of review on PCR.dev. You select N drafts, give them a name, and `pcr push` ships them.",
        when_to_use: "After a coherent block of work — a feature, a fix, an experiment. Bundles can be edited (add/remove drafts) until you push them.",
        examples: &[
            ("pcr bundle", "show drafts + bundle overview"),
            ("pcr bundle \"auth fix\" --select 1-5", "create a sealed bundle from drafts 1 through 5"),
            ("pcr bundle \"auth fix\" --select all", "bundle every draft in the current repo"),
            ("pcr bundle \"auth fix\" --add --select 6,7", "add drafts 6 and 7 to an existing bundle"),
            ("pcr bundle \"auth fix\" --remove --select 2", "remove draft #2 from the bundle"),
            ("pcr bundle --list", "list every unpushed bundle across projects"),
        ],
        see_also: &["log", "show", "push"],
        runnable: Runnable::Direct,
    },
    HelpEntry {
        command: "push",
        short: "Upload sealed bundles to PCR.dev for review",
        purpose: "Send every sealed bundle (and any open ones — they get auto-sealed) to your team's PCR.dev dashboard.",
        when_to_use: "When a bundle is ready for review. Push surfaces the prompt history, diffs, and PR link to your reviewers.",
        examples: &[
            ("pcr push", "push every sealed bundle"),
        ],
        see_also: &["bundle", "pull", "log"],
        runnable: Runnable::Direct,
    },
    HelpEntry {
        command: "pull",
        short: "Restore a pushed bundle to local drafts",
        purpose: "Re-create local drafts for a bundle that was pushed from another machine or that you'd locally garbage-collected.",
        when_to_use: "Rare. Mostly useful when you're picking up a teammate's bundle for context.",
        examples: &[
            ("pcr pull", "list pushed bundles, pick one"),
            ("pcr pull <remote-id>", "restore a specific bundle by ID"),
        ],
        see_also: &["push", "log"],
        runnable: Runnable::Direct,
    },
    HelpEntry {
        command: "gc",
        short: "Reclaim space in the local store",
        purpose: "Delete pushed records older than N days, discard unpushed bundles, remove orphaned bundles whose git SHA no longer exists, or clear out stale unbundled drafts.",
        when_to_use: "When `~/.pcr-dev/drafts.db` is growing, your `pcr bundle` list is full of old experiments, or you want to throw away abandoned drafts.",
        examples: &[
            ("pcr gc", "delete pushed records older than 30 days"),
            ("pcr gc --older-than 7d", "delete pushed records older than 7 days"),
            ("pcr gc --all-pushed", "delete every pushed record locally"),
            ("pcr gc --unpushed", "discard every unpushed bundle"),
            ("pcr gc --orphaned", "delete unpushed bundles whose HEAD SHA is gone"),
            ("pcr gc --drafts-older-than 7d", "drop unbundled drafts older than 7 days"),
            ("pcr gc --drafts", "drop every unbundled draft (bundled / pushed untouched)"),
        ],
        see_also: &["log", "push", "bundle"],
        runnable: Runnable::Direct,
    },
    HelpEntry {
        command: "mcp",
        short: "Start the MCP server on stdio (not yet implemented in v0.2.x)",
        purpose: "Future home of the Model Context Protocol server that exposes pcr_log_prompt / pcr_log_session / pcr_status as MCP tools.",
        when_to_use: "Currently exits with code 50 (NotImplemented). Track the rewrite issue on the cli repo.",
        examples: &[("pcr mcp", "shows a placeholder message and exits")],
        see_also: &["start"],
        runnable: Runnable::Direct,
    },
    HelpEntry {
        command: "hook",
        short: "Internal — invoked by Claude Code's Stop hook",
        purpose: "Hidden subcommand. Claude Code runs this after every response so PCR can prompt you to bundle the latest exchange.",
        when_to_use: "You don't run this directly. It's wired up automatically when you use `pcr start` and Claude Code together.",
        examples: &[],
        see_also: &["start"],
        runnable: Runnable::Hidden,
    },
];

/// Look up an entry by command name. Returns `None` if no match.
pub fn entry(command: &str) -> Option<&'static HelpEntry> {
    HELP.iter().find(|h| h.command == command)
}

/// Render an entry as plain text suitable for clap's `long_about` /
/// `after_help`. Phase 3 wires this into the CLI parser.
pub fn render_plain(entry: &HelpEntry) -> String {
    let mut out = String::new();
    out.push_str(entry.purpose);
    out.push_str("\n\n");
    out.push_str("WHEN TO USE\n");
    out.push_str("  ");
    out.push_str(entry.when_to_use);
    out.push_str("\n\n");
    if !entry.examples.is_empty() {
        out.push_str("EXAMPLES\n");
        for (cmd, desc) in entry.examples {
            out.push_str(&format!("  $ {cmd}\n      {desc}\n"));
        }
        out.push('\n');
    }
    if !entry.see_also.is_empty() {
        out.push_str("SEE ALSO\n  ");
        out.push_str(
            &entry
                .see_also
                .iter()
                .map(|s| format!("pcr {s}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push('\n');
    }
    out.push_str(&format!("\nMore: https://pcr.dev/docs/{}\n", entry.command));
    out
}

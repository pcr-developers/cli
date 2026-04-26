//! Top-level CLI entry point. Both `pcr-cli` and `pcr-napi` call this.
//!
//! Every subcommand's `--help` / `--long-help` text is rendered from the
//! single source-of-truth table in [`crate::help`]. This guarantees that
//! `pcr help` (interactive) and `pcr <cmd> --help` (line) say the same
//! thing forever, without us having to maintain two copies.

use clap::{Args, Parser, Subcommand};

use crate::agent::OutputMode;
use crate::exit::ExitCode;
use crate::help;

const ROOT_LONG_ABOUT: &str = "\
PCR.dev — Prompt & Code Review for AI-native teams.

Captures every prompt you send to Cursor / Claude Code / VS Code Copilot,
attributes it to the right project + branch + git SHA, and ships it to
your team's PCR.dev dashboard for code-review-style discussion.

Get started:
  pcr login              authenticate
  cd your-repo && pcr init   register the project
  pcr start              capture prompts as you work
  pcr bundle \"name\" --select all   group drafts
  pcr push               ship for review

Tips:
  pcr help               browse every command interactively
  pcr <cmd> --help       command-specific examples
  pcr --plain ...        line-mode output (good for scripts / agents)
  pcr --json status      machine-readable JSON
";

const ROOT_AFTER_HELP: &str = "\
Docs:    https://pcr.dev/docs
Issues:  https://github.com/pcr-developers/cli/issues
";

#[derive(Debug, Parser)]
#[command(
    name = "pcr",
    about = "PCR.dev — capture, bundle, and review your AI prompts",
    long_about = ROOT_LONG_ABOUT,
    after_help = ROOT_AFTER_HELP,
    version = concat!(env!("CARGO_PKG_VERSION"), " (rust)"),
    disable_help_subcommand = true,
    disable_version_flag = false,
)]
struct Cli {
    #[command(flatten)]
    global: GlobalArgs,

    /// Subcommand. When omitted, `pcr` opens the interactive command
    /// browser (same as `pcr help`) on a TTY, or prints the long-form
    /// help to stderr when piped / `--plain` / `CI` / `NO_COLOR`.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Args, Clone, Default)]
struct GlobalArgs {
    /// Force line-based output (disables ratatui TUI). Implied by NO_COLOR / CI / non-TTY.
    #[arg(long, global = true)]
    plain: bool,

    /// Emit machine-readable JSON on stdout. Implies `--plain`.
    #[arg(long, global = true)]
    json: bool,
}

impl GlobalArgs {
    fn output_mode(&self) -> OutputMode {
        if self.json {
            OutputMode::Json
        } else if self.plain {
            OutputMode::Plain
        } else {
            OutputMode::Auto
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Authenticate with PCR.dev — opens your browser, paste a CLI token
    Login,

    /// Remove saved credentials from ~/.pcr-dev/auth.json
    Logout,

    /// Register the current git repo (or all sub-repos) as a tracked project
    Init(InitArgs),

    /// Watch your editor for new prompts and save them as local drafts
    Start(StartArgs),

    /// Start the MCP server on stdio (not yet implemented in v0.2.x)
    Mcp,

    /// Snapshot of auth, projects, and the draft-bundle-push pipeline
    Status,

    /// Group captured drafts into named, reviewable bundles
    #[command(arg_required_else_help = false)]
    Bundle(BundleArgs),

    /// Push sealed bundles to PCR.dev for review
    Push,

    /// Show captured prompts and bundles for the current repo
    Log,

    /// Open one draft in the full-screen browser (number from `pcr log`)
    Show(ShowArgs),

    /// Restore a pushed bundle to local drafts
    Pull(PullArgs),

    /// Reclaim local-store space (delete pushed records, orphans, etc.)
    Gc(GcArgs),

    /// Browse every command interactively
    Help,

    /// Internal — invoked by Claude Code's Stop hook
    #[command(hide = true)]
    Hook,
}

#[derive(Debug, Args, Clone, Default)]
pub struct InitArgs {
    /// Unregister the current project instead of registering it
    #[arg(long)]
    pub unregister: bool,
}

#[derive(Debug, Args, Clone, Default)]
pub struct StartArgs {
    /// Print real-time events from all watchers (diffs, session state changes, completed turns)
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Debug, Args, Clone, Default)]
pub struct BundleArgs {
    /// Bundle name (used as the commit message). Quote it if it contains spaces.
    pub name: Vec<String>,
    /// Select drafts by number — `1-5`, `1,3,7`, or `all`
    #[arg(long)]
    pub select: Option<String>,
    /// Add more prompts to an existing bundle (use with --select)
    #[arg(long)]
    pub add: bool,
    /// Remove prompts from a bundle, returning them to drafts
    #[arg(long)]
    pub remove: bool,
    /// Delete a bundle entirely, returning all its prompts to drafts
    #[arg(long)]
    pub delete: bool,
    /// List every unpushed bundle across projects
    #[arg(long)]
    pub list: bool,
    /// Filter drafts to only those touching a specific repo (e.g. cli, pcr-dev)
    #[arg(long)]
    pub repo: Option<String>,
    /// Show every draft, not just the most recent ones (default cap: 100)
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args, Clone, Default)]
pub struct ShowArgs {
    /// Draft number (1-based) — get one from `pcr log`
    pub number: String,
    /// Show every draft in the browser, not just the most recent ones (default cap: 100)
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args, Clone, Default)]
pub struct PullArgs {
    /// Remote bundle ID — if omitted, lists pushed bundles to pick from
    pub remote_id: Option<String>,
}

#[derive(Debug, Args, Clone, Default)]
pub struct GcArgs {
    /// Delete all pushed records regardless of age
    #[arg(long = "all-pushed")]
    pub all_pushed: bool,
    /// Delete pushed records older than N days (e.g. `30d` or just `7`)
    #[arg(long = "older-than")]
    pub older_than: Option<String>,
    /// Delete unpushed bundles whose git SHA no longer exists
    #[arg(long)]
    pub orphaned: bool,
    /// Discard all unpushed committed bundles
    #[arg(long)]
    pub unpushed: bool,
    /// Delete every unbundled draft prompt across all projects
    #[arg(long)]
    pub drafts: bool,
    /// Delete unbundled drafts older than N days (e.g. `7d` or just `7`)
    #[arg(long = "drafts-older-than")]
    pub drafts_older_than: Option<String>,
}

/// Parse `argv` and dispatch to the matching command. Returns the process exit code.
pub fn run(argv: Vec<String>) -> i32 {
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) => {
            let code = e.exit_code();
            let _ = e.print();
            return code;
        }
    };
    let mode = cli.global.output_mode();
    let code: ExitCode = match cli.command {
        // No subcommand → open the interactive command browser. On a non-
        // TTY / `--plain` / `--json` / `CI` / `NO_COLOR`, the help command
        // gracefully degrades to a line dump of every entry, so this stays
        // useful for scripts and agents that just want to discover commands.
        None => crate::commands::help::run(mode),
        Some(Command::Login) => crate::commands::login::run(mode),
        Some(Command::Logout) => crate::commands::logout::run(mode),
        Some(Command::Init(a)) => crate::commands::init::run(mode, a),
        Some(Command::Start(a)) => crate::commands::start::run(mode, a),
        Some(Command::Mcp) => crate::mcp::run_stub(),
        Some(Command::Status) => crate::commands::status::run(mode),
        Some(Command::Bundle(a)) => crate::commands::bundle::run(mode, a),
        Some(Command::Push) => crate::commands::push::run(mode),
        Some(Command::Log) => crate::commands::log::run(mode),
        Some(Command::Show(a)) => crate::commands::show::run(mode, a),
        Some(Command::Pull(a)) => crate::commands::pull::run(mode, a),
        Some(Command::Gc(a)) => crate::commands::gc::run(mode, a),
        Some(Command::Help) => crate::commands::help::run(mode),
        Some(Command::Hook) => crate::commands::hook::run(mode),
    };
    code.as_i32()
}

// Re-export a render helper for the command implementations that want to
// emit the long-form help in plain mode.
pub use help::render_plain as render_command_help;

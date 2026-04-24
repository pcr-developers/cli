//! Top-level CLI entry point. Both `pcr-cli` and `pcr-napi` call this.
//!
//! The clap tree mirrors `cli/cmd/root.go` — same subcommand names, same
//! flags, same hidden commands — so every muscle-memory invocation from
//! the Go build keeps working.

use clap::{Args, Parser, Subcommand};

use crate::agent::OutputMode;
use crate::exit::ExitCode;

#[derive(Debug, Parser)]
#[command(
    name = "pcr",
    about = "PCR.dev — prompt & code review",
    version = concat!(env!("CARGO_PKG_VERSION"), " (rust)"),
    disable_help_subcommand = true,
    disable_version_flag = false,
)]
struct Cli {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Args, Clone, Default)]
struct GlobalArgs {
    /// Force line-based output (disables ratatui TUI).
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
    /// Authenticate with PCR.dev
    Login,
    /// Remove saved credentials
    Logout,
    /// Register the current directory (or all sub-repos) as tracked projects
    Init(InitArgs),
    /// Watch for new Claude Code and Cursor prompts and save them as drafts
    Start(StartArgs),
    /// Start the MCP server on stdio
    Mcp,
    /// Show auth, registered projects, and prompt bundle state
    Status,
    /// Create and manage prompt bundles
    #[command(arg_required_else_help = false)]
    Bundle(BundleArgs),
    /// Push sealed prompt bundles to PCR.dev for review
    Push,
    /// Show captured prompts and prompt bundles for the current repo
    Log,
    /// Show the full content of a draft prompt by its list number
    Show(ShowArgs),
    /// Restore a pushed prompt bundle to local drafts
    Pull(PullArgs),
    /// Clean up old pushed records or orphaned prompt bundles
    Gc(GcArgs),
    /// Internal: called by Claude Code's Stop hook after each response
    #[command(hide = true)]
    Hook,
}

#[derive(Debug, Args, Clone, Default)]
pub struct InitArgs {
    /// Unregister the current project
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
    /// Bundle name (used as the commit message).
    pub name: Vec<String>,
    /// Select drafts by number (e.g. 1-5, 1,3,7, or all)
    #[arg(long)]
    pub select: Option<String>,
    /// Add more prompts to an existing bundle
    #[arg(long)]
    pub add: bool,
    /// Remove prompts from a bundle
    #[arg(long)]
    pub remove: bool,
    /// Delete a bundle and return its prompts to drafts
    #[arg(long)]
    pub delete: bool,
    /// List all unpushed bundles
    #[arg(long)]
    pub list: bool,
    /// Filter drafts to only those touching a specific repo (e.g. cli, pcr-dev)
    #[arg(long)]
    pub repo: Option<String>,
}

#[derive(Debug, Args, Clone, Default)]
pub struct ShowArgs {
    /// Draft number (1-based).
    pub number: String,
}

#[derive(Debug, Args, Clone, Default)]
pub struct PullArgs {
    /// Remote bundle ID. If omitted, lists pushed bundles interactively.
    pub remote_id: Option<String>,
}

#[derive(Debug, Args, Clone, Default)]
pub struct GcArgs {
    /// Delete all pushed records regardless of age
    #[arg(long = "all-pushed")]
    pub all_pushed: bool,
    /// Delete pushed records older than N days (e.g. 30d or 7)
    #[arg(long = "older-than")]
    pub older_than: Option<String>,
    /// Delete unpushed bundles whose git SHA no longer exists
    #[arg(long)]
    pub orphaned: bool,
    /// Discard all unpushed committed bundles
    #[arg(long)]
    pub unpushed: bool,
}

/// Parse `argv` and dispatch to the matching command. Returns the process exit code.
pub fn run(argv: Vec<String>) -> i32 {
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) => {
            // clap prints its own formatted error; propagate its exit code.
            let code = e.exit_code();
            let _ = e.print();
            return code;
        }
    };
    let mode = cli.global.output_mode();
    let code: ExitCode = match cli.command {
        Command::Login => crate::commands::login::run(mode),
        Command::Logout => crate::commands::logout::run(mode),
        Command::Init(a) => crate::commands::init::run(mode, a),
        Command::Start(a) => crate::commands::start::run(mode, a),
        Command::Mcp => crate::mcp::run_stub(),
        Command::Status => crate::commands::status::run(mode),
        Command::Bundle(a) => crate::commands::bundle::run(mode, a),
        Command::Push => crate::commands::push::run(mode),
        Command::Log => crate::commands::log::run(mode),
        Command::Show(a) => crate::commands::show::run(mode, a),
        Command::Pull(a) => crate::commands::pull::run(mode, a),
        Command::Gc(a) => crate::commands::gc::run(mode, a),
        Command::Hook => crate::commands::hook::run(mode),
    };
    code.as_i32()
}

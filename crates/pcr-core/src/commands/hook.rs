//! `pcr hook` — Claude Code Stop hook handler. Always exits 0 so the tool
//! never re-engages the model. Mirrors `cli/cmd/hook.go`.

use crate::agent::OutputMode;
use crate::commands::{project_context, start::pid_file_path, start::read_existing_pid};
use crate::exit::ExitCode;
use crate::sources::claudecode::hook::run_hook as run_claude_hook;

pub fn run(_mode: OutputMode) -> ExitCode {
    // Only act if `pcr start` is currently running. If `$HOME`
    // can't even be resolved we can't have a live watcher anyway —
    // exit 0 so the hook doesn't re-engage the model.
    let Ok(path) = pid_file_path() else {
        return ExitCode::Success;
    };
    if read_existing_pid(&path).is_none() {
        return ExitCode::Success;
    }
    let ctx = project_context::resolve();
    let _ = run_claude_hook(&ctx.ids, &ctx.names, &ctx.name);
    ExitCode::Success
}

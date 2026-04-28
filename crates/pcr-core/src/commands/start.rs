//! `pcr start`. Mirrors `cli/cmd/start.go`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::agent::{self, OutputMode};
use crate::config;
use crate::display;
use crate::entry::StartArgs;
use crate::exit::ExitCode;
use crate::projects;
use crate::sources;

pub fn pid_file_path() -> PathBuf {
    config::pcr_dir().join("watcher.pid")
}

pub fn read_existing_pid(pid_file: &PathBuf) -> Option<i32> {
    let data = std::fs::read_to_string(pid_file).ok()?;
    let pid: i32 = data.trim().parse().ok()?;
    #[cfg(unix)]
    {
        use std::io;
        // kill(pid, 0) checks liveness without delivering a signal.
        let r = unsafe { libc_stub::kill(pid, 0) };
        if r == 0 {
            return Some(pid);
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc_stub::ESRCH) {
            return None;
        }
        return Some(pid);
    }
    #[cfg(not(unix))]
    {
        // On Windows we don't attempt liveness detection — assume live.
        let _ = pid;
        Some(pid)
    }
}

pub fn run(mode: OutputMode, args: StartArgs) -> ExitCode {
    let pid_file = pid_file_path();

    if let Some(pid) = read_existing_pid(&pid_file) {
        if !agent::is_interactive_terminal() {
            display::eprintln(&format!("PCR: Replacing existing watcher (PID {pid})."));
        } else {
            display::eprint(&format!(
                "PCR: Watcher already running (PID {pid}). Replace it? [Y/n]: "
            ));
            let mut buf = String::new();
            let _ = std::io::stdin().read_line(&mut buf);
            if buf.trim().eq_ignore_ascii_case("n") {
                return ExitCode::Success;
            }
        }
        #[cfg(unix)]
        {
            unsafe {
                libc_stub::kill(pid, libc_stub::SIGTERM);
            }
        }
    }

    if let Some(parent) = pid_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&pid_file, format!("{}", std::process::id()));
    let pid_guard = PidFileGuard(pid_file.clone());

    display::set_verbose(args.verbose);
    let projs = projects::load();

    if agent::is_tui_eligible(mode) {
        // Launch the dashboard TUI. Sources still run in the background so
        // captures accumulate while the user views the dashboard.
        let _ = spawn_all_sources();
        let exit = run_tui_cycle(crate::tui::NavTarget::Start);
        drop(pid_guard);
        return exit;
    } else {
        display::print_startup_banner(crate::VERSION, crate::BUILD_TIME, projs.len());
        let _ = spawn_all_sources();
        wait_for_shutdown();
        display::eprintln("\nPCR: Watcher stopped.");
    }

    drop(pid_guard);
    ExitCode::Success
}

/// Cross-screen Tab / Left / Right cycle for the three TUI screens
/// (`pcr start` dashboard, drafts list, bundles list). Each screen
/// returns a [`crate::tui::NavTarget`] indicating where the user wants
/// to go next; this loop launches the matching screen until the user
/// quits or asks for `pcr push`.
///
/// Watchers must already be spawned by the caller — entering the
/// dashboard repeatedly does not respawn them.
pub fn run_tui_cycle(initial: crate::tui::NavTarget) -> ExitCode {
    use crate::tui::NavTarget;
    let mut target = match initial {
        NavTarget::Stay | NavTarget::Quit => return ExitCode::Success,
        NavTarget::PushAfterExit => {
            return crate::commands::push::run(crate::agent::OutputMode::Auto);
        }
        other => other,
    };
    // Outer guard pins the alt screen so nested screen transitions
    // don't briefly drop back to the cooked PowerShell buffer (which
    // on Windows causes the prompt to bleed through the TUI). Bound to
    // a named local so we can explicitly `drop()` it before running
    // `pcr push` — push prints the review URL to stderr and we'd
    // otherwise paint it onto the alt screen and wipe it on exit.
    let outer = crate::tui::app::AltScreenGuard::enter();
    loop {
        let next = match target {
            NavTarget::Start => match crate::tui::screens::start::run(0) {
                Ok(t) => t,
                Err(_) => NavTarget::Quit,
            },
            NavTarget::Drafts => launch_drafts_view(false),
            NavTarget::Bundles => launch_drafts_view(true),
            NavTarget::Stay => NavTarget::Quit,
            NavTarget::Quit => return ExitCode::Success,
            NavTarget::PushAfterExit => {
                drop(outer);
                return crate::commands::push::run(crate::agent::OutputMode::Auto);
            }
        };
        match next {
            NavTarget::Quit | NavTarget::Stay => return ExitCode::Success,
            NavTarget::PushAfterExit => {
                drop(outer);
                return crate::commands::push::run(crate::agent::OutputMode::Auto);
            }
            other => target = other,
        }
    }
}

/// Re-enter the drafts/bundles browser from the dispatcher loop.
/// Re-resolves project context and re-queries drafts each time so the
/// view always reflects current store state — cheap on a hot SQLite
/// page cache.
fn launch_drafts_view(start_on_bundles: bool) -> crate::tui::NavTarget {
    use crate::tui::NavTarget;
    let initial_view = if start_on_bundles {
        crate::tui::screens::show::InitialView::Bundles
    } else {
        crate::tui::screens::show::InitialView::Drafts
    };
    let ctx = crate::commands::project_context::resolve();
    let proj_by_id = crate::commands::project_context::load_proj_by_id();
    let drafts = crate::commands::bundle::get_available_drafts_pub(&ctx, "", &proj_by_id)
        .unwrap_or_default();
    let (display_drafts, hidden) = crate::commands::helpers::cap_recent_drafts(
        drafts,
        crate::commands::helpers::DEFAULT_RECENT_DRAFTS_CAP,
    );
    let focus = display_drafts.len().saturating_sub(1);

    let reload_ctx = ctx.clone();
    let reload_proj_by_id = proj_by_id.clone();
    let reloader: Box<dyn Fn() -> Vec<crate::store::DraftRecord>> = Box::new(move || {
        match crate::commands::bundle::get_available_drafts_pub(&reload_ctx, "", &reload_proj_by_id)
        {
            Ok(v) => {
                crate::commands::helpers::cap_recent_drafts(
                    v,
                    crate::commands::helpers::DEFAULT_RECENT_DRAFTS_CAP,
                )
                .0
            }
            Err(_) => Vec::new(),
        }
    });

    match crate::tui::screens::show::run_focused_with_reload(
        display_drafts,
        focus,
        hidden,
        initial_view,
        None,
        Some(reloader),
    ) {
        Ok(t) => t,
        Err(_) => NavTarget::Quit,
    }
}

fn spawn_all_sources() -> Vec<thread::JoinHandle<()>> {
    let a = crate::auth::load();
    let user_id = a.map(|a| a.user_id).unwrap_or_default();
    let mut handles = Vec::new();
    for src in sources::all() {
        let user_id = user_id.clone();
        handles.push(thread::spawn(move || {
            src.start(&user_id);
        }));
    }
    handles
}

fn wait_for_shutdown() {
    let flag = Arc::new(AtomicBool::new(false));
    let flag_handler = flag.clone();
    let _ = ctrlc::set_handler(move || {
        flag_handler.store(true, Ordering::SeqCst);
    });
    while !flag.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(200));
    }
}

struct PidFileGuard(PathBuf);
impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// A tiny libc shim so we don't depend on the full `libc` crate for just
// two symbols. Compiled only on Unix.
#[cfg(unix)]
mod libc_stub {
    #[allow(non_camel_case_types)]
    pub type pid_t = i32;

    pub const SIGTERM: i32 = 15;
    pub const ESRCH: i32 = 3;

    extern "C" {
        pub fn kill(pid: pid_t, sig: i32) -> i32;
    }
}

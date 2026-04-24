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
        let _ = crate::tui::screens::start::run(projs.len());
    } else {
        display::print_startup_banner(crate::VERSION, crate::BUILD_TIME, projs.len());
        let _ = spawn_all_sources();
        wait_for_shutdown();
        display::eprintln("\nPCR: Watcher stopped.");
    }

    drop(pid_guard);
    ExitCode::Success
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

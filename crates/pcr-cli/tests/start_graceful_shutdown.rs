//! Integration test for the audit's task 1: `pcr start` must shut
//! down gracefully on SIGINT.
//!
//! Before this fix the source watcher threads ran a bare `loop {
//! sleep; scan() }` with no cooperative shutdown signal. Ctrl-C
//! killed the process mid-scan and the PID file was racey to remove.
//! Now `pcr start` installs a Ctrl-C handler that flips a process-
//! wide `crate::shutdown` flag; scan loops poll it at the top of
//! every iteration; `wait_for_shutdown` returns; the `PidFileGuard`
//! drop removes the PID file before the binary exits cleanly with
//! code 0.
//!
//! Unix-only — `nix`/`libc::kill(pid, SIGINT)` is the natural way to
//! send the signal, and the `ctrlc` crate's Windows path is a
//! console-mode handler that doesn't translate to a clean test
//! shape. Windows ungraceful-shutdown is a separate concern.

#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

use common::home_fixture;

#[test]
fn pcr_start_exits_zero_and_removes_pid_file_on_sigint() {
    let fx = home_fixture();
    let pid_file = fx.pcr_dir().join("watcher.pid");
    assert!(!pid_file.exists(), "fresh fixture has no PID file yet");

    // Spawn `pcr start --plain` headless in the fixture's $HOME. We
    // need a `std::process::Child` (not `assert_cmd::Command`) so we
    // can grab the PID and send a signal to it directly.
    let bin = assert_cmd::cargo::cargo_bin("pcr");
    let mut child = std::process::Command::new(&bin)
        .arg("start")
        .env("HOME", fx.home_path())
        .env("USERPROFILE", fx.home_path())
        // Disable TUI: `is_tui_eligible` already returns false because
        // stderr is piped, but set NO_COLOR for belt-and-braces in case
        // the binary's env carries a real CI/terminal value through.
        .env("NO_COLOR", "1")
        .env_remove("CI")
        .env_remove("CURSOR_AGENT")
        .current_dir(fx.cwd_path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn pcr start");

    // Wait for the PID file to appear, capped at 10 s. The binary
    // writes it before `wait_for_shutdown()` so it's the simplest
    // readiness signal — without it, we might send SIGINT during
    // startup, before the ctrlc handler is installed.
    let pid = child.id() as i32;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pid_file.exists() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("PID file never appeared at {pid_file:?} — pcr start may have crashed");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Send SIGINT, exactly as `kill -INT <pid>` would.
    // SAFETY: `pid` is the child we just spawned; kill(2) with SIGINT
    // is safe and has no other observable side effects in this test.
    let rc = unsafe { libc::kill(pid, libc::SIGINT) };
    assert_eq!(
        rc,
        0,
        "kill(2) syscall failed: {}",
        std::io::Error::last_os_error()
    );

    // Wait for graceful exit. 15 s gives the 200 ms ctrlc-poll loop +
    // any scan-in-progress + final flush plenty of headroom.
    let exit_deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(s) => break s,
            None => {
                if Instant::now() >= exit_deadline {
                    let _ = child.kill();
                    panic!("pcr start did not exit within 15s after SIGINT — graceful shutdown is broken");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };

    assert!(
        status.success(),
        "expected exit code 0, got {status:?} — SIGINT must produce a clean exit"
    );
    assert!(
        !pid_file.exists(),
        "PidFileGuard::drop must have removed {pid_file:?}; otherwise a stale PID file \
         confuses the next `pcr start` into prompting about an already-dead watcher"
    );
}

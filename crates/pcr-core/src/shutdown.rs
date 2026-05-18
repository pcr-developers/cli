//! Process-wide cooperative shutdown signal.
//!
//! `pcr start` installs a Ctrl-C handler in
//! [`crate::commands::start::wait_for_shutdown`] that calls
//! [`request_shutdown`]. Long-running scan loops in the source watchers
//! poll [`is_shutting_down`] at the top of each iteration so they unwind
//! cooperatively instead of being torn down mid-scan when the process
//! exits. The PID file is then cleaned up via the `PidFileGuard` Drop
//! impl, which is idempotent (a `let _ =` on `remove_file` — no panic
//! if the file is already gone, e.g. because a prior `pcr start` was
//! replaced).
//!
//! Long sleeps (e.g. the 20 s cursor poll loop) should call
//! [`sleep_unless_shutdown`] instead of `thread::sleep` so Ctrl-C is
//! observable within ~200 ms rather than only at the next scan boundary.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Returns true once `pcr start` has been asked to terminate. Cheap to
/// poll — single relaxed-ish atomic load.
pub fn is_shutting_down() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Flip the shutdown flag. Called by the Ctrl-C handler. Idempotent —
/// callers can invoke this multiple times safely.
pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// Sleep for up to `duration`, broken into 200 ms slices so Ctrl-C is
/// noticed promptly. Returns `false` the moment the shutdown flag
/// flips so callers can break out of their loop without first
/// completing the next scan iteration.
pub fn sleep_unless_shutdown(duration: Duration) -> bool {
    let slice = Duration::from_millis(200);
    let mut remaining = duration;
    while remaining > Duration::ZERO {
        if is_shutting_down() {
            return false;
        }
        let step = remaining.min(slice);
        thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
    !is_shutting_down()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The flag is a process-wide static. Tests inside the same binary
    // share it, so we serialize via a mutex and reset between cases
    // — otherwise an interleaved run would see the flag set by a
    // previous test.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static M: std::sync::Mutex<()> = std::sync::Mutex::new(());
        M.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reset() {
        SHUTDOWN.store(false, Ordering::SeqCst);
    }

    #[test]
    fn sleep_returns_quickly_when_shutdown_requested() {
        let _g = lock();
        reset();
        let handle = thread::spawn(|| {
            thread::sleep(Duration::from_millis(50));
            request_shutdown();
        });
        let start = std::time::Instant::now();
        let completed_full = sleep_unless_shutdown(Duration::from_secs(10));
        let elapsed = start.elapsed();
        handle.join().unwrap();

        assert!(
            !completed_full,
            "must report early-exit when shutdown was requested"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "must break out within ~one slice of the request, not wait \
             the full 10 s; elapsed = {elapsed:?}"
        );
    }

    #[test]
    fn sleep_completes_when_no_shutdown() {
        let _g = lock();
        reset();
        let completed_full = sleep_unless_shutdown(Duration::from_millis(50));
        assert!(
            completed_full,
            "no shutdown signal → sleep must complete fully"
        );
    }
}

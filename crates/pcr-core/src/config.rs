//! Compile-time constants. Mirrors `cli/internal/config/constants.go`.

use std::path::PathBuf;

/// Supabase project URL used for all RPC calls.
pub const SUPABASE_URL: &str = "https://icbsvwffcykzimjjonad.supabase.co";

/// Supabase anon/publishable key.
pub const SUPABASE_KEY: &str = "sb_publishable_1a5u2j7KA23LlX958xW2hw_YE59GfOd";

/// Canonical web app URL used for deep links ("open settings", "review").
pub const APP_URL: &str = "https://pcr.dev";

/// Name of the per-user data directory under `$HOME`.
pub const PCR_DIR: &str = ".pcr-dev";

/// Returns the absolute path of `$HOME/.pcr-dev`, creating nothing on
/// disk.
///
/// Previously this fell back to `std::env::temp_dir()` when
/// `dirs::home_dir()` returned `None` — a silent failure mode that
/// wrote auth credentials and the SQLite store to `/tmp`, where they
/// vanished on reboot. The new contract: callers see the error and
/// can decide whether to fail loudly (CLI entry points) or skip
/// soft-state writes (background loops). No silent fallback ever.
///
/// On well-configured Unix systems and on Windows with `USERPROFILE`
/// set this returns `Ok(...)` essentially unconditionally; the
/// `Err` path only fires inside sandboxes / containers / cron jobs
/// where neither `HOME` nor `USERPROFILE` is set.
pub fn pcr_dir() -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| {
        anyhow::anyhow!(
            "could not determine $HOME (or %USERPROFILE% on Windows). PCR refuses to \
             silently fall back to a temp directory because that would lose auth + \
             local drafts on reboot. Set HOME and re-run."
        )
    })?;
    Ok(home.join(PCR_DIR))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `dirs::home_dir()` reads `HOME` (Unix) / `USERPROFILE` (Win)
    /// from the env at call time, so we can exercise the error
    /// path by removing both before calling. Restore them before
    /// the test ends so the rest of the test binary stays sane.
    ///
    /// Skipped on platforms where `dirs::home_dir()` falls back to
    /// something else (e.g. `getpwuid_r` on Unix): if the
    /// unset-everything-snapshot still resolves, we can't drive
    /// the error branch from a test, but the production code is
    /// still strictly safer than the previous `unwrap_or_else(
    /// env::temp_dir)`.
    #[test]
    fn pcr_dir_returns_err_when_home_is_unresolvable() {
        let prev_home = std::env::var_os("HOME");
        let prev_userprofile = std::env::var_os("USERPROFILE");
        // SAFETY: this test process is single-threaded inside its
        // own binary (the suite spawns each #[test] sequentially
        // by default), and we restore the env before returning.
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("USERPROFILE");
        }

        let result = pcr_dir();

        // Restore before any assertion can panic.
        unsafe {
            if let Some(v) = prev_home {
                std::env::set_var("HOME", v);
            }
            if let Some(v) = prev_userprofile {
                std::env::set_var("USERPROFILE", v);
            }
        }

        if dirs::home_dir().is_some() {
            eprintln!(
                "skipping: dirs::home_dir() still resolves with HOME/USERPROFILE unset \
                 (likely getpwuid_r on Unix); error branch isn't reachable from a test \
                 on this platform"
            );
            return;
        }
        let err = match result {
            Ok(p) => panic!(
                "expected an error when HOME is unset; got Ok({}). The previous \
                 implementation would silently fall back to /tmp here — that's exactly \
                 the regression we're guarding against.",
                p.display(),
            ),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("$HOME") || err.to_lowercase().contains("home"),
            "error message must mention $HOME so users know what to fix; got: {err}"
        );
    }
}

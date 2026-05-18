//! Best-effort "newer version available" notice on `pcr` runs.
//!
//! Modeled on the npm ecosystem's `update-notifier` package and
//! `cargo`'s own behaviour: at the start of an interactive command we
//! kick off a background thread that fetches `https://registry.npmjs.org/pcr-dev/latest`,
//! caches the result, and at the end of the command we print a soft
//! "X is available" notice to stderr if a newer version is published.
//!
//! Design constraints:
//!
//!   * **Never block the command.** Network failures, slow DNS, and
//!     captive portals can stall the registry request for seconds. The
//!     check runs on a detached thread; the foreground command never
//!     `join()`s it.
//!   * **No noise on failure.** Any error (network, parse, IO) just
//!     silently leaves the cache untouched. The next run tries again.
//!   * **No spam.** Two layers of throttling:
//!       1. The registry is hit at most once per `CACHE_TTL`. Between
//!          ticks we read the cached version.
//!       2. The notice is printed at most once per `NOTICE_INTERVAL`,
//!          so back-to-back `pcr log; pcr show` doesn't double-print.
//!   * **Respect machine output.** Skipped entirely for `--json`,
//!     the hidden `hook` subcommand, when `CI` is set, and when
//!     `PCR_NO_UPDATE_CHECK=1`.
//!   * **Install-method-aware suggestion.** Detects Homebrew vs npm by
//!     inspecting `current_exe()`, and prints the right upgrade
//!     command instead of a generic "go install it" link.
//!
//! Storage: `$PCR_DIR/update-check.json` (i.e. `~/.pcr-dev/update-check.json`).
//! Format is forward-compatible: unknown fields are ignored, missing
//! fields fall back to defaults.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const NOTICE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const NPM_LATEST_URL: &str = "https://registry.npmjs.org/pcr-dev/latest";

#[derive(Debug, Default, Serialize, Deserialize)]
struct CachedCheck {
    /// Unix seconds — last time we hit the registry.
    #[serde(default)]
    last_check_unix: u64,
    /// Unix seconds — last time we showed the user the notice. Used
    /// to throttle back-to-back commands.
    #[serde(default)]
    last_notice_unix: u64,
    /// Latest version string seen from the registry, e.g. `"0.3.0"`.
    #[serde(default)]
    latest_version: String,
}

#[derive(Debug, Deserialize)]
struct NpmDistTag {
    version: String,
}

fn cache_path() -> Option<PathBuf> {
    // `pcr_dir()` returns `Result<PathBuf>` and errors when neither
    // `$HOME` nor `%USERPROFILE%` resolves (sandboxes / locked-down
    // containers). The whole update-notifier module is best-effort,
    // so we collapse the Err into None and silently skip every cache
    // operation rather than propagating up to the foreground command.
    crate::config::pcr_dir()
        .ok()
        .map(|p| p.join("update-check.json"))
}

fn load_cache() -> CachedCheck {
    let Some(path) = cache_path() else {
        return CachedCheck::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return CachedCheck::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_cache(cache: &CachedCheck) {
    let Some(path) = cache_path() else { return };
    // Make sure the parent dir exists. We don't `?` this because the
    // entire module is best-effort.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(cache) {
        let _ = std::fs::write(&path, bytes);
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Returns true if `latest > current` under naive semver (`major.minor.patch`).
/// Pre-release suffixes are ignored — a `0.3.0-beta.1` versus `0.2.9` will
/// correctly resolve to "newer", but `0.3.0` vs `0.3.0-beta.1` resolves
/// equal (so we never tell a beta user to "upgrade" to the stable that
/// already shipped earlier).
fn semver_greater(latest: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<(u64, u64, u64)> {
        let core = v.split(|c: char| c == '-' || c == '+').next()?;
        let mut it = core.split('.');
        let a = it.next()?.parse().ok()?;
        let b = it.next()?.parse().ok()?;
        let c = it.next()?.parse().ok()?;
        Some((a, b, c))
    }
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallMethod {
    Homebrew,
    Npm,
    Unknown,
}

fn detect_install_method() -> InstallMethod {
    let Some(exe) = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_lowercase()))
    else {
        return InstallMethod::Unknown;
    };
    if exe.contains("/cellar/")
        || exe.contains("/opt/homebrew/")
        || exe.contains("/usr/local/cellar/")
        || exe.contains("homebrew")
    {
        return InstallMethod::Homebrew;
    }
    if exe.contains("/node_modules/") || exe.contains("/pcr-dev/bin/") {
        return InstallMethod::Npm;
    }
    InstallMethod::Unknown
}

fn upgrade_hint(method: InstallMethod) -> &'static str {
    match method {
        InstallMethod::Homebrew => "brew upgrade pcr",
        InstallMethod::Npm => "npm i -g pcr-dev@latest",
        InstallMethod::Unknown => "see https://pcr.dev/install",
    }
}

/// Should we skip the check entirely for this process? Honours
/// `PCR_NO_UPDATE_CHECK`, `CI`, and the `pcr_no_update_check`
/// dotenv-style key that some sandbox setups prefer.
fn should_skip_env() -> bool {
    if std::env::var_os("PCR_NO_UPDATE_CHECK").is_some() {
        return true;
    }
    if std::env::var_os("CI").is_some() {
        return true;
    }
    false
}

/// True for commands where a stderr notice would be noise rather than
/// signal. `hook` runs once per Claude Code Stop and the user never
/// sees its stderr; `mcp` is a stdio JSON-RPC channel.
fn is_quiet_subcommand(name: Option<&str>) -> bool {
    matches!(name, Some("hook") | Some("mcp"))
}

/// Fire-and-forget refresh. Spawns a detached thread that:
///   1. If the cached check is fresher than [`CACHE_TTL`], does nothing.
///   2. Otherwise hits the npm registry with a short timeout, parses
///      the `latest` dist-tag, and rewrites the cache file.
///
/// The thread is intentionally **not** joined. If the user's command
/// exits before the network call returns, the thread dies with the
/// process — net loss is a single missed cache update, and the next
/// run will retry.
///
/// Callers should invoke this near the top of `entry::run` so the
/// thread has the whole command duration to complete in the
/// background.
pub fn spawn_background_refresh(subcommand: Option<&str>, json_output: bool) {
    if should_skip_env() || json_output || is_quiet_subcommand(subcommand) {
        return;
    }
    let now = now_unix();
    let cache = load_cache();
    if now.saturating_sub(cache.last_check_unix) < CACHE_TTL.as_secs()
        && !cache.latest_version.is_empty()
    {
        // Cache is fresh — nothing to do.
        return;
    }

    std::thread::Builder::new()
        .name("pcr-update-check".into())
        .spawn(move || {
            let client = match reqwest::blocking::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .user_agent(concat!("pcr-update-check/", env!("CARGO_PKG_VERSION")))
                .build()
            {
                Ok(c) => c,
                Err(_) => return,
            };
            let Ok(resp) = client.get(NPM_LATEST_URL).send() else {
                return;
            };
            if !resp.status().is_success() {
                return;
            }
            let Ok(body) = resp.json::<NpmDistTag>() else {
                return;
            };
            let mut next = load_cache();
            next.last_check_unix = now_unix();
            next.latest_version = body.version;
            save_cache(&next);
        })
        // If we can't even spawn the thread (extremely rare —
        // resource exhaustion), drop silently.
        .ok();
}

/// Print the "newer version available" notice if appropriate. Reads
/// the cache that was populated by [`spawn_background_refresh`] —
/// either this run's, or any previous run's. Throttled to once per
/// [`NOTICE_INTERVAL`] to avoid double-printing on back-to-back
/// commands.
///
/// Callers should invoke this near the bottom of `entry::run`, just
/// before returning the exit code, so the notice sits *after* command
/// output rather than in front of it.
pub fn print_notice_if_due(subcommand: Option<&str>, json_output: bool) {
    if should_skip_env() || json_output || is_quiet_subcommand(subcommand) {
        return;
    }
    let mut cache = load_cache();
    let now = now_unix();
    if cache.latest_version.is_empty() {
        return;
    }
    if !semver_greater(&cache.latest_version, env!("CARGO_PKG_VERSION")) {
        return;
    }
    if now.saturating_sub(cache.last_notice_unix) < NOTICE_INTERVAL.as_secs() {
        return;
    }
    let method = detect_install_method();
    let upgrade = upgrade_hint(method);
    let current = env!("CARGO_PKG_VERSION");
    let latest = &cache.latest_version;
    // Plain text, no ANSI. Display is already colour-aware for the
    // command's own output; we don't want to fight `NO_COLOR` here.
    eprintln!();
    eprintln!("  ┌─ pcr {latest} is available (you have {current})");
    eprintln!("  └─ run: {upgrade}");
    eprintln!();
    cache.last_notice_unix = now;
    save_cache(&cache);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare_basic() {
        assert!(semver_greater("0.3.0", "0.2.9"));
        assert!(semver_greater("1.0.0", "0.99.99"));
        assert!(semver_greater("0.2.10", "0.2.9"));
        assert!(!semver_greater("0.2.9", "0.2.9"));
        assert!(!semver_greater("0.2.8", "0.2.9"));
    }

    #[test]
    fn semver_compare_handles_prerelease_suffix() {
        // Prereleases on either side strip down to their core triple;
        // we never *promote* a user to a prerelease version, but we
        // also don't accidentally demote-then-prompt.
        assert!(semver_greater("0.3.0-beta.1", "0.2.9"));
        assert!(!semver_greater("0.2.9-beta.1", "0.2.9"));
        assert!(!semver_greater("0.2.9", "0.2.9-beta.1"));
    }

    #[test]
    fn semver_compare_rejects_malformed() {
        assert!(!semver_greater("garbage", "0.2.9"));
        assert!(!semver_greater("0.2.9", "garbage"));
        assert!(!semver_greater("", ""));
        assert!(!semver_greater("0.3", "0.2.9"));
    }

    #[test]
    fn cache_roundtrip_through_serde() {
        let c = CachedCheck {
            last_check_unix: 12345,
            last_notice_unix: 23456,
            latest_version: "0.99.0".into(),
        };
        let bytes = serde_json::to_vec(&c).unwrap();
        let back: CachedCheck = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.last_check_unix, 12345);
        assert_eq!(back.last_notice_unix, 23456);
        assert_eq!(back.latest_version, "0.99.0");
    }

    #[test]
    fn cache_decodes_partial_legacy_payload() {
        // Forward-compat: an older cache file might not have the
        // `last_notice_unix` field. Deserialisation must default it
        // rather than erroring, otherwise a CLI upgrade silently
        // disables the notice forever.
        let payload = br#"{"last_check_unix":1,"latest_version":"9.9.9"}"#;
        let back: CachedCheck = serde_json::from_slice(payload).unwrap();
        assert_eq!(back.last_check_unix, 1);
        assert_eq!(back.last_notice_unix, 0);
        assert_eq!(back.latest_version, "9.9.9");
    }

    #[test]
    fn quiet_subcommands_skip() {
        assert!(is_quiet_subcommand(Some("hook")));
        assert!(is_quiet_subcommand(Some("mcp")));
        assert!(!is_quiet_subcommand(Some("status")));
        assert!(!is_quiet_subcommand(Some("log")));
        assert!(!is_quiet_subcommand(None));
    }
}

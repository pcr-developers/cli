// Each integration test under `tests/` compiles as its own binary, so
// not every helper here will be referenced by every binary. Suppress
// the unused-helper warnings instead of sprinkling per-fn `allow`s.
#![allow(dead_code)]

//! Shared integration-test harness for the `pcr` binary.
//!
//! Every integration test in this crate spawns the built `pcr` binary
//! through `assert_cmd`. The factory below bundles the env scrubbing
//! and `$HOME` / `cwd` isolation each test needs so we don't drift
//! between tests when the rules of "what env makes pcr deterministic"
//! evolve.

use assert_cmd::Command;
use tempfile::TempDir;

/// Build a freshly scrubbed `pcr` command bound to an isolated `$HOME`.
///
/// The temp dir's lifetime is returned alongside the command — drop it
/// at the end of the test or the dir disappears mid-run on platforms
/// that reap deleted directories aggressively.
pub fn pcr() -> (Command, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let cmd = build(&tmp);
    (cmd, tmp)
}

/// Two-temp-dir bundle: one for `$HOME` (where `.pcr-dev/` lives), one
/// for the spawned process's cwd. Splitting them keeps the registered
/// project state separate from the working directory we're pretending
/// to be inside — useful when a test needs to assert that `pcr` reads
/// state from `$HOME` while running in an unrelated repo.
pub struct HomeFixture {
    pub home: TempDir,
    pub cwd: TempDir,
}

impl HomeFixture {
    pub fn home_path(&self) -> &std::path::Path {
        self.home.path()
    }
    pub fn cwd_path(&self) -> &std::path::Path {
        self.cwd.path()
    }
    pub fn pcr_dir(&self) -> std::path::PathBuf {
        self.home.path().join(".pcr-dev")
    }
}

/// Build a `HomeFixture` with `$HOME/.pcr-dev/` pre-created so commands
/// don't have to mkdir on first boot.
pub fn home_fixture() -> HomeFixture {
    let home = TempDir::new().expect("home tempdir");
    let cwd = TempDir::new().expect("cwd tempdir");
    std::fs::create_dir_all(home.path().join(".pcr-dev")).expect("pre-create pcr-dev");
    HomeFixture { home, cwd }
}

/// Wire a `pcr` command against the given fixture: isolated `$HOME`,
/// the fixture's `cwd` as the process's working directory, and the
/// usual env scrubbing.
pub fn pcr_in(fx: &HomeFixture) -> Command {
    let mut cmd = build(&fx.home);
    cmd.current_dir(fx.cwd.path());
    cmd
}

fn build(home: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("pcr").expect("binary built");
    cmd.env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env_remove("CI")
        .env_remove("NO_COLOR")
        .env_remove("CURSOR_AGENT");
    cmd
}

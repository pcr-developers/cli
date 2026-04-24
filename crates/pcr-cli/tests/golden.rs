//! End-to-end golden tests. Each test spawns the real built `pcr` binary
//! and asserts on stdout / stderr / exit code.
//!
//! These tests deliberately exercise the *output surface* — not internal
//! logic — so they double as a regression guard that the Rust build
//! remains byte-compatible with the Go build in `--plain`/`--json` mode.
//! The Go CLI's stdout for the same inputs is expected to differ only in
//! trailing whitespace, which assert_cmd's `contains` predicates tolerate.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// Build a cmd bound to an isolated `$HOME` so the test doesn't touch the
/// developer's real `~/.pcr-dev/` state.
fn pcr() -> (Command, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let mut cmd = Command::cargo_bin("pcr").expect("binary built");
    cmd.env("HOME", tmp.path())
        .env("USERPROFILE", tmp.path())
        .env_remove("CI")
        .env_remove("NO_COLOR")
        .env_remove("CURSOR_AGENT");
    (cmd, tmp)
}

#[test]
fn help_mentions_pcr_dev_and_every_subcommand() {
    let (mut cmd, _tmp) = pcr();
    let out = cmd.arg("--help").assert().success();
    let text = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    assert!(text.contains("PCR.dev"), "banner missing: {text}");
    for sub in [
        "login", "logout", "init", "start", "mcp", "status", "bundle", "push", "log", "show",
        "pull", "gc",
    ] {
        assert!(text.contains(sub), "missing subcommand {sub}:\n{text}");
    }
}

#[test]
fn version_tag_is_runtime_aware() {
    let (mut cmd, _tmp) = pcr();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("(rust)"));
}

#[test]
fn logout_without_state_succeeds_and_prints_confirmation() {
    let (mut cmd, _tmp) = pcr();
    cmd.arg("--plain")
        .arg("logout")
        .assert()
        .success()
        .stderr(predicate::str::contains("Logged out"));
}

#[test]
fn mcp_command_exits_not_implemented() {
    let (mut cmd, _tmp) = pcr();
    cmd.arg("--plain")
        .arg("mcp")
        .assert()
        .code(50)
        .stderr(predicate::str::contains("not yet implemented"));
}

#[test]
fn status_plain_without_login_mentions_login_hint() {
    let (mut cmd, _tmp) = pcr();
    cmd.arg("--plain")
        .arg("status")
        .assert()
        .success()
        .stderr(predicate::str::contains("Not logged in"))
        .stderr(predicate::str::contains("No projects registered"));
}

#[test]
fn status_json_returns_structured_blob() {
    let (mut cmd, _tmp) = pcr();
    let out = cmd
        .arg("--json")
        .arg("status")
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("expected JSON, got {stdout:?}: {e}"));
    assert!(parsed.is_object());
    let obj = parsed.as_object().unwrap();
    for key in [
        "logged_in",
        "user_id",
        "projects",
        "unpushed_bundles",
        "draft_count",
    ] {
        assert!(obj.contains_key(key), "missing key {key}: {stdout}");
    }
    assert_eq!(obj["logged_in"], serde_json::Value::Bool(false));
}

#[test]
fn gc_with_invalid_older_than_exits_usage() {
    let (mut cmd, _tmp) = pcr();
    cmd.arg("--plain")
        .arg("gc")
        .arg("--older-than")
        .arg("banana")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid --older-than"));
}

#[test]
fn show_with_bad_number_exits_usage() {
    let (mut cmd, _tmp) = pcr();
    cmd.arg("--plain")
        .arg("show")
        .arg("not-a-number")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid number"));
}

#[test]
fn pull_without_auth_exits_auth_required() {
    let (mut cmd, _tmp) = pcr();
    cmd.arg("--plain")
        .arg("pull")
        .arg("some-remote-id")
        .assert()
        .code(10)
        .stderr(predicate::str::contains("not logged in"));
}

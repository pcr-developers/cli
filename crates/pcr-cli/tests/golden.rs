//! End-to-end golden tests. Each test spawns the real built `pcr` binary
//! and asserts on stdout / stderr / exit code.
//!
//! These tests deliberately exercise the *output surface* — not internal
//! logic — so they double as a regression guard that the Rust build
//! remains byte-compatible with the Go build in `--plain`/`--json` mode.
//! The Go CLI's stdout for the same inputs is expected to differ only in
//! trailing whitespace, which assert_cmd's `contains` predicates tolerate.

mod common;

use common::pcr;
use predicates::prelude::*;
use tempfile::TempDir;

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
        .stderr(predicate::str::contains("invalid draft number"));
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

#[test]
fn log_json_empty_store_is_stable() {
    let (mut cmd, _tmp) = pcr();
    // Pin cwd to a fresh non-project temp dir so `resolve()` returns an
    // empty context and the empty-state branch of `log.rs` runs.
    let cwd = TempDir::new().expect("cwd tempdir");
    let out = cmd
        .arg("--json")
        .arg("log")
        .current_dir(cwd.path())
        .assert()
        .success()
        .get_output()
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        let s = String::from_utf8_lossy(&out.stdout);
        panic!("expected JSON, got {s:?}: {e}")
    });
    let obj = parsed
        .as_object()
        .unwrap_or_else(|| panic!("expected JSON object, got {parsed}"));
    for key in ["project_name", "pushed", "unpushed", "drafts"] {
        assert!(obj.contains_key(key), "missing key {key}: {parsed}");
    }
    for key in ["pushed", "unpushed", "drafts"] {
        let arr = obj[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} not an array: {parsed}"));
        assert!(arr.is_empty(), "{key} expected empty, got {arr:?}");
    }
}

#[test]
fn log_plain_empty_store_messaging() {
    let (mut cmd, _tmp) = pcr();
    let cwd = TempDir::new().expect("cwd tempdir");
    // `NO_COLOR=1` strips ANSI escapes from `display::cstr` so the
    // substring match below stays stable across terminals.
    cmd.env("NO_COLOR", "1")
        .arg("--plain")
        .arg("log")
        .current_dir(cwd.path())
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "no project is registered for this directory",
        ));
}

// ─── Exit-code matrix gap-fillers (Step 5) ──────────────────────────────────

#[test]
fn push_without_auth_exits_auth_required() {
    // `pcr push` reaches Supabase, which requires an auth token; with
    // an empty `~/.pcr-dev/auth.json` we should bail with the
    // AuthRequired code (10) before any network call.
    let (mut cmd, _tmp) = pcr();
    cmd.arg("--plain")
        .arg("push")
        .assert()
        .code(10)
        .stderr(predicate::str::contains("Not logged in"));
}

#[test]
fn bundle_with_unknown_flag_exits_usage() {
    // clap rejects unknown args with exit code 2 (Usage) — pin the code
    // so a refactor that swaps clap for hand-rolled parsing keeps the
    // contract.
    let (mut cmd, _tmp) = pcr();
    cmd.arg("--plain")
        .arg("bundle")
        .arg("--nonexistent-flag")
        .assert()
        .code(2);
}

#[test]
fn bundle_delete_without_name_exits_usage() {
    let (mut cmd, _tmp) = pcr();
    cmd.arg("--plain")
        .arg("bundle")
        .arg("--delete")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--delete requires a bundle name"));
}

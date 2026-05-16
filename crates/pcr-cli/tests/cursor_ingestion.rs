//! Cursor ingestion → drafts integration test (partial).
//!
//! ## Status: harness smoke + registered-project log roundtrip
//!
//! The original Step 1 plan asked for a full e2e test driving a Cursor
//! transcript through `pcr --plain bundle` into the drafts store, then
//! re-reading via `pcr --json log`. That turned out to be intractable
//! to fixture from a subprocess:
//!
//! - `crates/pcr-core/src/sources/cursor/watcher.rs` only iterates
//!   `~/.cursor/projects/<slug>/agent-transcripts/*.jsonl` to discover
//!   *session ids*. The actual prompt + response payloads live in
//!   Cursor's `state.vscdb` SQLite database
//!   (`~/Library/Application Support/Cursor/User/globalStorage/state.vscdb`
//!   on macOS, `~/.config/Cursor/...` on Linux, `%APPDATA%/Cursor/...` on
//!   Windows), parsed by `sources/cursor/db.rs::get_session_meta` from
//!   the `cursorDiskKV` key-value table with `composerData:<sid>` and
//!   `bubbleId:<composerId>:<bubbleId>` rows.
//! - Faking that DB inside a subprocess test is brittle: it requires
//!   replicating Cursor's exact JSON-in-SQLite schema, OS-specific path
//!   resolution, and the `headers_only` + per-bubble fan-out. Drift on
//!   their side silently makes our test green for the wrong reason.
//!
//! The pragmatic fallback that lives below exercises:
//! - The shared `common` harness (HOME + cwd isolation).
//! - `projects.json` seeding (no `pcr init` needed — register direct).
//! - `pcr --json log` resolving the cwd to the registered project.
//!
//! TODO(repair/follow-up): if we ever need true Cursor ingestion
//! coverage, the right shape is a `crates/pcr-core/tests/`-style test
//! that calls `sources/cursor/watcher.rs::PromptScanner` directly with
//! a hand-built `state.vscdb` fixture, NOT a subprocess test through
//! `pcr bundle`.

mod common;

use common::{home_fixture, pcr_in};

fn seed_projects_json(fx: &common::HomeFixture, name: &str) {
    // Match the on-disk shape `crates/pcr-core/src/projects.rs::Registry`
    // serializes: `{ "projects": [ { path, cursorSlug, claudeSlug, name,
    // registeredAt } ] }`. Keep the camelCase keys — the registry is
    // shared with the (archived) Go build's `projects.json`.
    //
    // Canonicalize the cwd path: `tempfile::TempDir` on macOS hands back
    // `/var/folders/...` but `pcr`'s `std::env::current_dir()` resolves
    // through the `/private` symlink and reports `/private/var/...`.
    // `project_context::resolve` does a literal string compare against
    // `projects.json::path`, so we must write the post-symlink form.
    let path = std::fs::canonicalize(fx.cwd_path())
        .expect("canonicalize cwd")
        .to_string_lossy()
        .into_owned();
    let cursor_slug = path.trim_start_matches('/').replace(['/', '.'], "-");
    let claude_slug = path.replace('/', "-");
    let registry = serde_json::json!({
        "projects": [{
            "path": path,
            "cursorSlug": cursor_slug,
            "claudeSlug": claude_slug,
            "name": name,
            "registeredAt": "2026-01-01T00:00:00Z",
        }]
    });
    let projects_path = fx.pcr_dir().join("projects.json");
    std::fs::write(
        &projects_path,
        serde_json::to_vec_pretty(&registry).unwrap(),
    )
    .expect("write projects.json");
}

#[test]
fn registered_project_log_returns_named_empty_store() {
    let fx = home_fixture();
    seed_projects_json(&fx, "test-proj");

    let out = pcr_in(&fx)
        .args(["--json", "log"])
        .assert()
        .success()
        .get_output()
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        let s = String::from_utf8_lossy(&out.stdout);
        panic!("expected JSON, got {s:?}: {e}")
    });

    assert_eq!(parsed["project_name"], "test-proj");
    for key in ["pushed", "unpushed", "drafts"] {
        let arr = parsed[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} not an array: {parsed}"));
        assert!(arr.is_empty(), "{key} expected empty, got {arr:?}");
    }
}

#[test]
fn registered_project_log_is_idempotent_across_runs() {
    let fx = home_fixture();
    seed_projects_json(&fx, "idem-proj");

    let first = pcr_in(&fx)
        .args(["--json", "log"])
        .assert()
        .success()
        .get_output()
        .clone()
        .stdout;
    let second = pcr_in(&fx)
        .args(["--json", "log"])
        .assert()
        .success()
        .get_output()
        .clone()
        .stdout;

    // Byte-identical (or at least the parsed JSON matches). Use parsed
    // comparison since the json! pretty-printer may shift trailing
    // whitespace between runs.
    let a: serde_json::Value = serde_json::from_slice(&first).unwrap();
    let b: serde_json::Value = serde_json::from_slice(&second).unwrap();
    assert_eq!(a, b, "log JSON should be stable across runs");
    assert_eq!(a["project_name"], "idem-proj");
}

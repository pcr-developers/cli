//! State-cursor ordering for `claudecode::watcher::process_file`.
//!
//! Regression test for the audit's task 6: `state.set(file_path, lines)`
//! used to run BEFORE `parse_claude_code_session`. If the parser
//! happened to fail (or even just extract zero prompts) the watcher
//! still advanced its line-count cursor — so on the next scan, none of
//! the previously-unprocessed lines would ever be re-examined.
//!
//! The fix is to delay `state.set` until parse produced at least one
//! prompt. This test exercises that contract with a real on-disk
//! JSONL file, the real `process_file` entry point, and a real
//! `$HOME/.pcr-dev` store — no mocks.
//!
//! Single test per file: the in-process SQLite singleton in
//! `crates/pcr-core/src/store/db.rs` survives across tests inside the
//! same integration binary, which would otherwise contaminate state
//! between cases.

use pcr_core::projects;
use pcr_core::sources::claudecode::watcher::process_file;
use pcr_core::sources::shared::{Deduplicator, FileState};
use tempfile::TempDir;

#[test]
fn parse_failure_leaves_state_cursor_unchanged() {
    let home = TempDir::new().expect("home tempdir");
    // SAFETY: this integration test binary contains exactly one #[test];
    // cargo runs each integration test binary in its own process; no
    // other thread can observe the env mutation.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("USERPROFILE", home.path());
    }
    std::fs::create_dir_all(home.path().join(".pcr-dev")).expect("mkdir pcr-dev");

    // Register a project whose claude_slug matches the synthetic
    // `~/.claude/projects/<slug>/` directory we're about to create.
    // `process_file` early-returns if the slug isn't registered, so
    // without this step the function would skip everything before
    // even touching the state cursor.
    let project_path = home.path().join("workspace-state-ordering");
    std::fs::create_dir_all(&project_path).expect("mkdir project");
    let registered = projects::register(&project_path.to_string_lossy());
    assert!(
        !registered.claude_slug.is_empty(),
        "register must populate claude_slug"
    );

    // Build the synthetic transcript path the watcher would have seen.
    let claude_dir = home
        .path()
        .join(".claude")
        .join("projects")
        .join(&registered.claude_slug);
    std::fs::create_dir_all(&claude_dir).expect("mkdir claude transcripts dir");
    let file_path = claude_dir.join("session-corrupt.jsonl");

    // Corrupt JSONL: real bytes (so the line count is non-zero), but
    // every line fails `serde_json::from_str` → parser returns an
    // empty session. Three lines so `count_non_empty_lines` returns 3.
    std::fs::write(
        &file_path,
        b"not json line 1\nnot json line 2\nnot json line 3\n",
    )
    .expect("write corrupt transcript");

    let state = FileState::new("claude-code-state-ordering-test");
    let dedup = Deduplicator::new();

    // Sanity-check the precondition: cursor is at zero before the
    // first call.
    let key = file_path.to_string_lossy().into_owned();
    assert_eq!(state.get(&key), 0, "fresh state starts at 0");

    process_file(&key, "", &state, &dedup, false);

    assert_eq!(
        state.get(&key),
        0,
        "parse extracted no prompts → state cursor must NOT advance, \
         otherwise a transient parse failure silently drops lines"
    );

    // Replace the corrupt file with a parseable transcript — one user
    // message that the parser will surface as a prompt. After this
    // call, `process_file` must advance the cursor because there's
    // now something legitimate to attribute.
    let session_id = "claude-session-state-ordering-001";
    let valid_line = serde_json::json!({
        "type": "user",
        "sessionId": session_id,
        "timestamp": "2026-05-18T00:00:00.000Z",
        "gitBranch": "main",
        "message": {
            "role": "user",
            "content": "do the thing",
        },
    });
    let valid_assistant = serde_json::json!({
        "type": "assistant",
        "sessionId": session_id,
        "timestamp": "2026-05-18T00:00:01.000Z",
        "message": {
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [{"type": "text", "text": "ok"}],
        },
    });
    let body = format!("{}\n{}\n", valid_line, valid_assistant);
    std::fs::write(&file_path, body.as_bytes()).expect("rewrite transcript");

    process_file(&key, "", &state, &dedup, false);
    assert!(
        state.get(&key) > 0,
        "after a parse that yielded prompts, state cursor must advance"
    );

    // Sanity: a forced re-scan over the same path is still idempotent
    // — the cursor stays at the line count, doesn't somehow regress.
    let advanced = state.get(&key);
    process_file(&key, "", &state, &dedup, false);
    assert_eq!(
        state.get(&key),
        advanced,
        "re-running on identical content must not move the cursor backwards"
    );
}

//! Claude Code transcript → draft store fixture roundtrip.
//!
//! Runs the real `parse_claude_code_session` parser on a frozen JSONL
//! fixture and saves the result via `store::drafts::save_draft` (the
//! same public API the watcher uses). Then re-queries the store and
//! asserts the row is intact and stable across a second save (idempotent
//! dedupe via `content_hash`).
//!
//! Single test per file: the in-process SQLite singleton in
//! `crates/pcr-core/src/store/db.rs` would otherwise survive between
//! tests inside the same integration binary.

use std::path::PathBuf;

use pcr_core::sources::claudecode::parser::parse_claude_code_session;
use pcr_core::store;
use pcr_core::supabase::{prompt_content_hash_v2, prompt_id_v2, PromptRecord};
use tempfile::TempDir;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("claude")
        .join("minimal_session.jsonl")
}

#[test]
fn claude_fixture_parses_and_persists_through_store() {
    // Isolate HOME so `$HOME/.pcr-dev/drafts.db` lands in a tempdir
    // and we don't touch the developer's real store. Must happen
    // BEFORE the first `store::open()` because the singleton caches
    // the resolved path.
    let home = TempDir::new().expect("home tempdir");
    // SAFETY: this integration test binary contains exactly one #[test];
    // cargo runs each integration test binary in its own process; no
    // other thread can observe the env mutation.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("USERPROFILE", home.path());
    }
    std::fs::create_dir_all(home.path().join(".pcr-dev")).expect("mkdir pcr-dev");

    let body = std::fs::read_to_string(fixture_path()).expect("fixture readable");
    let parsed =
        parse_claude_code_session(&body, "fixture-proj", &fixture_path().to_string_lossy());

    assert_eq!(parsed.session_id, "claude-fixture-001");
    assert_eq!(parsed.prompts.len(), 1);
    let prompt = &parsed.prompts[0];
    assert_eq!(
        prompt.prompt_text,
        "refactor extract_text to drop empty strings"
    );
    assert!(prompt.response_text.contains("Filtering blank chunks now"));

    // The parser leaves `id` and `content_hash` empty — `save_draft`
    // computes them. Pre-compute the expected hash so we can re-query
    // and verify stability across the two saves.
    let expected_hash =
        prompt_content_hash_v2(&prompt.session_id, &prompt.prompt_text, &prompt.captured_at);
    let mut record_for_save = PromptRecord {
        content_hash: expected_hash.clone(),
        id: prompt_id_v2(&prompt.session_id, &prompt.prompt_text, &prompt.captured_at),
        ..prompt.clone()
    };
    record_for_save.project_id = "p-claude-fixture".into();

    store::save_draft(&record_for_save, &[], "", "").expect("first save");

    let drafts =
        store::get_drafts_by_status(store::DraftStatus::Draft, &[], &[]).expect("re-query drafts");
    assert_eq!(drafts.len(), 1, "exactly one draft after save");
    let saved = &drafts[0];
    assert_eq!(saved.content_hash, expected_hash);
    assert_eq!(saved.session_id, "claude-fixture-001");
    assert_eq!(saved.source, "claude-code");
    assert!(!saved.id.is_empty(), "id populated");
    // file_context should carry at least the tool_results we saw in
    // the fixture (one Edit tool_use → one tool_result).
    let fc = saved.file_context.as_ref().expect("file_context populated");
    assert!(
        fc.contains_key("tool_results"),
        "tool_results missing from file_context: {fc:?}"
    );

    // Idempotent dedupe: a second save of the same record must not
    // produce a duplicate row (content_hash UNIQUE) AND must leave the
    // existing row's hash unchanged.
    store::save_draft(&record_for_save, &[], "", "").expect("idempotent save");
    let drafts_after = store::get_drafts_by_status(store::DraftStatus::Draft, &[], &[])
        .expect("re-query drafts after second save");
    assert_eq!(drafts_after.len(), 1, "no duplicate after second save");
    assert_eq!(
        drafts_after[0].content_hash, expected_hash,
        "hash must remain stable"
    );
}

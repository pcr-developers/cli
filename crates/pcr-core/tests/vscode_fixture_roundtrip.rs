//! VS Code Copilot Chat chatSession → draft store fixture roundtrip.
//!
//! Exercises `parse_chatsession` → `exchange_to_prompt_record` →
//! `store::drafts::save_draft`. Mirrors the Claude Code fixture test
//! but on the new CRDT-style `chatSessions/` format.

use std::path::PathBuf;

use pcr_core::sources::vscode::chatsession_parser::parse_chatsession;
use pcr_core::sources::vscode::parser::exchange_to_prompt_record;
use pcr_core::store;
use pcr_core::supabase::{prompt_content_hash_v2, prompt_id_v2};
use tempfile::TempDir;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("vscode")
        .join("minimal_chatsession.jsonl")
}

#[test]
fn vscode_fixture_parses_and_persists_through_store() {
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
    let transcript = parse_chatsession(&body);

    assert_eq!(transcript.session_id, "vscode-fixture-001");
    assert_eq!(transcript.exchanges.len(), 1);
    let ex = &transcript.exchanges[0];
    assert_eq!(ex.prompt_text, "summarize the failing test");
    assert!(ex.response_text.contains("clock_override is None"));
    assert_eq!(ex.relevant_files, vec!["/repo/src/main.rs"]);

    let mut record = exchange_to_prompt_record(
        ex,
        &transcript.session_id,
        "fixture-proj",
        "p-vscode-fixture",
        "main",
    );
    // The parser does not seed `id` / `content_hash`; mirror the
    // production watcher path that fills them in before save.
    let expected_hash =
        prompt_content_hash_v2(&record.session_id, &record.prompt_text, &record.captured_at);
    record.content_hash = expected_hash.clone();
    record.id = prompt_id_v2(&record.session_id, &record.prompt_text, &record.captured_at);

    store::save_draft(&record, &[], "", "").expect("first save");

    let drafts =
        store::get_drafts_by_status(store::DraftStatus::Draft, &[], &[]).expect("re-query drafts");
    assert_eq!(drafts.len(), 1);
    let saved = &drafts[0];
    assert_eq!(saved.content_hash, expected_hash);
    assert_eq!(saved.session_id, "vscode-fixture-001");
    assert_eq!(saved.source, "vscode");
    assert_eq!(saved.project_id, "p-vscode-fixture");
    assert_eq!(saved.branch_name, "main");
    let fc = saved.file_context.as_ref().expect("file_context populated");
    assert!(
        fc.contains_key("relevant_files"),
        "relevant_files missing: {fc:?}"
    );

    store::save_draft(&record, &[], "", "").expect("idempotent save");
    let drafts_after = store::get_drafts_by_status(store::DraftStatus::Draft, &[], &[])
        .expect("re-query drafts after second save");
    assert_eq!(drafts_after.len(), 1);
    assert_eq!(drafts_after[0].content_hash, expected_hash);
}

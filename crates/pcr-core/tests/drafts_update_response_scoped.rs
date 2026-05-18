//! Regression test for the audit's task 2: `update_draft_response`
//! used to issue an unscoped `UPDATE … WHERE session_id = ? AND
//! prompt_text = ?` which would silently overwrite EVERY draft with
//! the same (session, prompt) tuple. In practice Claude Code and
//! Cursor both let users re-send identical prompt text inside one
//! session (e.g. "go", "continue", "yes please"); the two distinct
//! drafts share `session_id` + `prompt_text` but live as separate
//! rows (distinct `content_hash` via the v2 hash, which folds in
//! `captured_at`).
//!
//! The fix selects the most recent matching id first, then updates
//! that single row. This test proves the older row is left intact.

use pcr_core::store::{self, DraftRecord, DraftStatus};
use pcr_core::supabase::{prompt_content_hash_v2, prompt_id_v2, PromptRecord};
use tempfile::TempDir;

fn save(session_id: &str, prompt_text: &str, captured_at: &str) -> String {
    let hash = prompt_content_hash_v2(session_id, prompt_text, captured_at);
    let rec = PromptRecord {
        id: prompt_id_v2(session_id, prompt_text, captured_at),
        content_hash: hash.clone(),
        session_id: session_id.to_string(),
        project_id: "p-scoped-update".into(),
        project_name: "p-scoped-update".into(),
        prompt_text: prompt_text.to_string(),
        source: "claude-code".into(),
        capture_method: "test".into(),
        captured_at: captured_at.to_string(),
        ..Default::default()
    };
    store::save_draft(&rec, &[], "", "").expect("save_draft");
    hash
}

fn find(captured_at: &str, drafts: &[DraftRecord]) -> DraftRecord {
    drafts
        .iter()
        .find(|d| d.captured_at == captured_at)
        .cloned()
        .unwrap_or_else(|| panic!("no draft at {captured_at} in {drafts:?}"))
}

#[test]
fn update_draft_response_only_touches_the_most_recent_match() {
    let home = TempDir::new().expect("home tempdir");
    // SAFETY: this integration test binary contains exactly one
    // #[test]; cargo runs each binary in its own process.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("USERPROFILE", home.path());
    }
    std::fs::create_dir_all(home.path().join(".pcr-dev")).expect("mkdir pcr-dev");

    let sid = "claude-session-dup-prompt-001";
    let prompt = "continue";

    // Two drafts with identical (session_id, prompt_text). The v2
    // hash folds `captured_at` in so the rows survive the
    // `content_hash UNIQUE` constraint independently.
    save(sid, prompt, "2026-05-18T00:00:00.000Z");
    save(sid, prompt, "2026-05-18T00:05:00.000Z");

    let before = store::get_drafts_by_status(DraftStatus::Draft, &[], &[]).expect("query");
    assert_eq!(before.len(), 2, "both drafts should be stored");
    let older_id = find("2026-05-18T00:00:00.000Z", &before).id.clone();
    let newer_id = find("2026-05-18T00:05:00.000Z", &before).id.clone();
    assert_ne!(older_id, newer_id, "rows must be distinct");

    store::update_draft_response(sid, prompt, "newer reply").expect("update");

    let after = store::get_drafts_by_status(DraftStatus::Draft, &[], &[]).expect("query");
    assert_eq!(after.len(), 2, "row count unchanged");
    let older = find("2026-05-18T00:00:00.000Z", &after);
    let newer = find("2026-05-18T00:05:00.000Z", &after);

    assert_eq!(
        newer.response_text, "newer reply",
        "most recent draft must receive the response"
    );
    assert_eq!(
        older.response_text, "",
        "older draft must be UNTOUCHED — the previous unscoped UPDATE \
         overwrote it, which is the regression we're guarding against"
    );

    // Second call must be idempotent: the existing response is no
    // shorter than what we'd write, so the LEN-filter selects no row
    // and we exit without an UPDATE. Older row stays empty either way.
    store::update_draft_response(sid, prompt, "newer reply").expect("idempotent update");
    let after2 = store::get_drafts_by_status(DraftStatus::Draft, &[], &[]).expect("query");
    let older2 = find("2026-05-18T00:00:00.000Z", &after2);
    let newer2 = find("2026-05-18T00:05:00.000Z", &after2);
    assert_eq!(newer2.response_text, "newer reply");
    assert_eq!(older2.response_text, "");

    // A *longer* response targeted at the same (session, prompt)
    // still scopes to the newest row, not the older one.
    store::update_draft_response(sid, prompt, "newer reply — extended").expect("update v2");
    let after3 = store::get_drafts_by_status(DraftStatus::Draft, &[], &[]).expect("query");
    let older3 = find("2026-05-18T00:00:00.000Z", &after3);
    let newer3 = find("2026-05-18T00:05:00.000Z", &after3);
    assert_eq!(newer3.response_text, "newer reply — extended");
    assert_eq!(
        older3.response_text, "",
        "older row must remain untouched even when the response grows"
    );
}

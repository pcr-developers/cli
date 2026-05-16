//! SQLite migration smoke test.
//!
//! Materializes a v1-schema `drafts.db` in an isolated `$HOME/.pcr-dev/`,
//! invokes `pcr --json log` (which opens the store and triggers the
//! migration ladder in `crates/pcr-core/src/store/db.rs::migrate`), and
//! re-queries the resulting DB to confirm:
//!
//! - `schema_version` advanced to 6.
//! - All v2..v6 column adds + table adds (`git_diff`, `head_sha`,
//!   `permission_mode`, `diff_events`, `session_state_events`,
//!   `saved_bubbles`, `bundle_status`) are present.
//! - The original v1 drafts survived.
//!
//! ## Fixture provenance
//!
//! The plan asked for a checked-in `tests/fixtures/db/drafts_v1.sqlite`
//! binary blob. We materialize it programmatically instead — same
//! observable outcome (a v1 DB that the migration ladder must lift),
//! without committing a binary that's hard to diff in code review. The
//! v1 schema SQL below is copy-pasted verbatim from `migrate_v1` as of
//! v0.2.8; future drift in `migrate_v1` will NOT affect this test
//! because the fixture's schema is frozen in this file.

mod common;

use common::home_fixture;
use rusqlite::Connection;

/// Original `migrate_v1` SQL, frozen at v0.2.8. Keep byte-identical with
/// the production code path that shipped in v1 of `drafts.db`.
const V1_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS drafts (
  id              TEXT PRIMARY KEY,
  content_hash    TEXT UNIQUE NOT NULL,
  session_id      TEXT NOT NULL,
  project_id      TEXT,
  project_name    TEXT NOT NULL,
  branch_name     TEXT,
  prompt_text     TEXT NOT NULL,
  response_text   TEXT,
  model           TEXT,
  source          TEXT NOT NULL,
  capture_method  TEXT NOT NULL,
  tool_calls      TEXT,
  file_context    TEXT,
  captured_at     TEXT NOT NULL,
  session_commit_shas TEXT,
  status          TEXT NOT NULL DEFAULT 'draft',
  created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS drafts_status   ON drafts(status);
CREATE INDEX IF NOT EXISTS drafts_project  ON drafts(project_id);
CREATE INDEX IF NOT EXISTS drafts_captured ON drafts(captured_at);

CREATE TABLE IF NOT EXISTS prompt_commits (
  id           TEXT PRIMARY KEY,
  message      TEXT NOT NULL,
  project_id   TEXT,
  project_name TEXT,
  branch_name  TEXT,
  session_shas TEXT,
  head_sha     TEXT NOT NULL,
  pushed_at    TEXT,
  remote_id    TEXT,
  committed_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS prompt_commit_items (
  prompt_commit_id TEXT NOT NULL REFERENCES prompt_commits(id),
  draft_id         TEXT NOT NULL REFERENCES drafts(id),
  PRIMARY KEY (prompt_commit_id, draft_id)
);

CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version (version) VALUES (1);
"#;

fn install_v1_drafts_db(db_path: &std::path::Path) {
    let conn = Connection::open(db_path).expect("create v1 drafts.db");
    conn.execute_batch(V1_SCHEMA_SQL).expect("apply v1 schema");
    // Two sample drafts so the post-migration query has rows to find.
    conn.execute(
        "INSERT INTO drafts (id, content_hash, session_id, project_name, prompt_text, source, capture_method, captured_at) \
         VALUES ('id-1', 'hash-1', 'sess-a', 'fixture-proj', 'how do I refactor this?', 'cursor', 'prompt-scanner', '2026-01-01T00:00:00Z')",
        [],
    ).expect("insert sample draft 1");
    conn.execute(
        "INSERT INTO drafts (id, content_hash, session_id, project_name, prompt_text, source, capture_method, captured_at) \
         VALUES ('id-2', 'hash-2', 'sess-b', 'fixture-proj', 'add a test', 'claude-code', 'file-watcher', '2026-01-02T00:00:00Z')",
        [],
    ).expect("insert sample draft 2");
}

#[test]
fn migrates_v1_drafts_db_to_current() {
    let fx = home_fixture();
    let db_path = fx.pcr_dir().join("drafts.db");
    install_v1_drafts_db(&db_path);

    // Pre-migration sanity: schema_version is 1 and v2+ tables are absent.
    {
        let conn = Connection::open(&db_path).expect("open pre-migrate");
        let v: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .expect("read schema_version");
        assert_eq!(v, 1, "fixture must start at v1");
    }

    // `pcr --json log` opens the store, which triggers `migrate`. With
    // no projects.json + no drafts attributed to this cwd it exits 0.
    common::pcr_in(&fx)
        .args(["--json", "log"])
        .assert()
        .success();

    // Post-migration: schema_version advanced and v2..v6 changes landed.
    let conn = Connection::open(&db_path).expect("open post-migrate");
    let version: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .expect("read schema_version post-migrate");
    assert_eq!(version, 6, "expected migrations up through v6 to run");

    // Spot-check each migration's add.
    // v2: drafts.git_diff column + prompt_commits.bundle_status column.
    let drafts_cols = collect_columns(&conn, "drafts");
    assert!(
        drafts_cols.contains(&"git_diff".to_string()),
        "drafts.git_diff missing (v2 didn't run): {drafts_cols:?}"
    );
    let commits_cols = collect_columns(&conn, "prompt_commits");
    assert!(
        commits_cols.contains(&"bundle_status".to_string()),
        "prompt_commits.bundle_status missing (v2 didn't run): {commits_cols:?}"
    );
    // v3: diff_events table.
    assert!(
        table_exists(&conn, "diff_events"),
        "diff_events missing (v3)"
    );
    // v4: drafts.head_sha column.
    assert!(
        drafts_cols.contains(&"head_sha".to_string()),
        "drafts.head_sha missing (v4 didn't run): {drafts_cols:?}"
    );
    // v5: session_state_events + saved_bubbles tables.
    assert!(
        table_exists(&conn, "session_state_events"),
        "session_state_events missing (v5)"
    );
    assert!(
        table_exists(&conn, "saved_bubbles"),
        "saved_bubbles missing (v5)"
    );
    // v6: drafts.permission_mode column.
    assert!(
        drafts_cols.contains(&"permission_mode".to_string()),
        "drafts.permission_mode missing (v6 didn't run): {drafts_cols:?}"
    );

    // Original rows survived.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM drafts", [], |r| r.get(0))
        .expect("count drafts");
    assert_eq!(count, 2, "original v1 drafts should survive migration");
}

fn collect_columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("prepare table_info");
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .expect("table_info rows");
    rows.filter_map(|r| r.ok()).collect()
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            [name],
            |r| r.get(0),
        )
        .unwrap_or(0);
    n > 0
}

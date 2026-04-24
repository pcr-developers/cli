//! Process-wide singleton SQLite connection + schema migrations. Mirrors
//! `cli/internal/store/store.go`.
//!
//! The schema and migration steps are byte-identical to the Go version so
//! a user can upgrade without any data migration. Specifically:
//!
//! - Database file: `$HOME/.pcr-dev/drafts.db`
//! - Pragma: `journal_mode=WAL`, `busy_timeout=5000`
//! - Migrations v1..v6, applied in order, tracked in `schema_version`

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use rusqlite::Connection;

use crate::config;

static CONN: OnceLock<Mutex<Connection>> = OnceLock::new();

fn db_path() -> PathBuf {
    config::pcr_dir().join("drafts.db")
}

/// Open (and lazily create) the singleton database. Returns a locked guard
/// around a `rusqlite::Connection`.
pub fn open() -> MutexGuard<'static, Connection> {
    let mutex = CONN.get_or_init(|| {
        let path = db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&path).expect("pcr: failed to open draft store");
        // WAL + 5 s busy timeout match the Go `_pragma=journal_mode(WAL)&_pragma=busy_timeout(5000)` URI.
        let _ = conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;");
        migrate(&conn).expect("pcr: failed to run store migrations");
        Mutex::new(conn)
    });
    mutex.lock().expect("draft store mutex poisoned")
}

/// Return the current schema version persisted in the `schema_version` table.
/// Zero if the table doesn't exist yet.
fn current_schema_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)",
        [],
    )?;
    let v: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(v)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version = current_schema_version(conn)?;
    let steps: &[fn(&rusqlite::Transaction) -> rusqlite::Result<()>] = &[
        migrate_v1, migrate_v2, migrate_v3, migrate_v4, migrate_v5, migrate_v6,
    ];
    for (i, step) in steps.iter().enumerate() {
        if (i as i64) < version {
            continue;
        }
        let tx = conn.unchecked_transaction()?;
        step(&tx)?;
        tx.execute(
            "INSERT INTO schema_version (version) VALUES (?)",
            [i as i64 + 1],
        )?;
        tx.commit()?;
    }
    Ok(())
}

fn migrate_v1(tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
    tx.execute_batch(
        r#"
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
        "#,
    )
}

fn migrate_v2(tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
    tx.execute_batch(
        r#"
        ALTER TABLE drafts ADD COLUMN git_diff TEXT;
        ALTER TABLE prompt_commits ADD COLUMN bundle_status TEXT NOT NULL DEFAULT 'open';
        CREATE INDEX IF NOT EXISTS idx_commits_bundle_status ON prompt_commits(bundle_status);
        "#,
    )
}

fn migrate_v3(tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
    tx.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS diff_events (
          id           INTEGER PRIMARY KEY AUTOINCREMENT,
          project_id   TEXT NOT NULL,
          project_name TEXT NOT NULL,
          files        TEXT NOT NULL,
          occurred_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS diff_events_occurred_at ON diff_events(occurred_at);
        CREATE INDEX IF NOT EXISTS diff_events_project_id  ON diff_events(project_id);
        "#,
    )
}

fn migrate_v4(tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
    tx.execute_batch("ALTER TABLE drafts ADD COLUMN head_sha TEXT")
}

fn migrate_v5(tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
    tx.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS session_state_events (
          id                   INTEGER PRIMARY KEY AUTOINCREMENT,
          session_id           TEXT NOT NULL,
          occurred_at          TEXT NOT NULL,
          unified_mode         TEXT,
          model_name           TEXT,
          context_tokens_used  INTEGER,
          context_token_limit  INTEGER
        );
        CREATE INDEX IF NOT EXISTS sse_session_time
            ON session_state_events(session_id, occurred_at);

        CREATE TABLE IF NOT EXISTS saved_bubbles (
          session_id   TEXT NOT NULL,
          bubble_id    TEXT NOT NULL,
          draft_hash   TEXT NOT NULL,
          saved_at     TEXT NOT NULL DEFAULT (datetime('now')),
          PRIMARY KEY (session_id, bubble_id)
        );
        "#,
    )
}

fn migrate_v6(tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
    tx.execute_batch("ALTER TABLE drafts ADD COLUMN permission_mode TEXT")
}

/// Helper: treat empty strings as NULL for optional columns (matches the Go
/// `nullableStr` helper in multiple store files).
pub fn null_if_empty(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

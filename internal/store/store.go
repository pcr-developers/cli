package store

import (
	"database/sql"
	"os"
	"path/filepath"
	"strconv"
	"sync"

	"github.com/pcr-developers/cli/internal/config"
	_ "modernc.org/sqlite"
)

var (
	db   *sql.DB
	once sync.Once
)

func dbPath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, config.PCRDir, "drafts.db")
}

// Open returns the singleton DB, initializing it on first call.
func Open() *sql.DB {
	once.Do(func() {
		path := dbPath()
		_ = os.MkdirAll(filepath.Dir(path), 0755)
		var err error
		db, err = sql.Open("sqlite", path+"?_journal=WAL&_timeout=5000")
		if err != nil {
			panic("pcr: failed to open draft store: " + err.Error())
		}
		db.SetMaxOpenConns(1)
		migrate(db)
	})
	return db
}

func migrate(db *sql.DB) {
	_, _ = db.Exec(`CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)`)

	var version int
	_ = db.QueryRow(`SELECT COALESCE(MAX(version), 0) FROM schema_version`).Scan(&version)

	steps := []func(*sql.Tx) error{migrateV1, migrateV2, migrateV3, migrateV4, migrateV5, migrateV6}

	for i, step := range steps {
		if i < version {
			continue
		}
		tx, err := db.Begin()
		if err != nil {
			panic("pcr: failed to begin migration v" + strconv.Itoa(i+1) + ": " + err.Error())
		}
		if err := step(tx); err != nil {
			_ = tx.Rollback()
			panic("pcr: failed to apply migration v" + strconv.Itoa(i+1) + ": " + err.Error())
		}
		if _, err := tx.Exec(`INSERT INTO schema_version (version) VALUES (?)`, i+1); err != nil {
			_ = tx.Rollback()
			panic("pcr: failed to record schema version: " + err.Error())
		}
		if err := tx.Commit(); err != nil {
			panic("pcr: failed to commit migration v" + strconv.Itoa(i+1) + ": " + err.Error())
		}
	}
}

func migrateV1(tx *sql.Tx) error {
	_, err := tx.Exec(`
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
	`)
	return err
}

func migrateV2(tx *sql.Tx) error {
	_, err := tx.Exec(`
		ALTER TABLE drafts ADD COLUMN git_diff TEXT;
		ALTER TABLE prompt_commits ADD COLUMN bundle_status TEXT NOT NULL DEFAULT 'open';
		CREATE INDEX IF NOT EXISTS idx_commits_bundle_status ON prompt_commits(bundle_status);
	`)
	return err
}

func migrateV4(tx *sql.Tx) error {
	_, err := tx.Exec(`ALTER TABLE drafts ADD COLUMN head_sha TEXT`)
	return err
}

func migrateV3(tx *sql.Tx) error {
	_, err := tx.Exec(`
		CREATE TABLE IF NOT EXISTS diff_events (
		  id           INTEGER PRIMARY KEY AUTOINCREMENT,
		  project_id   TEXT NOT NULL,
		  project_name TEXT NOT NULL,
		  files        TEXT NOT NULL,
		  occurred_at  TEXT NOT NULL DEFAULT (datetime('now'))
		);
		CREATE INDEX IF NOT EXISTS diff_events_occurred_at ON diff_events(occurred_at);
		CREATE INDEX IF NOT EXISTS diff_events_project_id  ON diff_events(project_id);
	`)
	return err
}

func migrateV6(tx *sql.Tx) error {
	_, err := tx.Exec(`ALTER TABLE drafts ADD COLUMN permission_mode TEXT`)
	return err
}

func migrateV5(tx *sql.Tx) error {
	_, err := tx.Exec(`
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
	`)
	return err
}

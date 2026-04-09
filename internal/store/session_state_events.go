package store

import (
	"time"
)

// SessionStateEvent records a point-in-time snapshot of a Cursor session's
// metadata. A new row is written only when something changes (mode, model,
// context usage, etc.), so the table is a sparse change-log, not a heartbeat.
type SessionStateEvent struct {
	ID                int64
	SessionID         string
	OccurredAt        time.Time
	UnifiedMode       string
	ModelName         string
	ContextTokensUsed int
	ContextTokenLimit int
}

// RecordSessionStateEvent inserts a new session state snapshot.
// Called by the SessionStateWatcher whenever it detects a change.
func RecordSessionStateEvent(e SessionStateEvent) error {
	db := Open()
	_, err := db.Exec(
		`INSERT INTO session_state_events
		    (session_id, occurred_at, unified_mode, model_name, context_tokens_used, context_token_limit)
		 VALUES (?, ?, ?, ?, ?, ?)`,
		e.SessionID,
		e.OccurredAt.UTC().Format(time.RFC3339),
		e.UnifiedMode,
		e.ModelName,
		e.ContextTokensUsed,
		e.ContextTokenLimit,
	)
	return err
}

// GetSessionStateAt returns the most recent session state event for sessionID
// that occurred at or before the given time. This gives the exact mode, model,
// and context state that was active when a specific user prompt was sent.
// Returns nil if no events exist for this session before the given time.
func GetSessionStateAt(sessionID string, at time.Time) (*SessionStateEvent, error) {
	db := Open()
	row := db.QueryRow(`
		SELECT id, session_id, occurred_at, unified_mode, model_name,
		       context_tokens_used, context_token_limit
		FROM session_state_events
		WHERE session_id = ? AND occurred_at <= ?
		ORDER BY occurred_at DESC
		LIMIT 1
	`, sessionID, at.UTC().Format(time.RFC3339))

	var e SessionStateEvent
	var occurredAtStr string
	err := row.Scan(
		&e.ID, &e.SessionID, &occurredAtStr,
		&e.UnifiedMode, &e.ModelName,
		&e.ContextTokensUsed, &e.ContextTokenLimit,
	)
	if err != nil {
		return nil, nil // no state recorded yet, not an error
	}
	e.OccurredAt, _ = time.Parse(time.RFC3339, occurredAtStr)
	return &e, nil
}


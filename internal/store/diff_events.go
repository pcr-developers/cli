package store

import (
	"database/sql"
	"encoding/json"
	"strings"
	"time"
)

// DiffEvent records a batch of file changes detected in one DiffTracker poll cycle.
// files contains absolute paths of files that changed state since the previous poll.
type DiffEvent struct {
	ID          int64
	ProjectID   string
	ProjectName string
	Files       []string
	OccurredAt  time.Time
}

// RecordDiffEvent stores a new diff event. Called by DiffTracker when it detects
// that files in a project changed since the last poll.
func RecordDiffEvent(projectID, projectName string, files []string, at time.Time) error {
	if len(files) == 0 {
		return nil
	}
	filesJSON, err := json.Marshal(files)
	if err != nil {
		return err
	}
	db := Open()
	_, err = db.Exec(
		`INSERT INTO diff_events (project_id, project_name, files, occurred_at) VALUES (?, ?, ?, ?)`,
		projectID, projectName, string(filesJSON), at.UTC().Format(time.RFC3339),
	)
	return err
}

// GetDiffEventsInWindow returns all diff events whose occurred_at falls within
// [from, to]. Pass zero time for from to get all events up to to.
func GetDiffEventsInWindow(from, to time.Time) ([]DiffEvent, error) {
	db := Open()

	var (
		rows *sql.Rows
		err  error
	)
	if from.IsZero() {
		rows, err = db.Query(
			`SELECT id, project_id, project_name, files, occurred_at
			 FROM diff_events WHERE occurred_at <= ? ORDER BY occurred_at ASC`,
			to.UTC().Format(time.RFC3339),
		)
	} else {
		rows, err = db.Query(
			`SELECT id, project_id, project_name, files, occurred_at
			 FROM diff_events WHERE occurred_at > ? AND occurred_at <= ? ORDER BY occurred_at ASC`,
			from.UTC().Format(time.RFC3339),
			to.UTC().Format(time.RFC3339),
		)
	}
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var events []DiffEvent
	for rows.Next() {
		var e DiffEvent
		var filesJSON, occurredAtStr string
		if err := rows.Scan(&e.ID, &e.ProjectID, &e.ProjectName, &filesJSON, &occurredAtStr); err != nil {
			return nil, err
		}
		_ = json.Unmarshal([]byte(filesJSON), &e.Files)
		e.OccurredAt, _ = time.Parse(time.RFC3339, occurredAtStr)
		events = append(events, e)
	}
	return events, rows.Err()
}

// PruneDiffEvents deletes events older than the given time to keep the DB small.
func PruneDiffEvents(before time.Time) error {
	db := Open()
	_, err := db.Exec(
		`DELETE FROM diff_events WHERE occurred_at < ?`,
		before.UTC().Format(time.RFC3339),
	)
	return err
}

// DeleteDiffEventsByID deletes specific diff_events by ID. Called after a
// Cursor turn is saved so consumed events are removed immediately rather
// than waiting for the TTL prune.
func DeleteDiffEventsByID(ids []int64) error {
	if len(ids) == 0 {
		return nil
	}
	db := Open()
	placeholders := strings.Repeat("?,", len(ids))
	placeholders = placeholders[:len(placeholders)-1]
	args := make([]any, len(ids))
	for i, id := range ids {
		args[i] = id
	}
	_, err := db.Exec(`DELETE FROM diff_events WHERE id IN (`+placeholders+`)`, args...)
	return err
}


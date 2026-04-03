package store

import (
	"os/exec"
	"time"
)

// GCPushed deletes pushed drafts and their commits older than olderThanDays.
// Returns the number of drafts deleted.
func GCPushed(olderThanDays int) (int, error) {
	db := Open()
	cutoff := time.Now().Add(-time.Duration(olderThanDays) * 24 * time.Hour).UTC().Format(time.RFC3339)

	rows, err := db.Query("SELECT id FROM prompt_commits WHERE pushed_at IS NOT NULL AND pushed_at < ?", cutoff)
	if err != nil {
		return 0, err
	}
	var ids []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return 0, err
		}
		ids = append(ids, id)
	}
	rows.Close()

	return deleteCommits(ids)
}

// GCAllPushed deletes all pushed records regardless of age.
func GCAllPushed() (int, error) {
	db := Open()
	rows, err := db.Query("SELECT id FROM prompt_commits WHERE pushed_at IS NOT NULL")
	if err != nil {
		return 0, err
	}
	var ids []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return 0, err
		}
		ids = append(ids, id)
	}
	rows.Close()

	return deleteCommits(ids)
}

// GCUnpushed deletes all unpushed committed bundles and their associated drafts.
func GCUnpushed() (int, error) {
	db := Open()
	rows, err := db.Query("SELECT id FROM prompt_commits WHERE pushed_at IS NULL")
	if err != nil {
		return 0, err
	}
	var ids []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return 0, err
		}
		ids = append(ids, id)
	}
	rows.Close()
	return deleteCommits(ids)
}

// GCOrphaned deletes unpushed commits whose HEAD SHA no longer exists in git history.
// Restored drafts are set back to 'draft' status. Returns number of commits deleted.
func GCOrphaned(projectPath string) (int, error) {
	db := Open()
	rows, err := db.Query("SELECT id, head_sha FROM prompt_commits WHERE pushed_at IS NULL")
	if err != nil {
		return 0, err
	}
	type row struct{ id, sha string }
	var commits []row
	for rows.Next() {
		var r row
		if err := rows.Scan(&r.id, &r.sha); err != nil {
			rows.Close()
			return 0, err
		}
		commits = append(commits, r)
	}
	rows.Close()

	var orphanIDs []string
	for _, c := range commits {
		cmd := exec.Command("git", "cat-file", "-e", c.sha)
		cmd.Dir = projectPath
		if err := cmd.Run(); err != nil {
			// SHA not in git history
			orphanIDs = append(orphanIDs, c.id)
		}
	}
	if len(orphanIDs) == 0 {
		return 0, nil
	}

	// Restore associated drafts to 'draft' before deleting the commit
	tx, err := db.Begin()
	if err != nil {
		return 0, err
	}
	defer func() { _ = tx.Rollback() }()

	for _, id := range orphanIDs {
		itemRows, err := tx.Query("SELECT draft_id FROM prompt_commit_items WHERE prompt_commit_id = ?", id)
		if err != nil {
			return 0, err
		}
		var draftIDs []string
		for itemRows.Next() {
			var did string
			_ = itemRows.Scan(&did)
			draftIDs = append(draftIDs, did)
		}
		itemRows.Close()

		if _, err := tx.Exec("DELETE FROM prompt_commit_items WHERE prompt_commit_id = ?", id); err != nil {
			return 0, err
		}
		for _, did := range draftIDs {
			if _, err := tx.Exec("UPDATE drafts SET status = 'draft' WHERE id = ? AND status = 'committed'", did); err != nil {
				return 0, err
			}
		}
		if _, err := tx.Exec("DELETE FROM prompt_commits WHERE id = ?", id); err != nil {
			return 0, err
		}
	}

	if err := tx.Commit(); err != nil {
		return 0, err
	}
	return len(orphanIDs), nil
}

func deleteCommits(ids []string) (int, error) {
	if len(ids) == 0 {
		return 0, nil
	}
	db := Open()
	deleted := 0

	tx, err := db.Begin()
	if err != nil {
		return 0, err
	}
	defer func() { _ = tx.Rollback() }()

	for _, id := range ids {
		// Count drafts
		var n int
		_ = tx.QueryRow("SELECT COUNT(*) FROM prompt_commit_items WHERE prompt_commit_id = ?", id).Scan(&n)
		deleted += n

		// Get draft IDs
		itemRows, err := tx.Query("SELECT draft_id FROM prompt_commit_items WHERE prompt_commit_id = ?", id)
		if err != nil {
			return 0, err
		}
		var draftIDs []string
		for itemRows.Next() {
			var did string
			_ = itemRows.Scan(&did)
			draftIDs = append(draftIDs, did)
		}
		itemRows.Close()

		// Delete in order: items → drafts → commit
		if _, err := tx.Exec("DELETE FROM prompt_commit_items WHERE prompt_commit_id = ?", id); err != nil {
			return 0, err
		}
		for _, did := range draftIDs {
			if _, err := tx.Exec("DELETE FROM drafts WHERE id = ?", did); err != nil {
				return 0, err
			}
		}
		if _, err := tx.Exec("DELETE FROM prompt_commits WHERE id = ?", id); err != nil {
			return 0, err
		}
	}

	return deleted, tx.Commit()
}

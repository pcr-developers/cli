package store

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

type PromptCommit struct {
	ID          string        `json:"id"`
	Message     string        `json:"message"`
	ProjectID   string        `json:"project_id,omitempty"`
	ProjectName string        `json:"project_name,omitempty"`
	BranchName  string        `json:"branch_name,omitempty"`
	SessionShas []string      `json:"session_shas,omitempty"`
	HeadSha     string        `json:"head_sha"`
	PushedAt    string        `json:"pushed_at,omitempty"`
	RemoteID    string        `json:"remote_id,omitempty"`
	CommittedAt string        `json:"committed_at"`
	Items       []DraftRecord `json:"items,omitempty"`
}

// CreateCommit bundles drafts into a commit record atomically.
func CreateCommit(message, headSha string, draftIDs []string, projectID, projectName, branchName string) (*PromptCommit, error) {
	db := Open()
	id := newUUID()
	now := time.Now().UTC().Format(time.RFC3339)

	// Collect union of session_commit_shas from all included drafts
	shaSet := map[string]bool{}
	for _, draftID := range draftIDs {
		var sessionShasJSON *string
		_ = db.QueryRow("SELECT session_commit_shas FROM drafts WHERE id = ?", draftID).Scan(&sessionShasJSON)
		if sessionShasJSON != nil {
			var shas []string
			if err := json.Unmarshal([]byte(*sessionShasJSON), &shas); err == nil {
				for _, sha := range shas {
					shaSet[sha] = true
				}
			}
		}
	}
	sessionShas := make([]string, 0, len(shaSet))
	for sha := range shaSet {
		sessionShas = append(sessionShas, sha)
	}
	var sessionShasJSON *string
	if len(sessionShas) > 0 {
		b, _ := json.Marshal(sessionShas)
		s := string(b)
		sessionShasJSON = &s
	}

	tx, err := db.Begin()
	if err != nil {
		return nil, err
	}
	defer func() { _ = tx.Rollback() }()

	_, err = tx.Exec(`
		INSERT INTO prompt_commits (id, message, project_id, project_name, branch_name, session_shas, head_sha, committed_at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?)
	`, id, message,
		nullableStr(projectID), nullableStr(projectName), nullableStr(branchName),
		sessionShasJSON, headSha, now)
	if err != nil {
		return nil, err
	}

	for _, draftID := range draftIDs {
		if _, err := tx.Exec("INSERT OR IGNORE INTO prompt_commit_items (prompt_commit_id, draft_id) VALUES (?, ?)", id, draftID); err != nil {
			return nil, err
		}
		if _, err := tx.Exec("UPDATE drafts SET status = 'committed' WHERE id = ?", draftID); err != nil {
			return nil, err
		}
	}

	if err := tx.Commit(); err != nil {
		return nil, err
	}

	return &PromptCommit{
		ID:          id,
		Message:     message,
		ProjectID:   projectID,
		ProjectName: projectName,
		BranchName:  branchName,
		SessionShas: sessionShas,
		HeadSha:     headSha,
		CommittedAt: now,
	}, nil
}

// ListCommits returns commits with optional filters.
// projectIDs and projectNames are OR-combined; pass nil slices to return all.
func ListCommits(pushed *bool, projectIDs, projectNames []string) ([]PromptCommit, error) {
	db := Open()
	conditions := []string{}
	args := []any{}
	if pushed != nil {
		if *pushed {
			conditions = append(conditions, "pushed_at IS NOT NULL")
		} else {
			conditions = append(conditions, "pushed_at IS NULL")
		}
	}
	var projectClauses []string
	for _, id := range projectIDs {
		projectClauses = append(projectClauses, "project_id = ?")
		args = append(args, id)
	}
	for _, name := range projectNames {
		projectClauses = append(projectClauses, "project_name = ?")
		args = append(args, name)
	}
	if len(projectClauses) > 0 {
		conditions = append(conditions, "("+strings.Join(projectClauses, " OR ")+")")
	}
	where := ""
	if len(conditions) > 0 {
		where = "WHERE " + strings.Join(conditions, " AND ")
	}
	rows, err := db.Query(
		fmt.Sprintf("SELECT * FROM prompt_commits %s ORDER BY committed_at DESC", where),
		args...,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanCommitRows(rows)
}

// GetUnpushedCommits returns all commits not yet pushed (across all projects).
func GetUnpushedCommits() ([]PromptCommit, error) {
	f := false
	return ListCommits(&f, nil, nil)
}

// GetCommitBySha finds a commit by its git HEAD SHA.
func GetCommitBySha(headSha string) (*PromptCommit, error) {
	db := Open()
	rows, err := db.Query("SELECT * FROM prompt_commits WHERE head_sha = ?", headSha)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	commits, err := scanCommitRows(rows)
	if err != nil || len(commits) == 0 {
		return nil, err
	}
	return &commits[0], nil
}

// RelinkCommit updates a commit's HEAD SHA (for git amend).
func RelinkCommit(commitID, newHeadSha string) error {
	db := Open()
	_, err := db.Exec("UPDATE prompt_commits SET head_sha = ? WHERE id = ?", newHeadSha, commitID)
	return err
}

// GetCommitWithItems fetches a commit and its associated drafts.
func GetCommitWithItems(commitID string) (*PromptCommit, error) {
	db := Open()
	rows, err := db.Query("SELECT * FROM prompt_commits WHERE id = ?", commitID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	commits, err := scanCommitRows(rows)
	if err != nil || len(commits) == 0 {
		return nil, err
	}
	commit := commits[0]
	rows.Close()

	itemRows, err := db.Query(`
		SELECT d.* FROM drafts d
		JOIN prompt_commit_items i ON i.draft_id = d.id
		WHERE i.prompt_commit_id = ?
		ORDER BY d.captured_at ASC
	`, commitID)
	if err != nil {
		return nil, err
	}
	defer itemRows.Close()
	commit.Items, err = scanDraftRows(itemRows)
	if err != nil {
		return nil, err
	}
	return &commit, nil
}

// MarkPushed marks a commit and its drafts as pushed.
func MarkPushed(commitID, remoteID string) error {
	db := Open()
	now := time.Now().UTC().Format(time.RFC3339)
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()

	if _, err := tx.Exec("UPDATE prompt_commits SET pushed_at = ?, remote_id = ? WHERE id = ?", now, remoteID, commitID); err != nil {
		return err
	}
	if _, err := tx.Exec(`
		UPDATE drafts SET status = 'pushed'
		WHERE id IN (SELECT draft_id FROM prompt_commit_items WHERE prompt_commit_id = ?)
	`, commitID); err != nil {
		return err
	}
	return tx.Commit()
}

func newUUID() string {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	b[6] = (b[6] & 0x0f) | 0x40
	b[8] = (b[8] & 0x3f) | 0x80
	h := hex.EncodeToString(b)
	return fmt.Sprintf("%s-%s-%s-%s-%s", h[:8], h[8:12], h[12:16], h[16:20], h[20:])
}

func scanCommitRows(rows sqlRows) ([]PromptCommit, error) {
	var commits []PromptCommit
	for rows.Next() {
		var (
			c                PromptCommit
			projectID        *string
			projectName      *string
			branchName       *string
			sessionShasJSON  *string
			pushedAt         *string
			remoteID         *string
		)
		err := rows.Scan(
			&c.ID, &c.Message,
			&projectID, &projectName, &branchName,
			&sessionShasJSON, &c.HeadSha,
			&pushedAt, &remoteID, &c.CommittedAt,
		)
		if err != nil {
			return nil, err
		}
		if projectID != nil {
			c.ProjectID = *projectID
		}
		if projectName != nil {
			c.ProjectName = *projectName
		}
		if branchName != nil {
			c.BranchName = *branchName
		}
		if sessionShasJSON != nil {
			_ = json.Unmarshal([]byte(*sessionShasJSON), &c.SessionShas)
		}
		if pushedAt != nil {
			c.PushedAt = *pushedAt
		}
		if remoteID != nil {
			c.RemoteID = *remoteID
		}
		commits = append(commits, c)
	}
	return commits, rows.Close()
}

package store

import (
	"encoding/json"
	"fmt"
	"path/filepath"
	"strings"
	"time"

	"github.com/pcr-developers/cli/internal/supabase"
)

type DraftStatus string

const (
	StatusDraft     DraftStatus = "draft"
	StatusStaged    DraftStatus = "staged"
	StatusCommitted DraftStatus = "committed"
	StatusPushed    DraftStatus = "pushed"
)

type DraftRecord struct {
	ID                string           `json:"id"`
	ContentHash       string           `json:"content_hash"`
	SessionID         string           `json:"session_id"`
	ProjectID         string           `json:"project_id,omitempty"`
	ProjectName       string           `json:"project_name"`
	BranchName        string           `json:"branch_name,omitempty"`
	PromptText        string           `json:"prompt_text"`
	ResponseText      string           `json:"response_text,omitempty"`
	Model             string           `json:"model,omitempty"`
	Source            string           `json:"source"`
	CaptureMethod     string           `json:"capture_method"`
	ToolCalls         []map[string]any `json:"tool_calls,omitempty"`
	FileContext       map[string]any   `json:"file_context,omitempty"`
	CapturedAt        string           `json:"captured_at"`
	SessionCommitShas []string         `json:"session_commit_shas,omitempty"`
	Status            DraftStatus      `json:"status"`
	CreatedAt         string           `json:"created_at"`
}

// SaveDraft inserts or updates a draft. Idempotent via content_hash.
func SaveDraft(record supabase.PromptRecord, sessionShas []string) error {
	db := Open()

	id := supabase.PromptID(record.SessionID, record.PromptText, "")
	hash := supabase.PromptContentHash(record.SessionID, record.PromptText, "")

	var toolCallsJSON, fileContextJSON, sessionShasJSON *string
	if len(record.ToolCalls) > 0 {
		b, _ := json.Marshal(record.ToolCalls)
		s := string(b)
		toolCallsJSON = &s
	}
	if len(record.FileContext) > 0 {
		b, _ := json.Marshal(record.FileContext)
		s := string(b)
		fileContextJSON = &s
	}
	if len(sessionShas) > 0 {
		b, _ := json.Marshal(sessionShas)
		s := string(b)
		sessionShasJSON = &s
	}

	capturedAt := record.CapturedAt
	if capturedAt == "" {
		capturedAt = time.Now().UTC().Format(time.RFC3339)
	}

	_, err := db.Exec(`
		INSERT INTO drafts (
		  id, content_hash, session_id, project_id, project_name, branch_name,
		  prompt_text, response_text, model, source, capture_method,
		  tool_calls, file_context, captured_at, session_commit_shas, status
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'draft')
		ON CONFLICT(content_hash) DO UPDATE SET
		  response_text = COALESCE(excluded.response_text, drafts.response_text),
		  tool_calls    = COALESCE(excluded.tool_calls,    drafts.tool_calls),
		  file_context  = COALESCE(excluded.file_context,  drafts.file_context),
		  model         = COALESCE(excluded.model,          drafts.model)
		WHERE drafts.status = 'draft'
	`,
		id, hash,
		record.SessionID,
		nullableStr(record.ProjectID),
		record.ProjectName,
		nullableStr(record.BranchName),
		record.PromptText,
		nullableStr(record.ResponseText),
		nullableStr(record.Model),
		record.Source,
		record.CaptureMethod,
		toolCallsJSON,
		fileContextJSON,
		capturedAt,
		sessionShasJSON,
	)
	return err
}

// IsDraftSaved checks if a draft with the given prompt-only hash already exists.
func IsDraftSaved(sessionID, promptText string) bool {
	db := Open()
	hash := supabase.PromptContentHash(sessionID, promptText, "")
	var exists int
	_ = db.QueryRow("SELECT 1 FROM drafts WHERE content_hash = ?", hash).Scan(&exists)
	return exists == 1
}

// GetDraftsByStatus returns drafts filtered by status and optionally by project.
// projectIDs and projectNames are OR-combined; pass nil slices to return all.
func GetDraftsByStatus(status DraftStatus, projectIDs, projectNames []string) ([]DraftRecord, error) {
	db := Open()
	args := []any{string(status)}
	where := "status = ?"

	var clauses []string
	for _, id := range projectIDs {
		clauses = append(clauses, "project_id = ?")
		args = append(args, id)
	}
	for _, name := range projectNames {
		clauses = append(clauses, "project_name = ?")
		args = append(args, name)
	}
	if len(clauses) > 0 {
		where += " AND (" + strings.Join(clauses, " OR ") + ")"
	}

	rows, err := db.Query(
		fmt.Sprintf("SELECT * FROM drafts WHERE %s ORDER BY captured_at ASC", where),
		args...,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanDraftRows(rows)
}

// GetAllDrafts returns all drafts with optional filters.
func GetAllDrafts(status DraftStatus, projectID string) ([]DraftRecord, error) {
	db := Open()
	conditions := []string{}
	args := []any{}
	if status != "" {
		conditions = append(conditions, "status = ?")
		args = append(args, string(status))
	}
	if projectID != "" {
		conditions = append(conditions, "project_id = ?")
		args = append(args, projectID)
	}
	where := ""
	if len(conditions) > 0 {
		where = "WHERE " + strings.Join(conditions, " AND ")
	}
	rows, err := db.Query(
		fmt.Sprintf("SELECT * FROM drafts %s ORDER BY captured_at ASC", where),
		args...,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanDraftRows(rows)
}

// GetCandidatesForCommit scores drafts by how many changed files they mention.
func GetCandidatesForCommit(projectIDs, projectNames []string, changedFiles []string) (relevant []DraftRecord, unrelated []DraftRecord, err error) {
	drafts, err := GetDraftsByStatus(StatusDraft, projectIDs, projectNames)
	if err != nil {
		return nil, nil, err
	}
	if len(changedFiles) == 0 {
		return nil, drafts, nil
	}

	type scored struct {
		draft      DraftRecord
		matchCount int
	}
	results := make([]scored, 0, len(drafts))

	for _, d := range drafts {
		matchCount := 0
		touched := map[string]bool{}

		// 1. Tool call paths (strongest signal)
		for _, tc := range d.ToolCalls {
			var path string
			if input, ok := tc["input"].(map[string]any); ok {
				path, _ = input["path"].(string)
				if path == "" {
					path, _ = input["file_path"].(string)
				}
			}
			if path == "" {
				path, _ = tc["path"].(string)
			}
			if path != "" {
				for _, cf := range changedFiles {
					if !touched[cf] && (strings.HasSuffix(path, cf) || strings.HasSuffix(cf, path) ||
						strings.Contains(path, cf) || strings.Contains(cf, path)) {
						matchCount++
						touched[cf] = true
					}
				}
			}
		}

		// 2. Text mentions
		text := d.PromptText + " " + d.ResponseText
		for _, cf := range changedFiles {
			if touched[cf] {
				continue
			}
			base := filepath.Base(cf)
			stem := base
			if idx := strings.LastIndex(base, "."); idx >= 0 {
				stem = base[:idx]
			}
			if strings.Contains(text, cf) || strings.Contains(text, base) ||
				(len(stem) >= 5 && strings.Contains(text, stem)) {
				matchCount++
				touched[cf] = true
			}
		}

		results = append(results, scored{draft: d, matchCount: matchCount})
	}

	// Sort by match count descending
	for i := 0; i < len(results); i++ {
		for j := i + 1; j < len(results); j++ {
			if results[j].matchCount > results[i].matchCount {
				results[i], results[j] = results[j], results[i]
			}
		}
	}

	for _, s := range results {
		if s.matchCount > 0 {
			relevant = append(relevant, s.draft)
		} else {
			unrelated = append(unrelated, s.draft)
		}
	}
	return relevant, unrelated, nil
}

// StageDrafts marks the given draft IDs as "staged" for manual bundling.
func StageDrafts(ids []string) error {
	db := Open()
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	for _, id := range ids {
		if _, err := tx.Exec("UPDATE drafts SET status = 'staged' WHERE id = ? AND status = 'draft'", id); err != nil {
			return err
		}
	}
	return tx.Commit()
}

// GetStagedDrafts returns all drafts with status "staged".
func GetStagedDrafts() ([]DraftRecord, error) {
	return GetDraftsByStatus(StatusStaged, nil, nil)
}

// ClearStaged resets all staged drafts back to "draft" status.
func ClearStaged() error {
	db := Open()
	_, err := db.Exec("UPDATE drafts SET status = 'draft' WHERE status = 'staged'")
	return err
}

// RestoreDraftToDraft sets a pushed draft back to 'draft' status.
func RestoreDraftToDraft(draftID string) bool {
	db := Open()
	res, err := db.Exec("UPDATE drafts SET status = 'draft' WHERE id = ? AND status = 'pushed'", draftID)
	if err != nil {
		return false
	}
	n, _ := res.RowsAffected()
	return n > 0
}

// ─── Row scanning ─────────────────────────────────────────────────────────────

type sqlRows interface {
	Next() bool
	Scan(...any) error
	Close() error
}

func scanDraftRows(rows sqlRows) ([]DraftRecord, error) {
	var drafts []DraftRecord
	for rows.Next() {
		d, err := scanOneDraft(rows)
		if err != nil {
			return nil, err
		}
		drafts = append(drafts, d)
	}
	return drafts, rows.Close()
}

func scanOneDraft(row interface{ Scan(...any) error }) (DraftRecord, error) {
	var (
		d                     DraftRecord
		projectID             *string
		branchName            *string
		responseText          *string
		model                 *string
		toolCallsJSON         *string
		fileContextJSON       *string
		sessionCommitShasJSON *string
	)
	err := row.Scan(
		&d.ID, &d.ContentHash, &d.SessionID,
		&projectID, &d.ProjectName, &branchName,
		&d.PromptText, &responseText, &model,
		&d.Source, &d.CaptureMethod,
		&toolCallsJSON, &fileContextJSON,
		&d.CapturedAt, &sessionCommitShasJSON,
		&d.Status, &d.CreatedAt,
	)
	if err != nil {
		return d, err
	}
	if projectID != nil {
		d.ProjectID = *projectID
	}
	if branchName != nil {
		d.BranchName = *branchName
	}
	if responseText != nil {
		d.ResponseText = *responseText
	}
	if model != nil {
		d.Model = *model
	}
	if toolCallsJSON != nil {
		_ = json.Unmarshal([]byte(*toolCallsJSON), &d.ToolCalls)
	}
	if fileContextJSON != nil {
		_ = json.Unmarshal([]byte(*fileContextJSON), &d.FileContext)
	}
	if sessionCommitShasJSON != nil {
		_ = json.Unmarshal([]byte(*sessionCommitShasJSON), &d.SessionCommitShas)
	}
	return d, nil
}

func nullableStr(s string) any {
	if s == "" {
		return nil
	}
	return s
}

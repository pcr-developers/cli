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
	GitDiff           string           `json:"git_diff,omitempty"`
	HeadSha           string           `json:"head_sha,omitempty"`
}

// TouchedProjectIDs returns all project IDs recorded in file_context for this
// draft — the primary project plus any additional repos whose files were in
// context for this prompt. Safe to call on any draft; returns nil if unset.
func (d DraftRecord) TouchedProjectIDs() []string {
	if d.FileContext == nil {
		return nil
	}
	raw, ok := d.FileContext["touched_project_ids"]
	if !ok {
		return nil
	}
	// file_context round-trips through JSON so the value comes back as []any.
	switch v := raw.(type) {
	case []string:
		return v
	case []any:
		out := make([]string, 0, len(v))
		for _, item := range v {
			if s, ok := item.(string); ok && s != "" {
				out = append(out, s)
			}
		}
		return out
	}
	return nil
}

// SaveDraft inserts or updates a draft. Idempotent via content_hash.
// If record.ID and record.ContentHash are pre-populated (e.g. using V2 hashes
// that include a timestamp), those values are used directly so that identical
// prompts sent at different times in the same session produce distinct records.
func SaveDraft(record supabase.PromptRecord, sessionShas []string, gitDiff string, headShaArg ...string) error {
	headSha := ""
	if len(headShaArg) > 0 {
		headSha = headShaArg[0]
	}
	db := Open()

	id := record.ID
	hash := record.ContentHash
	if id == "" {
		id = supabase.PromptID(record.SessionID, record.PromptText, "")
	}
	if hash == "" {
		hash = supabase.PromptContentHash(record.SessionID, record.PromptText, "")
	}

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
		  tool_calls, file_context, captured_at, session_commit_shas, status, git_diff, head_sha
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'draft', ?, ?)
		ON CONFLICT(content_hash) DO UPDATE SET
		  response_text = COALESCE(excluded.response_text, drafts.response_text),
		  tool_calls    = COALESCE(excluded.tool_calls,    drafts.tool_calls),
		  file_context  = COALESCE(excluded.file_context,  drafts.file_context),
		  model         = COALESCE(excluded.model,          drafts.model),
		  git_diff      = COALESCE(excluded.git_diff,       drafts.git_diff),
		  head_sha      = COALESCE(excluded.head_sha,       drafts.head_sha),
		  project_id   = COALESCE(NULLIF(drafts.project_id,   ''), excluded.project_id),
		  project_name = COALESCE(NULLIF(drafts.project_name, ''), excluded.project_name)
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
		nullableStr(gitDiff),
		nullableStr(headSha),
	)
	return err
}

// IsDraftSavedByBubble checks whether a specific Cursor bubble (identified by
// sessionID + bubbleID) has already been saved as a draft. Used by the
// PromptScanner to avoid re-saving turns on restart.
func IsDraftSavedByBubble(sessionID, bubbleID string) bool {
	db := Open()
	var exists int
	_ = db.QueryRow(
		"SELECT 1 FROM saved_bubbles WHERE session_id = ? AND bubble_id = ?",
		sessionID, bubbleID,
	).Scan(&exists)
	return exists == 1
}

// MarkBubbleSaved records that a bubble has been saved, keyed by
// session_id + bubble_id. Called immediately after SaveDraft succeeds.
func MarkBubbleSaved(sessionID, bubbleID, draftHash string) error {
	db := Open()
	_, err := db.Exec(
		`INSERT OR IGNORE INTO saved_bubbles (session_id, bubble_id, draft_hash) VALUES (?, ?, ?)`,
		sessionID, bubbleID, draftHash,
	)
	return err
}

// IsDraftSaved checks if a draft with the given session + prompt text already
// exists. Checks both the legacy hash (no timestamp) and the V2 hash (with
// capturedAt) so that re-fires for the same bubble are correctly deduplicated.
func IsDraftSaved(sessionID, promptText string) bool {
	return IsDraftSavedAt(sessionID, promptText, "")
}

// IsDraftSavedAt checks if a draft exists for this session + prompt text +
// timestamp combination. Pass capturedAt="" to check only the legacy hash.
func IsDraftSavedAt(sessionID, promptText, capturedAt string) bool {
	db := Open()
	legacyHash := supabase.PromptContentHash(sessionID, promptText, "")
	var exists int
	_ = db.QueryRow("SELECT 1 FROM drafts WHERE content_hash = ?", legacyHash).Scan(&exists)
	if exists == 1 {
		return true
	}
	if capturedAt != "" {
		v2Hash := supabase.PromptContentHashV2(sessionID, promptText, capturedAt)
		_ = db.QueryRow("SELECT 1 FROM drafts WHERE content_hash = ?", v2Hash).Scan(&exists)
	}
	return exists == 1
}

// UpsertDraftProject updates attribution for an existing draft:
//   - Sets project_id/project_name if currently empty (primary attribution)
//   - Merges allIDs into file_context.touched_project_ids (accumulated across firings)
//
// This handles the case where a prompt is initially saved with only one repo tagged
// (e.g. cli), then a later watcher firing discovers an additional repo (pcr-dev)
// was also touched in the same window.
func UpsertDraftProject(contentHash, projectID, projectName string, allIDs []string) error {
	if projectID == "" && len(allIDs) == 0 {
		return nil
	}
	db := Open()

	// Read current file_context to merge touched_project_ids.
	var fcJSON *string
	if err := db.QueryRow(
		"SELECT file_context FROM drafts WHERE content_hash = ? AND status = 'draft'",
		contentHash,
	).Scan(&fcJSON); err != nil {
		return nil // draft not found or already pushed
	}

	current := map[string]any{}
	if fcJSON != nil {
		_ = json.Unmarshal([]byte(*fcJSON), &current)
	}

	// Set touched_project_ids to exactly allIDs (replace, not union).
	// Callers that want accumulation should read+merge before calling.
	if len(allIDs) > 1 {
		current["touched_project_ids"] = allIDs
	} else {
		delete(current, "touched_project_ids")
	}

	fcBytes, _ := json.Marshal(current)
	fcStr := string(fcBytes)

	// COALESCE for primary: only fill project_id/name when currently empty.
	// touched_project_ids always merges (union) so cross-repo evidence accumulates
	// over time without overwriting correct primary attribution from earlier.
	_, err := db.Exec(`
		UPDATE drafts SET
		  project_id   = COALESCE(NULLIF(project_id,   ''), ?),
		  project_name = COALESCE(NULLIF(project_name, ''), ?),
		  file_context = ?
		WHERE content_hash = ? AND status = 'draft'
	`, projectID, projectName, fcStr, contentHash)
	return err
}

// TagUnattributedDrafts sets project_id/name on drafts that have no attribution yet.
// Only touches drafts where project_id is currently empty — never overwrites
// correct per-prompt tags set by the watcher.
// projFiles maps projectID → projectName for projects with dirty files.
// allIDs is the set of all touched project IDs to store in touched_project_ids.
func TagUnattributedDrafts(primaryID, primaryName string, allIDs []string) error {
	if primaryID == "" {
		return nil
	}
	db := Open()
	rows, err := db.Query(
		`SELECT content_hash FROM drafts WHERE status IN ('draft','staged') AND (project_id IS NULL OR project_id = '')`,
	)
	if err != nil {
		return err
	}
	var hashes []string
	for rows.Next() {
		var h string
		if err := rows.Scan(&h); err != nil {
			rows.Close()
			return err
		}
		hashes = append(hashes, h)
	}
	rows.Close()

	for _, h := range hashes {
		if err := UpsertDraftProject(h, primaryID, primaryName, allIDs); err != nil {
			return err
		}
	}
	return nil
}

// ClearAllChangedFiles removes diff_event-derived attribution from every draft:
// file_context["changed_files"] and file_context["touched_project_ids"].
// Called on tracker start to discard attributions derived from stale diff_events
// so that only events from the current pcr start run are used for attribution.
func ClearAllChangedFiles() error {
	db := Open()
	rows, err := db.Query(`SELECT content_hash, file_context FROM drafts WHERE status IN ('draft','staged')`)
	if err != nil {
		return err
	}
	type row struct {
		hash string
		fc   string
	}
	var toUpdate []row
	for rows.Next() {
		var r row
		if rows.Scan(&r.hash, &r.fc) == nil {
			toUpdate = append(toUpdate, r)
		}
	}
	rows.Close()

	for _, r := range toUpdate {
		fc := map[string]any{}
		if r.fc != "" {
			_ = json.Unmarshal([]byte(r.fc), &fc)
		}
		changed := false
		for _, key := range []string{"changed_files", "touched_project_ids"} {
			if _, exists := fc[key]; exists {
				delete(fc, key)
				changed = true
			}
		}
		if !changed {
			continue
		}
		b, _ := json.Marshal(fc)
		_, _ = db.Exec(`UPDATE drafts SET file_context = ? WHERE content_hash = ?`, string(b), r.hash)
	}
	return nil
}

// EnrichDraftChangedFiles sets file_context["changed_files"] on a draft that
// was saved without it (e.g. captured before the agent finished its response).
// Only writes if changed_files is not already set — the first closed-window
// attribution is considered authoritative and is not overwritten.
// Returns nil silently if the draft doesn't exist or already has data.
func EnrichDraftChangedFiles(contentHash string, changedFiles []string) error {
	if len(changedFiles) == 0 {
		return nil
	}
	db := Open()

	var fcJSON *string
	if err := db.QueryRow(
		"SELECT file_context FROM drafts WHERE content_hash = ?",
		contentHash,
	).Scan(&fcJSON); err != nil {
		return nil
	}

	fc := map[string]any{}
	if fcJSON != nil {
		_ = json.Unmarshal([]byte(*fcJSON), &fc)
	}

	// Don't overwrite attribution that was already set.
	if existing, ok := fc["changed_files"]; ok && existing != nil {
		if arr, ok := existing.([]any); ok && len(arr) > 0 {
			return nil
		}
	}

	fc["changed_files"] = changedFiles
	b, _ := json.Marshal(fc)
	_, err := db.Exec(
		"UPDATE drafts SET file_context = ? WHERE content_hash = ?",
		string(b), contentHash,
	)
	return err
}

// UpdateDraftResponse fills in response_text for an existing draft that has none.
// Uses exact content hash match.
func UpdateDraftResponse(sessionID, promptText, responseText string) error {
	if responseText == "" {
		return nil
	}
	db := Open()
	hash := supabase.PromptContentHash(sessionID, promptText, "")
	_, err := db.Exec(
		"UPDATE drafts SET response_text = ? WHERE content_hash = ? AND (response_text IS NULL OR response_text = '')",
		responseText, hash,
	)
	return err
}

// UpdateDraftResponseFuzzy fills in response_text by matching session_id + prompt prefix
// in Go (not SQL). Handles cases where the prompt was captured from a partially-written
// JSONL line, meaning the stored prompt_text is a prefix of the full parsed prompt.
// Returns the number of rows updated.
func UpdateDraftResponseFuzzy(sessionID string, prompts map[string]string) (int, error) {
	if len(prompts) == 0 {
		return 0, nil
	}
	db := Open()

	// Fetch all drafts for the session with missing response_text
	rows, err := db.Query(
		"SELECT id, prompt_text FROM drafts WHERE session_id = ? AND (response_text IS NULL OR response_text = '')",
		sessionID,
	)
	if err != nil {
		return 0, err
	}
	type row struct{ id, text string }
	var drafts []row
	for rows.Next() {
		var r row
		if err := rows.Scan(&r.id, &r.text); err != nil {
			rows.Close()
			return 0, err
		}
		drafts = append(drafts, r)
	}
	rows.Close()

	// For each draft, find the JSONL prompt that starts with (or equals) the stored text.
	updated := 0
	for _, d := range drafts {
		for promptText, responseText := range prompts {
			if responseText == "" {
				continue
			}
			// Match: JSONL prompt starts with stored text, or exact match.
			if strings.HasPrefix(promptText, d.text) || promptText == d.text {
				res, err := db.Exec(
					"UPDATE drafts SET response_text = ? WHERE id = ? AND (response_text IS NULL OR response_text = '')",
					responseText, d.id,
				)
				if err != nil {
					return updated, err
				}
				if n, _ := res.RowsAffected(); n > 0 {
					updated++
				}
				break
			}
		}
	}
	return updated, nil
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

// GetAllDraftsSortedByTime returns all non-pushed drafts ordered by captured_at ASC.
// Used by retagDraftsNow to iterate prompts in chronological order for window attribution.
func GetAllDraftsSortedByTime() ([]DraftRecord, error) {
	db := Open()
	rows, err := db.Query(
		`SELECT * FROM drafts WHERE status IN ('draft','staged','committed') ORDER BY captured_at ASC`,
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

// DeleteDrafts permanently removes drafts by ID. Only deletes drafts with
// status 'draft' or 'staged' — committed/pushed drafts are left untouched.
func DeleteDrafts(ids []string) error {
	db := Open()
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	for _, id := range ids {
		if _, err := tx.Exec("DELETE FROM drafts WHERE id = ? AND status IN ('draft', 'staged')", id); err != nil {
			return err
		}
	}
	return tx.Commit()
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
		gitDiff               *string
		headSha               *string
	)
	err := row.Scan(
		&d.ID, &d.ContentHash, &d.SessionID,
		&projectID, &d.ProjectName, &branchName,
		&d.PromptText, &responseText, &model,
		&d.Source, &d.CaptureMethod,
		&toolCallsJSON, &fileContextJSON,
		&d.CapturedAt, &sessionCommitShasJSON,
		&d.Status, &d.CreatedAt,
		&gitDiff, &headSha,
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
	if gitDiff != nil {
		d.GitDiff = *gitDiff
	}
	if headSha != nil {
		d.HeadSha = *headSha
	}
	return d, nil
}

func nullableStr(s string) any {
	if s == "" {
		return nil
	}
	return s
}

func UpdateDraftToolCalls(sessionID, promptText string, toolCalls []map[string]any) error {
	if len(toolCalls) == 0 {
		return nil
	}
	db := Open()
	hash := supabase.PromptContentHash(sessionID, promptText, "")
	b, _ := json.Marshal(toolCalls)
	_, err := db.Exec(
		"UPDATE drafts SET tool_calls = ? WHERE content_hash = ? AND status = 'draft'",
		string(b), hash,
	)
	return err
}

func MergeDraftFileContext(sessionID, promptText string, updates map[string]any) error {
	if len(updates) == 0 {
		return nil
	}
	db := Open()
	hash := supabase.PromptContentHash(sessionID, promptText, "")
	var fcJSON *string
	if err := db.QueryRow(
		"SELECT file_context FROM drafts WHERE content_hash = ? AND status = 'draft'",
		hash,
	).Scan(&fcJSON); err != nil {
		return nil
	}
	current := map[string]any{}
	if fcJSON != nil {
		_ = json.Unmarshal([]byte(*fcJSON), &current)
	}
	for k, v := range updates {
		current[k] = v
	}
	b, _ := json.Marshal(current)
	_, err := db.Exec(
		"UPDATE drafts SET file_context = ? WHERE content_hash = ? AND status = 'draft'",
		string(b), hash,
	)
	return err
}

func UpdateDraftGitDiff(sessionID, promptText, gitDiff, headSha string) error {
	if gitDiff == "" {
		return nil
	}
	db := Open()
	hash := supabase.PromptContentHash(sessionID, promptText, "")
	_, err := db.Exec(
		"UPDATE drafts SET git_diff = ?, head_sha = COALESCE(NULLIF(head_sha,''), ?) WHERE content_hash = ? AND status = 'draft' AND (git_diff IS NULL OR git_diff = '')",
		gitDiff, headSha, hash,
	)
	return err
}

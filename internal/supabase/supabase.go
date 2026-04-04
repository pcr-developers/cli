package supabase

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"net/http"

	"github.com/pcr-developers/cli/internal/config"
)

// ─── Types ────────────────────────────────────────────────────────────────────

type PromptRecord struct {
	ID            string           `json:"id,omitempty"`
	ContentHash   string           `json:"content_hash,omitempty"`
	SessionID     string           `json:"session_id"`
	ProjectID     string           `json:"project_id,omitempty"`
	ProjectName   string           `json:"project_name,omitempty"`
	BranchName    string           `json:"branch_name,omitempty"`
	PromptText    string           `json:"prompt_text"`
	ResponseText  string           `json:"response_text,omitempty"`
	Model         string           `json:"model,omitempty"`
	Source        string           `json:"source"`
	CaptureMethod string           `json:"capture_method"`
	ToolCalls     []map[string]any `json:"tool_calls,omitempty"`
	FileContext   map[string]any   `json:"file_context,omitempty"`
	CapturedAt    string           `json:"captured_at,omitempty"`
	UserID        string           `json:"user_id,omitempty"`
	TeamID        string           `json:"team_id,omitempty"`
}

type CursorSessionData struct {
	SessionID          string
	ProjectName        string
	Branch             string
	ModelName          string
	IsAgentic          *bool
	UnifiedMode        *bool
	PlanModeUsed       *bool
	DebugModeUsed      *bool
	SchemaV            int
	ContextTokensUsed  *int
	ContextTokenLimit  *int
	FilesChangedCount  *int
	TotalLinesAdded    *int
	TotalLinesRemoved  *int
	SessionCreatedAt   *int64
	SessionUpdatedAt   *int64
	CommitShaStart     string
	CommitShaEnd       string
	CommitShas         []string
	Meta               map[string]any
}

type ClaudeBundleData struct {
	BundleID      string
	Message       string
	ProjectName   string
	BranchName    string
	SessionShas   []string
	HeadSha       string
	ExchangeCount int
	CommittedAt   string
}

// BundleData is the source-agnostic bundle descriptor used by UpsertBundle.
type BundleData struct {
	BundleID           string
	Message            string
	Source             string   // "cursor", "claude-code", etc.
	ProjectName        string
	BranchName         string
	SessionShas        []string
	HeadSha            string
	ExchangeCount      int
	CommittedAt        string
	// TouchedProjectIDs is the full set of project IDs whose files appeared
	// in this bundle's prompts. Includes the primary project_id and any
	// additional repos touched by cross-repo prompts.
	TouchedProjectIDs  []string
}

// ─── Hashing ──────────────────────────────────────────────────────────────────

// PromptContentHash returns a SHA-256 hex digest of session_id + \x00 + prompt_text + \x00 + response_text.
func PromptContentHash(sessionID, promptText, responseText string) string {
	h := sha256.Sum256([]byte(sessionID + "\x00" + promptText + "\x00" + responseText))
	return fmt.Sprintf("%x", h)
}

// PromptID formats the same hash as a UUID (8-4-4-4-12).
func PromptID(sessionID, promptText, responseText string) string {
	hex := PromptContentHash(sessionID, promptText, responseText)
	return fmt.Sprintf("%s-%s-%s-%s-%s", hex[:8], hex[8:12], hex[12:16], hex[16:20], hex[20:32])
}

// ─── HTTP RPC ─────────────────────────────────────────────────────────────────

func rpc(token, functionName string, payload any) ([]byte, error) {
	body, err := json.Marshal(payload)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequest("POST",
		config.SupabaseURL+"/rest/v1/rpc/"+functionName,
		bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("apikey", config.SupabaseKey)
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	respBody, _ := io.ReadAll(resp.Body)
	if resp.StatusCode >= 400 {
		return nil, fmt.Errorf("supabase rpc %s: %s — %s", functionName, resp.Status, string(respBody))
	}
	return respBody, nil
}

// ─── Public RPC calls ─────────────────────────────────────────────────────────

// UpsertPrompts batch-upserts prompts. Returns count inserted/updated.
func UpsertPrompts(token string, records []PromptRecord) (int, error) {
	if len(records) == 0 {
		return 0, nil
	}
	// Enrich with content_hash; clear id so DB generates it
	enriched := make([]PromptRecord, len(records))
	for i, r := range records {
		enriched[i] = r
		enriched[i].ID = ""
		if enriched[i].ContentHash == "" {
			enriched[i].ContentHash = PromptContentHash(r.SessionID, r.PromptText, r.ResponseText)
		}
	}
	data, err := rpc(token, "upsert_prompts", map[string]any{"p_records": enriched})
	if err != nil {
		return 0, err
	}
	var count int
	_ = json.Unmarshal(data, &count)
	return count, nil
}

// UpsertPrompt upserts a single prompt.
func UpsertPrompt(token string, record PromptRecord) (bool, error) {
	n, err := UpsertPrompts(token, []PromptRecord{record})
	return n > 0, err
}

// ValidateCLIToken validates a CLI token and returns the userId.
// Uses anon auth (no bearer) because the user is not yet logged in.
func ValidateCLIToken(token string) (string, error) {
	data, err := rpc("", "validate_cli_token", map[string]any{"p_token": token})
	if err != nil {
		return "", err
	}
	var userID string
	_ = json.Unmarshal(data, &userID)
	return userID, nil
}

// RegisterProject registers a project and returns its remote UUID.
// userID is passed explicitly because CLI tokens are not Supabase JWTs,
// so auth.uid() would be NULL inside the RPC.
func RegisterProject(token, name, gitRemote, localPath, userID string) (string, error) {
	data, err := rpc(token, "register_project", map[string]any{
		"p_name":       name,
		"p_git_remote": gitRemote,
		"p_local_path": localPath,
		"p_user_id":    nullableStr(userID),
	})
	if err != nil {
		return "", err
	}
	var projectID string
	_ = json.Unmarshal(data, &projectID)
	return projectID, nil
}

// UpsertCursorSession upserts Cursor session metadata.
func UpsertCursorSession(token string, data CursorSessionData, projectID, userID string) error {
	payload := map[string]any{
		"session_id":           data.SessionID,
		"project_id":           nullableStr(projectID),
		"user_id":              nullableStr(userID),
		"model_name":           nullableStr(data.ModelName),
		"branch":               nullableStr(data.Branch),
		"is_agentic":           data.IsAgentic,
		"unified_mode":         data.UnifiedMode,
		"plan_mode_used":       data.PlanModeUsed,
		"debug_mode_used":      data.DebugModeUsed,
		"cursor_schema_v":      data.SchemaV,
		"context_tokens_used":  data.ContextTokensUsed,
		"context_token_limit":  data.ContextTokenLimit,
		"files_changed_count":  data.FilesChangedCount,
		"total_lines_added":    data.TotalLinesAdded,
		"total_lines_removed":  data.TotalLinesRemoved,
		"commit_sha_start":     nullableStr(data.CommitShaStart),
		"commit_sha_end":       nullableStr(data.CommitShaEnd),
		"commit_shas":          data.CommitShas,
		"meta":                 data.Meta,
	}
	if data.SessionCreatedAt != nil {
		payload["session_created_at"] = *data.SessionCreatedAt
	}
	if data.SessionUpdatedAt != nil {
		payload["session_updated_at"] = *data.SessionUpdatedAt
	}
	_, err := rpc(token, "upsert_cursor_session", map[string]any{"p_session": payload})
	return err
}

// UpsertBundle upserts bundle metadata to the unified bundles table.
// Source should be "cursor", "claude-code", or any future source identifier.
func UpsertBundle(token string, data BundleData, projectID, userID string) (string, error) {
	// session_shas must be a JSON array (never null) because the SQL uses
	// jsonb_array_elements_text() which throws on a JSON null scalar.
	sessionShas := data.SessionShas
	if sessionShas == nil {
		sessionShas = []string{}
	}
	// touched_project_ids is embedded in p_bundle as a JSON string array
	// (same approach as session_shas) to avoid PostgREST uuid[] cast issues.
	touchedProjectIDs := data.TouchedProjectIDs
	if touchedProjectIDs == nil {
		touchedProjectIDs = []string{}
	}
	payload := map[string]any{
		"bundle_id":           data.BundleID,
		"message":             data.Message,
		"source":              data.Source,
		"project_id":          nullableStr(projectID),
		"project_name":        nullableStr(data.ProjectName),
		"branch_name":         nullableStr(data.BranchName),
		"session_shas":        sessionShas,
		"head_sha":            nullableStr(data.HeadSha),
		"exchange_count":      data.ExchangeCount,
		"committed_at":        nullableStr(data.CommittedAt),
		"touched_project_ids": touchedProjectIDs,
	}
	resp, err := rpc(token, "upsert_bundle", map[string]any{
		"p_bundle":  payload,
		"p_user_id": nullableStr(userID),
	})
	if err != nil {
		return "", err
	}
	var remoteID string
	_ = json.Unmarshal(resp, &remoteID)
	return remoteID, nil
}

// UpsertClaudeBundle upserts bundle metadata to claude_bundles (no prompt data).
func UpsertClaudeBundle(token string, data ClaudeBundleData, projectID, userID string) (string, error) {
	claudeSessionShas := data.SessionShas
	if claudeSessionShas == nil {
		claudeSessionShas = []string{}
	}
	payload := map[string]any{
		"bundle_id":      data.BundleID,
		"message":        data.Message,
		"project_id":     nullableStr(projectID),
		"project_name":   nullableStr(data.ProjectName),
		"branch_name":    nullableStr(data.BranchName),
		"session_shas":   claudeSessionShas,
		"head_sha":       nullableStr(data.HeadSha),
		"exchange_count": data.ExchangeCount,
		"committed_at":   nullableStr(data.CommittedAt),
	}
	resp, err := rpc(token, "upsert_claude_bundle", map[string]any{
		"p_bundle":  payload,
		"p_user_id": nullableStr(userID),
	})
	if err != nil {
		return "", err
	}
	var remoteID string
	_ = json.Unmarshal(resp, &remoteID)
	return remoteID, nil
}

// UpsertBundlePrompts upserts prompt rows and their git diffs for a pushed bundle.
func UpsertBundlePrompts(token string, items []map[string]any, diffs []map[string]any, userID string) error {
	if len(items) > 0 {
		if _, err := rpc(token, "upsert_prompts", map[string]any{
			"p_records": items,
			"p_user_id": nullableStr(userID),
		}); err != nil {
			return err
		}
	}
	if len(diffs) > 0 {
		if _, err := rpc(token, "upsert_git_diffs", map[string]any{
			"p_diffs": diffs,
		}); err != nil {
			return err
		}
	}
	return nil
}


// PullBundle fetches a bundle from the unified bundles table by bundle_id.
func PullBundle(token, remoteID string) (map[string]any, error) {
	url := config.SupabaseURL + "/rest/v1/bundles?bundle_id=eq." + remoteID + "&select=*&limit=1"
	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("apikey", config.SupabaseKey)
	req.Header.Set("Accept", "application/json")
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode >= 400 {
		return nil, fmt.Errorf("pull bundle: %s — %s", resp.Status, string(body))
	}
	var rows []map[string]any
	if err := json.Unmarshal(body, &rows); err != nil || len(rows) == 0 {
		return nil, fmt.Errorf("bundle %q not found", remoteID)
	}
	return rows[0], nil
}

func nullableStr(s string) any {
	if s == "" {
		return nil
	}
	return s
}

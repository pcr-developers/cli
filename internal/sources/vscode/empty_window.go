package vscode

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/pcr-developers/cli/internal/display"
	"github.com/pcr-developers/cli/internal/sources/shared"
	"github.com/pcr-developers/cli/internal/store"
	"github.com/pcr-developers/cli/internal/supabase"
	"github.com/pcr-developers/cli/internal/versions"
)

// ─── V3 JSON format (legacy) ─────────────────────────────────────────────────

type emptyWindowSession struct {
	Version       int                `json:"version"`
	SessionID     string             `json:"sessionId"`
	CreationDate  int64              `json:"creationDate"` // unix ms
	CustomTitle   string             `json:"customTitle"`
	Requests      []emptyWindowReq   `json:"requests"`
}

type emptyWindowReq struct {
	Message    emptyWindowMsg      `json:"message"`
	Response   []emptyWindowResp   `json:"response"`
	Result     *emptyWindowResult  `json:"result"`
	Agent      *emptyWindowAgent   `json:"agent"`
	IsCanceled bool                `json:"isCanceled"`
}

type emptyWindowMsg struct {
	Parts []emptyWindowPart `json:"parts"`
}

type emptyWindowPart struct {
	Text string `json:"text"`
}

type emptyWindowResp struct {
	Value string `json:"value"`
}

type emptyWindowResult struct {
	Timings  *emptyWindowTimings  `json:"timings"`
	Metadata json.RawMessage      `json:"metadata"`
}

type emptyWindowTimings struct {
	FirstProgress int64 `json:"firstProgress"`
	TotalElapsed  int64 `json:"totalElapsed"`
}

type emptyWindowAgent struct {
	ID string `json:"id"`
}

// ─── JSONL mutation log format ───────────────────────────────────────────────

type mutationEntry struct {
	Kind  int             `json:"kind"`
	Key   []string        `json:"k"`
	Value json.RawMessage `json:"v"`
}

type emptyWindowSnapshot struct {
	Version      int              `json:"version"`
	SessionID    string           `json:"sessionId"`
	CreationDate int64            `json:"creationDate"`
	Requests     []emptyWindowReq `json:"requests"`
	InputState   *inputState      `json:"inputState"`
}

type inputState struct {
	Mode          *modeInfo          `json:"mode"`
	SelectedModel *selectedModelInfo `json:"selectedModel"`
	PermLevel     string             `json:"permissionLevel"`
}

type modeInfo struct {
	ID   string `json:"id"`
	Kind string `json:"kind"`
}

type selectedModelInfo struct {
	Identifier string         `json:"identifier"`
	Metadata   *modelMetadata `json:"metadata"`
}

type modelMetadata struct {
	ID     string `json:"id"`
	Name   string `json:"name"`
	Vendor string `json:"vendor"`
}

// ProcessEmptyWindowSessions scans the emptyWindowChatSessions directory for
// sessions without a workspace and saves new exchanges as drafts.
func ProcessEmptyWindowSessions(userID string, state *shared.FileState, dedup *shared.Deduplicator) {
	globalBase := GlobalStorageBase()
	if globalBase == "" {
		return
	}
	dir := filepath.Join(globalBase, "emptyWindowChatSessions")
	entries, err := os.ReadDir(dir)
	if err != nil {
		return
	}

	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		path := filepath.Join(dir, e.Name())
		name := e.Name()

		if strings.HasSuffix(name, ".json") {
			processJSONSession(path, userID, state, dedup)
		} else if strings.HasSuffix(name, ".jsonl") {
			processJSONLSession(path, userID, state, dedup)
		}
	}
}

// processJSONSession handles the legacy v3 JSON format.
func processJSONSession(filePath, userID string, state *shared.FileState, dedup *shared.Deduplicator) {
	data, err := os.ReadFile(filePath)
	if err != nil {
		return
	}

	// Use file size as state marker
	prevSize := state.Get(filePath)
	if len(data) <= prevSize {
		return
	}
	state.Set(filePath, len(data))

	var session emptyWindowSession
	if err := json.Unmarshal(data, &session); err != nil {
		return
	}
	if session.Version < 3 || len(session.Requests) == 0 {
		return
	}

	saveEmptyWindowExchanges(session.SessionID, session.CreationDate, session.Requests, "", "", userID, dedup)
}

// processJSONLSession handles the kind-based mutation log format.
func processJSONLSession(filePath, userID string, state *shared.FileState, dedup *shared.Deduplicator) {
	data, err := os.ReadFile(filePath)
	if err != nil {
		return
	}

	prevSize := state.Get(filePath)
	if len(data) <= prevSize {
		return
	}
	state.Set(filePath, len(data))

	lines := strings.Split(strings.TrimSpace(string(data)), "\n")

	// Replay the mutation log to reconstruct the session state
	var snapshot emptyWindowSnapshot
	var modelName string

	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		var entry mutationEntry
		if err := json.Unmarshal([]byte(line), &entry); err != nil {
			continue
		}

		switch entry.Kind {
		case 0:
			// Full snapshot
			if err := json.Unmarshal(entry.Value, &snapshot); err != nil {
				continue
			}
			if snapshot.InputState != nil && snapshot.InputState.SelectedModel != nil {
				if snapshot.InputState.SelectedModel.Metadata != nil {
					modelName = snapshot.InputState.SelectedModel.Metadata.Name
				} else {
					modelName = snapshot.InputState.SelectedModel.Identifier
				}
			}
		case 1:
			// Property mutation
			if len(entry.Key) >= 2 && entry.Key[0] == "inputState" && entry.Key[1] == "selectedModel" {
				var model selectedModelInfo
				if err := json.Unmarshal(entry.Value, &model); err == nil {
					if model.Metadata != nil {
						modelName = model.Metadata.Name
					} else {
						modelName = model.Identifier
					}
				}
			}
		}
	}

	if snapshot.SessionID == "" || len(snapshot.Requests) == 0 {
		return
	}

	saveEmptyWindowExchanges(snapshot.SessionID, snapshot.CreationDate, snapshot.Requests, modelName, "", userID, dedup)
}

// saveEmptyWindowExchanges converts request/response pairs into drafts.
func saveEmptyWindowExchanges(sessionID string, creationDateMs int64, requests []emptyWindowReq, model, mode, userID string, dedup *shared.Deduplicator) {
	createdAt := time.UnixMilli(creationDateMs).UTC().Format(time.RFC3339)

	var newCount int
	for i, req := range requests {
		if req.IsCanceled {
			continue
		}
		promptText := extractEmptyWindowPrompt(req)
		if strings.TrimSpace(promptText) == "" {
			continue
		}
		responseText := extractEmptyWindowResponse(req)

		// Compute a per-exchange timestamp (offset by index if no better data)
		capturedAt := createdAt
		if creationDateMs > 0 {
			// Rough approximation: offset by request index × 30s
			t := time.UnixMilli(creationDateMs).Add(time.Duration(i) * 30 * time.Second)
			capturedAt = t.UTC().Format(time.RFC3339)
		}

		hash := supabase.PromptContentHashV2(sessionID, promptText, capturedAt)
		if dedup.IsDuplicate(sessionID, hash) {
			continue
		}
		if store.IsDraftSavedAt(sessionID, promptText, capturedAt) {
			dedup.Mark(sessionID, hash)
			continue
		}
		dedup.Mark(sessionID, hash)

		fileContext := map[string]any{
			"capture_schema": versions.CaptureSchemaVersion,
			"is_agentic":     req.Agent != nil && req.Agent.ID != "",
		}
		if req.Result != nil && req.Result.Timings != nil {
			fileContext["response_duration_ms"] = req.Result.Timings.TotalElapsed
			if req.Result.Timings.FirstProgress > 0 {
				fileContext["first_response_ms"] = req.Result.Timings.FirstProgress
			}
		}

		record := supabase.PromptRecord{
			ID:            supabase.PromptIDV2(sessionID, promptText, capturedAt),
			ContentHash:   hash,
			SessionID:     sessionID,
			PromptText:    promptText,
			ResponseText:  responseText,
			Model:         model,
			Source:        "vscode",
			CaptureMethod: "file-watcher",
			CapturedAt:    capturedAt,
			UserID:        userID,
			FileContext:   fileContext,
		}

		if err := store.SaveDraft(record, nil, ""); err != nil {
			display.PrintError("vscode", fmt.Sprintf("Failed to save empty-window draft: %s", err.Error()))
			continue
		}
		newCount++
	}

	if newCount > 0 {
		display.PrintDrafted(display.DraftDisplayOptions{
			ProjectName:   "(no workspace)",
			PromptText:    extractEmptyWindowPrompt(requests[len(requests)-1]),
			ExchangeCount: newCount,
		})
	}
}

func extractEmptyWindowPrompt(req emptyWindowReq) string {
	var parts []string
	for _, p := range req.Message.Parts {
		if p.Text != "" {
			parts = append(parts, p.Text)
		}
	}
	return strings.Join(parts, "\n")
}

func extractEmptyWindowResponse(req emptyWindowReq) string {
	var parts []string
	for _, r := range req.Response {
		if r.Value != "" {
			parts = append(parts, r.Value)
		}
	}
	return strings.Join(parts, "\n")
}

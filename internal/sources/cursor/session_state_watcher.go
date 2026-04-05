package cursor

import (
	"encoding/json"
	"fmt"
	"time"

	"github.com/pcr-developers/cli/internal/display"
	"github.com/pcr-developers/cli/internal/store"
)

// SessionStateWatcher polls Cursor's SQLite database every 2 seconds and
// records a session_state_event whenever session metadata changes (mode, model,
// context usage, etc.). This creates a timestamped change-log that lets the
// PromptScanner look up the exact mode and model that were active at the
// moment a specific user prompt was sent — even if the user switched modes
// multiple times within the same session.
type SessionStateWatcher struct {
	prevState map[string]sessionSnapshot // composerID → last recorded state
}

type sessionSnapshot struct {
	UnifiedMode       string
	ModelName         string
	ContextTokensUsed int
	ContextTokenLimit int
}

// NewSessionStateWatcher creates a watcher with no prior state. On the first
// poll it will record the current state for every active session, establishing
// a baseline that subsequent polls diff against.
func NewSessionStateWatcher() *SessionStateWatcher {
	return &SessionStateWatcher{
		prevState: map[string]sessionSnapshot{},
	}
}

// Start launches the polling loop. Call as go w.Start().
func (w *SessionStateWatcher) Start() {
	ticker := time.NewTicker(2 * time.Second)
	defer ticker.Stop()
	for range ticker.C {
		w.poll()
	}
}

func (w *SessionStateWatcher) poll() {
	db := openCursorDB()
	if db == nil {
		return
	}

	// Query all composerData entries to find active sessions.
	rows, err := db.Query(`
		SELECT
		  json_extract(value, '$.composerId')   as composer_id,
		  json_extract(value, '$.unifiedMode')  as unified_mode,
		  json_extract(value, '$.modelConfig')  as model_config,
		  json_extract(value, '$.contextTokensUsed')  as ctx_used,
		  json_extract(value, '$.contextTokenLimit')  as ctx_limit,
		  json_extract(value, '$.lastUpdatedAt') as last_updated
		FROM cursorDiskKV
		WHERE key LIKE 'composerData:%'
		  AND json_extract(value, '$.composerId') IS NOT NULL
		  AND json_extract(value, '$.lastUpdatedAt') IS NOT NULL
		ORDER BY last_updated DESC
		LIMIT 50
	`)
	if err != nil {
		return
	}
	defer rows.Close()

	now := time.Now()

	for rows.Next() {
		var (
			composerID  string
			unifiedMode *string
			modelConfig *string
			ctxUsed     *float64
			ctxLimit    *float64
			lastUpdated *float64
		)
		if err := rows.Scan(&composerID, &unifiedMode, &modelConfig, &ctxUsed, &ctxLimit, &lastUpdated); err != nil {
			continue
		}
		if composerID == "" {
			continue
		}

		// Parse model name from JSON config.
		modelName := ""
		if modelConfig != nil {
			var mc map[string]any
			if json.Unmarshal([]byte(*modelConfig), &mc) == nil {
				if mn, ok := mc["modelName"].(string); ok {
					modelName = mn
				}
			}
		}

		mode := ""
		if unifiedMode != nil {
			mode = *unifiedMode
		}

		ctxUsedInt := 0
		if ctxUsed != nil {
			ctxUsedInt = int(*ctxUsed)
		}
		ctxLimitInt := 0
		if ctxLimit != nil {
			ctxLimitInt = int(*ctxLimit)
		}

		snap := sessionSnapshot{
			UnifiedMode:       mode,
			ModelName:         modelName,
			ContextTokensUsed: ctxUsedInt,
			ContextTokenLimit: ctxLimitInt,
		}

		prev, known := w.prevState[composerID]
		if known && prev == snap {
			continue // nothing changed
		}

		// Something changed (or this is a newly seen session) — record it.
		w.prevState[composerID] = snap
		_ = store.RecordSessionStateEvent(store.SessionStateEvent{
			SessionID:         composerID,
			OccurredAt:        now,
			UnifiedMode:       mode,
			ModelName:         modelName,
			ContextTokensUsed: ctxUsedInt,
			ContextTokenLimit: ctxLimitInt,
		})

		if known {
			// Print what specifically changed (only on updates, not first-seen).
			if prev.UnifiedMode != snap.UnifiedMode && snap.UnifiedMode != "" {
				display.PrintVerboseEvent("session", fmt.Sprintf("[%s]  mode  %s → %s",
					composerID[:8], prev.UnifiedMode, snap.UnifiedMode))
			}
			if prev.ModelName != snap.ModelName && snap.ModelName != "" {
				display.PrintVerboseEvent("session", fmt.Sprintf("[%s]  model  %s → %s",
					composerID[:8], prev.ModelName, snap.ModelName))
			}
			if prev.ContextTokensUsed != snap.ContextTokensUsed && snap.ContextTokenLimit > 0 {
				pct := 100 * snap.ContextTokensUsed / snap.ContextTokenLimit
				display.PrintVerboseEvent("session", fmt.Sprintf("[%s]  context  %d/%d (%d%%)",
					composerID[:8], snap.ContextTokensUsed, snap.ContextTokenLimit, pct))
			}
		}
	}
}

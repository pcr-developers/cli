package cursor

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"sync"
	"time"

	_ "modernc.org/sqlite"
)

// ─── Types ────────────────────────────────────────────────────────────────────

type BubbleMeta struct {
	Type           int      `json:"type"` // 1=user, 2=assistant
	BubbleID       string   `json:"bubbleId,omitempty"`
	Text           string   `json:"text,omitempty"`
	CreatedAt      string   `json:"createdAt,omitempty"`      // ISO8601 from v14+
	TurnDurationMs *int64   `json:"turnDurationMs,omitempty"` // ms; set on final assistant bubble when turn completes
	IsAgentic      *bool    `json:"isAgentic,omitempty"`
	RelevantFiles  []string `json:"relevantFiles,omitempty"`
	UnifiedMode    string   `json:"unifiedMode,omitempty"`
}

type SessionMeta struct {
	Bubbles     []BubbleMeta `json:"bubbles"`
	ModelName   string       `json:"modelName,omitempty"`
	IsAgentic   bool         `json:"isAgentic"`
	UnifiedMode string       `json:"unifiedMode,omitempty"`
	ComposerID  string       `json:"composerId,omitempty"`
}

type SessionData struct {
	SessionID         string
	SchemaV           int
	Name              string
	Subtitle          string
	ModelName         string
	IsAgentic         bool
	UnifiedMode       string
	PlanModeUsed      *bool
	DebugModeUsed     *bool
	Branch            string
	ContextTokensUsed *int
	ContextTokenLimit *int
	FilesChangedCount *int
	TotalLinesAdded   *int
	TotalLinesRemoved *int
	SessionCreatedAt  *int64
	SessionUpdatedAt  *int64
	Meta              map[string]any
}

// ─── DB singleton ─────────────────────────────────────────────────────────────

var (
	cursorDB        *sql.DB
	cursorDBOnce    sync.Once
	cursorDBUnavail bool
)

func getCursorDBPath() string {
	home, _ := os.UserHomeDir()
	switch runtime.GOOS {
	case "darwin":
		return home + "/Library/Application Support/Cursor/User/globalStorage/state.vscdb"
	case "windows":
		appData := os.Getenv("APPDATA")
		if appData == "" {
			appData = home + "/AppData/Roaming"
		}
		return appData + "/Cursor/User/globalStorage/state.vscdb"
	default:
		return home + "/.config/Cursor/User/globalStorage/state.vscdb"
	}
}

func openCursorDB() *sql.DB {
	cursorDBOnce.Do(func() {
		path := getCursorDBPath()
		if _, err := os.Stat(path); os.IsNotExist(err) {
			cursorDBUnavail = true
			return
		}
		db, err := sql.Open("sqlite", fmt.Sprintf("file:%s?mode=ro&_mutex=no", path))
		if err != nil {
			cursorDBUnavail = true
			return
		}
		cursorDB = db
	})
	if cursorDBUnavail {
		return nil
	}
	return cursorDB
}

// ─── Cache ────────────────────────────────────────────────────────────────────

type cacheEntry struct {
	meta  *SessionMeta
	ts    time.Time
}

var (
	metaCache   = map[string]cacheEntry{}
	metaCacheMu sync.RWMutex
	cacheTTL    = 60 * time.Second
)

// InvalidateSessionCache removes a session from the metadata cache.
// Called by the watcher when a transcript file changes.
func InvalidateSessionCache(sessionID string) {
	metaCacheMu.Lock()
	delete(metaCache, sessionID)
	metaCacheMu.Unlock()
}

// ─── GetSessionMeta ───────────────────────────────────────────────────────────

func GetSessionMeta(sessionID string) *SessionMeta {
	metaCacheMu.RLock()
	if e, ok := metaCache[sessionID]; ok && time.Since(e.ts) < cacheTTL {
		metaCacheMu.RUnlock()
		return e.meta
	}
	metaCacheMu.RUnlock()

	db := openCursorDB()
	if db == nil {
		return storeMetaCache(sessionID, nil)
	}

	var (
		isAgentic   *int
		unifiedMode *string
		modelConfig *string
		headersOnly *string
	)

	err := db.QueryRow(`
		SELECT
		  json_extract(value, '$.isAgentic')                   as is_agentic,
		  json_extract(value, '$.unifiedMode')                 as unified_mode,
		  json_extract(value, '$.modelConfig')                 as model_config,
		  json_extract(value, '$.fullConversationHeadersOnly') as headers_only
		FROM cursorDiskKV
		WHERE key = ?
	`, "composerData:"+sessionID).Scan(
		&isAgentic, &unifiedMode, &modelConfig, &headersOnly,
	)
	if err != nil {
		return storeMetaCache(sessionID, nil)
	}

	agentic := isAgentic != nil && *isAgentic == 1

	var modelName string
	if modelConfig != nil {
		var mc map[string]any
		if json.Unmarshal([]byte(*modelConfig), &mc) == nil {
			if mn, ok := mc["modelName"].(string); ok {
				modelName = mn
			}
		}
	}

	um := ""
	if unifiedMode != nil {
		um = *unifiedMode
	}

	var composerID string
	_ = db.QueryRow(`SELECT json_extract(value, '$.composerId') FROM cursorDiskKV WHERE key = ?`,
		"composerData:"+sessionID).Scan(&composerID)

	var bubbles []BubbleMeta
	if headersOnly != nil {
		var headers []struct {
			BubbleID string `json:"bubbleId"`
			Type     int    `json:"type"`
		}
		if json.Unmarshal([]byte(*headersOnly), &headers) == nil {
			for _, h := range headers {
				b := BubbleMeta{Type: h.Type, BubbleID: h.BubbleID}
				if composerID != "" && h.BubbleID != "" {
					var bval string
					bkey := "bubbleId:" + composerID + ":" + h.BubbleID
					if err := db.QueryRow(`SELECT value FROM cursorDiskKV WHERE key = ?`, bkey).Scan(&bval); err == nil {
						var bd map[string]any
						if json.Unmarshal([]byte(bval), &bd) == nil {
							b.Text = getString(bd, "text")
							b.CreatedAt = getString(bd, "createdAt")
							b.UnifiedMode = getString(bd, "unifiedMode")
							if rf, ok := bd["relevantFiles"].([]any); ok {
								for _, f := range rf {
									if s, ok := f.(string); ok {
										b.RelevantFiles = append(b.RelevantFiles, s)
									}
								}
							}
							if ag, ok := bd["isAgentic"].(bool); ok {
								b.IsAgentic = &ag
							}
							if dur, ok := bd["turnDurationMs"].(float64); ok {
								ms := int64(dur)
								b.TurnDurationMs = &ms
							}
						}
					}
				}
				bubbles = append(bubbles, b)
			}
		}
	}

	return storeMetaCache(sessionID, &SessionMeta{
		Bubbles:     bubbles,
		ModelName:   modelName,
		IsAgentic:   agentic,
		UnifiedMode: um,
		ComposerID:  composerID,
	})
}

// GetSessionsForWorkspace returns all session IDs (composer IDs) that belong
// to the given Cursor workspace slug. Used by the PromptScanner to discover
// all sessions in the current workspace on each polling cycle.
func GetSessionsForWorkspace(slug string) []string {
	db := openCursorDB()
	if db == nil {
		return nil
	}
	// composerData keys match the workspace via the JSONL transcript path pattern:
	// ~/.cursor/projects/<slug>/agent-transcripts/<sessionID>/<sessionID>.jsonl
	// We can't directly join composerData to slug, so we use the agent-transcripts
	// directory listing (done by the caller) and look up each composerData by ID.
	// This function provides a convenience: given a known set of session IDs,
	// it just returns the ones that exist in the DB.
	// The actual slug→sessionIDs mapping is done by the PromptScanner via filesystem.
	_ = slug
	return nil // stub — slug→sessionID mapping is done via filesystem walk in the scanner
}

func storeMetaCache(sessionID string, meta *SessionMeta) *SessionMeta {
	metaCacheMu.Lock()
	metaCache[sessionID] = cacheEntry{meta: meta, ts: time.Now()}
	metaCacheMu.Unlock()
	return meta
}

// ─── GetFullSessionData ───────────────────────────────────────────────────────

var strippedFields = map[string]bool{
	"fullConversationHeadersOnly":          true,
	"conversationMap":                      true,
	"conversationState":                    true,
	"blobEncryptionKey":                    true,
	"speculativeSummarizationEncryptionKey": true,
	"richText":                             true,
	"generatingBubbleIds":                  true,
	"codeBlockData":                        true,
	"originalFileStates":                   true,
}

var structuredFields = map[string]bool{
	"_v": true, "composerId": true, "isAgentic": true, "unifiedMode": true,
	"forceMode": true, "modelConfig": true, "name": true, "subtitle": true,
	"planModeSuggestionUsed": true, "debugModeSuggestionUsed": true,
	"contextTokensUsed": true, "contextTokenLimit": true,
	"filesChangedCount": true, "totalLinesAdded": true, "totalLinesRemoved": true,
	"activeBranch": true, "createdOnBranch": true, "createdAt": true, "lastUpdatedAt": true,
}

func GetFullSessionData(sessionID string) *SessionData {
	db := openCursorDB()
	if db == nil {
		return nil
	}

	var value string
	err := db.QueryRow("SELECT value FROM cursorDiskKV WHERE key = ?", "composerData:"+sessionID).Scan(&value)
	if err != nil {
		return nil
	}

	var obj map[string]any
	if err := json.Unmarshal([]byte(value), &obj); err != nil {
		return nil
	}

	sd := &SessionData{SessionID: sessionID}
	sd.SchemaV = int(getFloat(obj, "_v"))
	sd.IsAgentic = getBool(obj, "isAgentic")
	sd.UnifiedMode = getString(obj, "unifiedMode")
	sd.Name = getString(obj, "name")
	sd.Subtitle = getString(obj, "subtitle")

	if mc, ok := obj["modelConfig"].(map[string]any); ok {
		sd.ModelName = getString(mc, "modelName")
	}
	if ab, ok := obj["activeBranch"].(map[string]any); ok {
		sd.Branch = getString(ab, "branchName")
	}

	if v := getIntPtr(obj, "planModeSuggestionUsed"); v != nil {
		b := *v == 1
		sd.PlanModeUsed = &b
	}
	if v := getIntPtr(obj, "debugModeSuggestionUsed"); v != nil {
		b := *v == 1
		sd.DebugModeUsed = &b
	}
	if v := getIntPtr(obj, "contextTokensUsed"); v != nil {
		sd.ContextTokensUsed = v
	}
	if v := getIntPtr(obj, "contextTokenLimit"); v != nil {
		sd.ContextTokenLimit = v
	}
	if v := getIntPtr(obj, "filesChangedCount"); v != nil {
		sd.FilesChangedCount = v
	}
	if v := getIntPtr(obj, "totalLinesAdded"); v != nil {
		sd.TotalLinesAdded = v
	}
	if v := getIntPtr(obj, "totalLinesRemoved"); v != nil {
		sd.TotalLinesRemoved = v
	}
	if v := getInt64Ptr(obj, "createdAt"); v != nil {
		sd.SessionCreatedAt = v
	}
	if v := getInt64Ptr(obj, "lastUpdatedAt"); v != nil {
		sd.SessionUpdatedAt = v
	}

	meta := map[string]any{}
	for k, v := range obj {
		if !structuredFields[k] && !strippedFields[k] && v != nil {
			meta[k] = v
		}
	}
	sd.Meta = meta

	return sd
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

func getFloat(m map[string]any, key string) float64 {
	if v, ok := m[key].(float64); ok {
		return v
	}
	return 0
}

func getString(m map[string]any, key string) string {
	if v, ok := m[key].(string); ok {
		return v
	}
	return ""
}

func getBool(m map[string]any, key string) bool {
	switch v := m[key].(type) {
	case bool:
		return v
	case float64:
		return v == 1
	}
	return false
}

func getIntPtr(m map[string]any, key string) *int {
	if v, ok := m[key].(float64); ok {
		i := int(v)
		return &i
	}
	return nil
}

func getInt64Ptr(m map[string]any, key string) *int64 {
	if v, ok := m[key].(float64); ok {
		i := int64(v)
		return &i
	}
	return nil
}


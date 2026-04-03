package cursor

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"
	"github.com/pcr-developers/cli/internal/display"
	"github.com/pcr-developers/cli/internal/projects"
	"github.com/pcr-developers/cli/internal/sources/shared"
	"github.com/pcr-developers/cli/internal/store"
	"github.com/pcr-developers/cli/internal/supabase"
	"github.com/pcr-developers/cli/internal/versions"
)

const sourceID = "cursor"

type Watcher struct {
	dir    string
	userID string
	state  *shared.FileState
	dedup  *shared.Deduplicator

	timerMu sync.Mutex
	timers  map[string]*time.Timer
}

func NewWatcher(dir, userID string) *Watcher {
	return &Watcher{
		dir:    dir,
		userID: userID,
		state:  shared.NewFileState(sourceID),
		dedup:  shared.NewDeduplicator(),
		timers: map[string]*time.Timer{},
	}
}

func (w *Watcher) Start() {
	w.state.Load()

	if _, err := os.Stat(w.dir); os.IsNotExist(err) {
		display.PrintError("cursor", "Directory not found: "+w.dir+". Will activate when it appears.")
	}
	display.PrintWatcherReady("Cursor", w.dir)

	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		display.PrintError("cursor", "Failed to create watcher: "+err.Error())
		return
	}
	defer watcher.Close()

	// Initial scan + recursive dir registration
	_ = filepath.WalkDir(w.dir, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			_ = watcher.Add(path)
		} else if isAgentTranscript(path) {
			w.processFile(path, true)
		}
		return nil
	})

	for {
		select {
		case event, ok := <-watcher.Events:
			if !ok {
				return
			}
			if event.Has(fsnotify.Create) {
				info, err := os.Stat(event.Name)
				if err == nil && info.IsDir() {
					_ = watcher.Add(event.Name)
				}
			}
			if (event.Has(fsnotify.Write) || event.Has(fsnotify.Create)) && isAgentTranscript(event.Name) {
				w.scheduleProcess(event.Name)
			}
		case err, ok := <-watcher.Errors:
			if !ok {
				return
			}
			display.PrintError("cursor watcher", err.Error())
		}
	}
}

func isAgentTranscript(path string) bool {
	return strings.HasSuffix(path, ".jsonl") &&
		strings.Contains(path, "/agent-transcripts/") &&
		!strings.Contains(path, "/subagents/")
}

func (w *Watcher) scheduleProcess(path string) {
	w.timerMu.Lock()
	defer w.timerMu.Unlock()
	if t, ok := w.timers[path]; ok {
		t.Reset(1500 * time.Millisecond)
	} else {
		w.timers[path] = time.AfterFunc(1500*time.Millisecond, func() {
			w.timerMu.Lock()
			delete(w.timers, path)
			w.timerMu.Unlock()
			w.processFile(path, false)
		})
	}
}

// parseTranscriptPath extracts projectSlug and sessionUUID from the file path.
// Pattern: ~/.cursor/projects/<slug>/agent-transcripts/<uuid>/<uuid>.jsonl
func parseTranscriptPath(filePath string) (projectSlug, sessionUUID string, ok bool) {
	parts := strings.Split(filePath, "/")
	for i, p := range parts {
		if p == "agent-transcripts" && i >= 1 {
			projectSlug = parts[i-1]
			sessionUUID = strings.TrimSuffix(filepath.Base(filePath), ".jsonl")
			return projectSlug, sessionUUID, true
		}
	}
	return "", "", false
}

func (w *Watcher) processFile(filePath string, forceFullScan bool) {
	projectSlug, sessionUUID, ok := parseTranscriptPath(filePath)
	if !ok {
		return
	}

	project := projects.GetBestProjectForCursorSlug(projectSlug)
	if project == nil {
		return
	}

	content, err := os.ReadFile(filePath)
	if err != nil {
		return
	}

	lines := filterNonEmpty(strings.Split(strings.TrimSpace(string(content)), "\n"))
	prevCount := w.state.Get(filePath)
	if !forceFullScan && len(lines) <= prevCount {
		return
	}
	w.state.Set(filePath, len(lines))

	session := ParseCursorTranscript(string(content), sessionUUID, projectSlug)
	if len(session.Prompts) == 0 {
		return
	}

	sessionMeta := GetSessionMeta(sessionUUID)
	var assistantBubbles []BubbleMeta
	if sessionMeta != nil {
		for _, b := range sessionMeta.Bubbles {
			if b.Type == 2 {
				assistantBubbles = append(assistantBubbles, b)
			}
		}
	}

	schemaV := versions.CaptureSchemaVersion
	var newPrompts []supabase.PromptRecord
	promptIdx := 0

	for _, p := range session.Prompts {
		hash := supabase.PromptContentHash(p.SessionID, p.PromptText, "")
		if w.dedup.IsDuplicate(p.SessionID, hash) {
			continue
		}
		if store.IsDraftSaved(p.SessionID, p.PromptText) {
			w.dedup.Mark(p.SessionID, hash)
			continue
		}
		w.dedup.Mark(p.SessionID, hash)

		fileContext := map[string]any{
			"capture_schema": schemaV,
		}

		var bubble *BubbleMeta
		if promptIdx < len(assistantBubbles) {
			b := assistantBubbles[promptIdx]
			bubble = &b
		}

		if bubble != nil && bubble.IsAgentic != nil {
			fileContext["is_agentic"] = *bubble.IsAgentic
		} else if sessionMeta != nil {
			fileContext["is_agentic"] = sessionMeta.IsAgentic
		}
		if sessionMeta != nil && sessionMeta.UnifiedMode != "" {
			fileContext["cursor_mode"] = sessionMeta.UnifiedMode
		}
		if bubble != nil && bubble.ResponseDurationMs != nil {
			fileContext["response_duration_ms"] = *bubble.ResponseDurationMs
		}
		if bubble != nil && len(bubble.RelevantFiles) > 0 {
			fileContext["relevant_files"] = bubble.RelevantFiles
		}

		capturedAt := p.CapturedAt
		if bubble != nil && bubble.SubmittedAt != nil {
			capturedAt = time.UnixMilli(*bubble.SubmittedAt).UTC().Format(time.RFC3339)
		}

		model := p.Model
		if sessionMeta != nil && sessionMeta.ModelName != "" {
			model = sessionMeta.ModelName
		}

		p.CapturedAt = capturedAt
		p.Model = model
		p.UserID = w.userID
		p.ProjectID = project.ProjectID
		p.ProjectName = project.Name
		p.FileContext = fileContext
		newPrompts = append(newPrompts, p)
		promptIdx++
	}

	if len(newPrompts) == 0 {
		return
	}

	// Get commit range from Cursor's session data
	var commitShas []string
	fullSession := GetFullSessionData(sessionUUID)
	if fullSession != nil && project.Path != "" {
		commits := getCommitRange(project.Path, fullSession.SessionCreatedAt, fullSession.SessionUpdatedAt)
		commitShas = commits

		// Upsert cursor session metadata (non-fatal)
		if w.userID != "" {
			var startSha, endSha string
			if len(commits) > 0 {
				endSha = commits[0]
				startSha = commits[len(commits)-1]
			}
			planMode := fullSession.PlanModeUsed
			debugMode := fullSession.DebugModeUsed
			_ = supabase.UpsertCursorSession(w.userID, supabase.CursorSessionData{
				SessionID:         sessionUUID,
				Branch:            fullSession.Branch,
				ModelName:         fullSession.ModelName,
				IsAgentic:         boolPtr(fullSession.IsAgentic),
				UnifiedMode:       boolPtrFromStr(fullSession.UnifiedMode),
				PlanModeUsed:      planMode,
				DebugModeUsed:     debugMode,
				SchemaV:           fullSession.SchemaV,
				ContextTokensUsed: fullSession.ContextTokensUsed,
				ContextTokenLimit: fullSession.ContextTokenLimit,
				FilesChangedCount: fullSession.FilesChangedCount,
				TotalLinesAdded:   fullSession.TotalLinesAdded,
				TotalLinesRemoved: fullSession.TotalLinesRemoved,
				SessionCreatedAt:  fullSession.SessionCreatedAt,
				SessionUpdatedAt:  fullSession.SessionUpdatedAt,
				CommitShaStart:    startSha,
				CommitShaEnd:      endSha,
				CommitShas:        commits,
				Meta:              fullSession.Meta,
			}, project.ProjectID, w.userID)
		}
	}

	for _, p := range newPrompts {
		if err := store.SaveDraft(p, commitShas); err != nil {
			display.PrintError("cursor", "Failed to save draft: "+err.Error())
		}
	}

	last := newPrompts[len(newPrompts)-1]
	branch := ""
	if fullSession != nil {
		branch = fullSession.Branch
	}
	if w.userID == "" {
		display.PrintDrafted(display.DraftDisplayOptions{
			ProjectName:   project.Name,
			Branch:        branch,
			PromptText:    last.PromptText,
			ExchangeCount: len(newPrompts),
		})
	} else {
		display.PrintCaptured(display.CaptureDisplayOptions{
			ProjectName:   project.Name,
			Branch:        branch,
			PromptText:    last.PromptText,
			ExchangeCount: len(newPrompts),
		})
	}
}

func getCommitRange(projectPath string, since, until *int64) []string {
	args := []string{"log", "--format=%H", "--no-merges"}
	if since != nil {
		args = append(args, "--after="+time.UnixMilli(*since).UTC().Format(time.RFC3339))
	}
	if until != nil {
		args = append(args, "--before="+time.UnixMilli(*until).UTC().Format(time.RFC3339))
	}
	cmd := exec.Command("git", args...)
	cmd.Dir = projectPath
	out, err := cmd.Output()
	if err != nil {
		return nil
	}
	return filterNonEmpty(strings.Split(strings.TrimSpace(string(out)), "\n"))
}

func filterNonEmpty(lines []string) []string {
	var result []string
	for _, l := range lines {
		if strings.TrimSpace(l) != "" {
			result = append(result, l)
		}
	}
	return result
}

func boolPtr(b bool) *bool { return &b }

func boolPtrFromStr(s string) *bool {
	if s == "" {
		return nil
	}
	b := s != ""
	return &b
}

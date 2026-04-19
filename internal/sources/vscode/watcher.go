package vscode

import (
	"os"
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
)

// Watcher monitors VS Code Copilot Chat transcript files for new exchanges.
type Watcher struct {
	userID     string
	state      *shared.FileState
	dedup      *shared.Deduplicator
	workspaces []WorkspaceMatch

	// Self-capture prevention: skip transcripts belonging to this session.
	selfSessionID string

	timerMu sync.Mutex
	timers  map[string]*time.Timer
}

// NewWatcher creates a watcher for the given workspace matches.
func NewWatcher(userID string, workspaces []WorkspaceMatch) *Watcher {
	// Detect self-session from VSCODE_TARGET_SESSION_LOG env var.
	// Format: /path/to/debug-logs/<sessionId>/main.jsonl
	selfSessionID := ""
	if logPath := os.Getenv("VSCODE_TARGET_SESSION_LOG"); logPath != "" {
		dir := filepath.Dir(logPath)
		selfSessionID = filepath.Base(dir)
	}

	return &Watcher{
		userID:        userID,
		state:         shared.NewFileState("vscode"),
		dedup:         shared.NewDeduplicator(),
		workspaces:    workspaces,
		selfSessionID: selfSessionID,
		timers:        map[string]*time.Timer{},
	}
}

// Start begins watching transcript directories. Blocks until stopped.
func (w *Watcher) Start() {
	w.state.Load()

	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		display.PrintError("vscode", "Failed to create watcher: "+err.Error())
		return
	}
	defer watcher.Close()

	// Register existing transcript directories and set baselines.
	for _, ws := range w.workspaces {
		w.watchTranscriptDir(watcher, ws.TranscriptDir)
	}

	// Also watch the parent workspaceStorage dirs for new workspace hashes.
	parentDirs := map[string]bool{}
	for _, base := range workspaceStorageBases() {
		if !parentDirs[base] {
			parentDirs[base] = true
			_ = watcher.Add(base)
		}
	}

	for {
		select {
		case event, ok := <-watcher.Events:
			if !ok {
				return
			}
			if event.Has(fsnotify.Create) {
				info, err := os.Stat(event.Name)
				if err == nil && info.IsDir() {
					// New directory — could be a new workspace hash or transcript dir.
					_ = watcher.Add(event.Name)
					w.handleNewDir(watcher, event.Name)
				}
			}
			if (event.Has(fsnotify.Write) || event.Has(fsnotify.Create)) &&
				strings.HasSuffix(event.Name, ".jsonl") {
				w.scheduleProcess(event.Name)
			}
		case err, ok := <-watcher.Errors:
			if !ok {
				return
			}
			display.PrintError("vscode watcher", err.Error())
		}
	}
}

// watchTranscriptDir registers a transcript directory for watching and sets
// initial baselines for existing files.
func (w *Watcher) watchTranscriptDir(watcher *fsnotify.Watcher, dir string) {
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		// Directory doesn't exist yet — watch parent instead.
		parent := filepath.Dir(dir)
		if _, err := os.Stat(parent); err == nil {
			_ = watcher.Add(parent)
		}
		return
	}

	_ = watcher.Add(dir)
	display.PrintWatcherReady("VS Code", dir)

	// Set baselines for existing files — don't process existing content.
	entries, err := os.ReadDir(dir)
	if err != nil {
		return
	}
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".jsonl") {
			continue
		}
		path := filepath.Join(dir, e.Name())
		content, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		lines := shared.FilterNonEmpty(strings.Split(strings.TrimSpace(string(content)), "\n"))
		w.state.Set(path, len(lines))
	}
}

// handleNewDir checks if a newly created directory is a transcript directory
// belonging to a matched workspace.
func (w *Watcher) handleNewDir(watcher *fsnotify.Watcher, dirPath string) {
	// Check if this is a transcript directory
	if filepath.Base(dirPath) == "transcripts" {
		_ = watcher.Add(dirPath)
		return
	}

	// Check if this is a new workspace hash directory
	wsFile := filepath.Join(dirPath, "workspace.json")
	if _, err := os.Stat(wsFile); err != nil {
		return
	}

	// Try to match against registered projects
	newMatches := ScanWorkspaces()
	for _, nm := range newMatches {
		// Check if already in our list
		found := false
		for _, existing := range w.workspaces {
			if existing.Hash == nm.Hash {
				found = true
				break
			}
		}
		if !found {
			w.workspaces = append(w.workspaces, nm)
			w.watchTranscriptDir(watcher, nm.TranscriptDir)
		}
	}
}

// scheduleProcess debounces processFile calls — waits 1s of file quiet.
func (w *Watcher) scheduleProcess(path string) {
	w.timerMu.Lock()
	defer w.timerMu.Unlock()
	if t, ok := w.timers[path]; ok {
		t.Reset(1 * time.Second)
	} else {
		w.timers[path] = time.AfterFunc(1*time.Second, func() {
			w.timerMu.Lock()
			delete(w.timers, path)
			w.timerMu.Unlock()
			w.processFile(path)
		})
	}
}

// processFile reads new lines from a transcript file and saves any new exchanges.
func (w *Watcher) processFile(filePath string) {
	// Find the workspace match for this transcript file
	ws := w.findWorkspace(filePath)
	if ws == nil {
		return
	}

	content, err := os.ReadFile(filePath)
	if err != nil {
		return
	}

	lines := shared.FilterNonEmpty(strings.Split(strings.TrimSpace(string(content)), "\n"))
	prevCount := w.state.Get(filePath)

	if len(lines) <= prevCount {
		return
	}
	w.state.Set(filePath, len(lines))

	// Parse the full transcript to get all exchanges
	transcript := ParseTranscript(string(content))

	// Self-capture prevention
	if w.selfSessionID != "" && transcript.SessionID == w.selfSessionID {
		return
	}

	if len(transcript.Exchanges) == 0 {
		return
	}

	// Build project ID → path map for multi-repo attribution
	projByID := map[string]string{}
	for _, p := range projects.Load() {
		if p.ProjectID != "" && p.Path != "" {
			projByID[p.ProjectID] = p.Path
		}
	}

	// Build path → project index for quick lookup
	projByPath := map[string]int{}
	for i, p := range ws.Projects {
		if p.Path != "" {
			projByPath[p.Path] = i
		}
	}

	var newCount int
	var lastName, lastPrompt string
	var lastToolCalls []map[string]any
	var lastBranch string

	for _, ex := range transcript.Exchanges {
		hash := supabase.PromptContentHashV2(transcript.SessionID, ex.PromptText, ex.CapturedAt)
		if w.dedup.IsDuplicate(transcript.SessionID, hash) {
			w.updateExistingDraft(transcript, ex, ws, projByPath, projByID)
			continue
		}
		if store.IsDraftSavedAt(transcript.SessionID, ex.PromptText, ex.CapturedAt) {
			w.dedup.Mark(transcript.SessionID, hash)
			w.updateExistingDraft(transcript, ex, ws, projByPath, projByID)
			continue
		}

		// Determine the primary project for THIS exchange based on tool calls.
		// Returns nil if no tool calls match any registered project.
		primary := w.projectForExchange(ex, ws.Projects, projByPath)

		var projName, projID, branch string
		var projPath string
		if primary != nil {
			projName = primary.Name
			projID = primary.ProjectID
			projPath = primary.Path
			branch = shared.GetBranch(projPath)
		}

		record := ExchangeToPromptRecord(ex, transcript.SessionID, projName, projID, branch)
		record.UserID = w.userID
		record.ID = supabase.PromptIDV2(transcript.SessionID, ex.PromptText, ex.CapturedAt)
		record.ContentHash = hash

		if transcript.CopilotVersion != "" {
			record.FileContext["copilot_version"] = transcript.CopilotVersion
		}
		if transcript.VSCodeVersion != "" {
			record.FileContext["vscode_version"] = transcript.VSCodeVersion
		}

		touchedIDs := shared.TouchedProjectIDs(ex.ToolCalls, projByID)
		if len(touchedIDs) > 1 {
			record.FileContext["touched_project_ids"] = touchedIDs
		}

		var gitDiff, headSha string
		var commitShas []string
		if projPath != "" {
			gitDiff = shared.GetGitDiff(projPath)
			headSha = shared.GetHeadSha(projPath)
			if transcript.StartTime != "" {
				commitShas = shared.GetCommitsSince(projPath, transcript.StartTime)
			}
		}

		if err := store.SaveDraft(record, commitShas, gitDiff, headSha); err != nil {
			display.PrintError("vscode", "Failed to save draft: "+err.Error())
			continue
		}
		w.dedup.Mark(transcript.SessionID, hash)
		newCount++
		lastName = projName
		lastPrompt = ex.PromptText
		lastToolCalls = ex.ToolCalls
		lastBranch = branch
	}

	if newCount == 0 {
		return
	}

	if w.userID == "" {
		display.PrintDrafted(display.DraftDisplayOptions{
			ProjectName:   lastName,
			Branch:        lastBranch,
			PromptText:    lastPrompt,
			ExchangeCount: newCount,
		})
	} else {
		display.PrintCaptured(display.CaptureDisplayOptions{
			ProjectName:   lastName,
			Branch:        lastBranch,
			PromptText:    lastPrompt,
			ToolCalls:     lastToolCalls,
			ExchangeCount: newCount,
		})
	}
}

// updateExistingDraft enriches an already-saved draft with the latest response,
// tool calls, file_context metadata, and git diff from the current parse.
func (w *Watcher) updateExistingDraft(transcript ParsedTranscript, ex ParsedExchange, ws *WorkspaceMatch, projByPath map[string]int, projByID map[string]string) {
	_ = store.UpdateDraftResponse(transcript.SessionID, ex.PromptText, ex.ResponseText)
	_ = store.UpdateDraftToolCalls(transcript.SessionID, ex.PromptText, ex.ToolCalls)

	// Merge file_context updates (duration, changed_files, etc.)
	updates := map[string]any{}
	if ex.DurationMs > 0 {
		updates["response_duration_ms"] = ex.DurationMs
	}
	if ex.FirstResponseMs > 0 {
		updates["first_response_ms"] = ex.FirstResponseMs
	}
	if len(ex.ChangedFiles) > 0 {
		updates["changed_files"] = ex.ChangedFiles
	}
	if len(ex.RelevantFiles) > 0 {
		updates["relevant_files"] = ex.RelevantFiles
	}
	if ex.ReasoningText != "" {
		updates["reasoning_text"] = ex.ReasoningText
	}
	if len(ex.ToolCalls) > 0 {
		updates["is_agentic"] = true
	}
	touchedIDs := shared.TouchedProjectIDs(ex.ToolCalls, projByID)
	if len(touchedIDs) > 1 {
		updates["touched_project_ids"] = touchedIDs
	}
	_ = store.MergeDraftFileContext(transcript.SessionID, ex.PromptText, updates)

	// Update git diff if a project is now attributable
	primary := w.projectForExchange(ex, ws.Projects, projByPath)
	if primary != nil && primary.Path != "" {
		gitDiff := shared.GetGitDiff(primary.Path)
		headSha := shared.GetHeadSha(primary.Path)
		_ = store.UpdateDraftGitDiff(transcript.SessionID, ex.PromptText, gitDiff, headSha)
	}
}

// findWorkspace returns the WorkspaceMatch that owns the given transcript file path.
func (w *Watcher) findWorkspace(filePath string) *WorkspaceMatch {
	for i := range w.workspaces {
		if strings.HasPrefix(filePath, filepath.Dir(w.workspaces[i].TranscriptDir)) {
			return &w.workspaces[i]
		}
	}
	return nil
}

// projectForExchange determines which project an exchange belongs to by looking
// at tool call file paths. Returns the project that has the most file touches.
// Returns nil if no tool calls match any registered project.
func (w *Watcher) projectForExchange(ex ParsedExchange, wsProjects []projects.Project, projByPath map[string]int) *projects.Project {
	// Count file touches per project index
	hits := map[int]int{}
	allFiles := append(ex.ChangedFiles, ex.RelevantFiles...)
	for _, f := range allFiles {
		for _, p := range wsProjects {
			if p.Path != "" && strings.HasPrefix(f, p.Path+"/") {
				hits[projByPath[p.Path]]++
				break // file can only belong to one project (most specific)
			}
		}
	}

	// Also check tool call inputs for file paths
	for _, tc := range ex.ToolCalls {
		path := extractToolCallPath(tc)
		if path == "" {
			continue
		}
		for _, p := range wsProjects {
			if p.Path != "" && strings.HasPrefix(path, p.Path+"/") {
				hits[projByPath[p.Path]]++
				break
			}
		}
	}

	if len(hits) == 0 {
		return nil
	}

	// Pick the project with the most hits
	bestIdx := -1
	bestCount := 0
	for idx, count := range hits {
		if count > bestCount {
			bestCount = count
			bestIdx = idx
		}
	}
	if bestIdx >= 0 && bestIdx < len(wsProjects) {
		return &wsProjects[bestIdx]
	}
	return nil
}

// extractToolCallPath pulls a file path from a tool call's input.
func extractToolCallPath(tc map[string]any) string {
	input, ok := tc["input"].(map[string]any)
	if !ok {
		return ""
	}
	for _, key := range []string{"filePath", "file_path", "path"} {
		if v, ok := input[key].(string); ok && v != "" {
			return v
		}
	}
	return ""
}

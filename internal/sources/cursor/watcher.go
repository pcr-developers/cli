package cursor

import (
	"fmt"
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
	"github.com/pcr-developers/cli/internal/versions"
)

const sourceID = "cursor" // pcr

// PromptScanner discovers completed Cursor agent turns by polling Cursor's
// SQLite database every 20 seconds. A turn is "complete" when the last
// assistant bubble in a user→assistant exchange has `turnDurationMs` set.
//
// When a completed turn is found for the first time:
//  1. Look up mode/model from session_state_events at the exact prompt timestamp
//  2. If agent mode: query diff_events in the exact turn window for changed files
//  3. Save the draft once, fully attributed, no retroactive enrichment needed
//
// fsnotify watches the same directories as a fast-path trigger so newly created
// sessions are processed within ~300ms instead of waiting up to 20 seconds.
type PromptScanner struct {
	dir         string
	userID      string
	diffTracker *diffTracker

	seenMu       sync.Mutex
	seen         map[string]bool // "sessionID:userBubbleID" → already saved
	initialScan  bool            // true during first scan — suppresses verbose output
}

func NewPromptScanner(dir, userID string, dt *diffTracker) *PromptScanner {
	return &PromptScanner{
		dir:         dir,
		userID:      userID,
		diffTracker: dt,
		seen:        map[string]bool{},
		initialScan: true,
	}
}

// Start launches the 20-second polling ticker and registers fsnotify as a
// fast-path trigger. Both paths call scan() which is idempotent.
func (s *PromptScanner) Start() {
	display.PrintWatcherReady("Cursor", s.dir)

	// Initial scan — saves historical prompts silently (no verbose flood).
	s.scan()
	s.initialScan = false

	// Kick off fsnotify to catch newly created sessions quickly.
	go s.watchFSNotify()

	// Periodic full scan every 20 seconds — catches any missed fsnotify events.
	ticker := time.NewTicker(20 * time.Second)
	defer ticker.Stop()
	for range ticker.C {
		s.scan()
	}
}

// scan walks the agent-transcripts directory, discovers sessions, and
// processes any completed turns that haven't been saved yet.
func (s *PromptScanner) scan() {
	if s.diffTracker != nil {
		s.diffTracker.poll()
	}

	_ = filepath.WalkDir(s.dir, func(path string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return nil
		}
		if !isAgentTranscript(path) {
			return nil
		}
		projectSlug, sessionID, ok := parseTranscriptPath(path)
		if !ok {
			return nil
		}
		s.processSession(projectSlug, sessionID)
		return nil
	})
}

// processSession checks one Cursor session for newly completed turns and
// saves any that haven't been recorded yet.
func (s *PromptScanner) processSession(projectSlug, sessionID string) {
	candidates := projects.GetAllProjectsForCursorSlug(projectSlug)
	if len(candidates) == 0 {
		return
	}

	// Register candidate projects with the DiffTracker so it begins polling them.
	// This is the only place diff_events are generated — Claude Code never calls this.
	if s.diffTracker != nil {
		for _, c := range candidates {
			s.diffTracker.registerProject(c.ProjectID)
		}
	}

	meta := GetSessionMeta(sessionID)
	if meta == nil || len(meta.Bubbles) == 0 {
		return
	}

	bubbles := meta.Bubbles

	for i, b := range bubbles {
		if b.Type != 1 || strings.TrimSpace(b.Text) == "" {
			continue
		}

		// Check if this turn is already saved.
		key := sessionID + ":" + b.BubbleID
		s.seenMu.Lock()
		if s.seen[key] {
			s.seenMu.Unlock()
			continue
		}
		s.seenMu.Unlock()

		// Find the last assistant bubble for this user turn and check
		// whether it has turnDurationMs (which means the turn is complete).
		var lastAssistant *BubbleMeta
		var responseText string
		for j := i + 1; j < len(bubbles); j++ {
			if bubbles[j].Type == 1 {
				break // next user turn — stop
			}
			if bubbles[j].Type == 2 {
				bub := bubbles[j]
				lastAssistant = &bub
				if responseText == "" && strings.TrimSpace(bub.Text) != "" {
					responseText = bub.Text
				}
			}
		}

		if lastAssistant == nil || lastAssistant.TurnDurationMs == nil {
			// Turn not yet complete — skip until next scan.
			continue
		}

		// Mark as seen immediately to prevent duplicate saves if scan()
		// is called again before this goroutine finishes.
		s.seenMu.Lock()
		s.seen[key] = true
		s.seenMu.Unlock()

		// Also verify against the persistent store in case of restarts.
		// Check both new saved_bubbles table AND legacy hash-based drafts table.
		if store.IsDraftSavedByBubble(sessionID, b.BubbleID) || store.IsDraftSaved(sessionID, b.Text) {
			continue
		}

		if !s.initialScan {
			durSec := *lastAssistant.TurnDurationMs / 1000
			display.PrintVerboseEvent("scan", fmt.Sprintf("[%s]  turn complete  %ds  %q",
				sessionID[:8], durSec, truncate(b.Text, 50)))
		}

		s.saveCompletedTurn(sessionID, meta.ComposerID, b, *lastAssistant, responseText, meta, candidates, !s.initialScan)
	}
}

// saveCompletedTurn computes full attribution for a completed agent turn and
// writes a single draft record. All attribution is resolved at save time:
//   - Mode and model from session_state_events (point-in-time lookup)
//   - Changed files from diff_events within the exact turn window
func (s *PromptScanner) saveCompletedTurn(
	sessionID, composerID string,
	userBubble, lastAssistant BubbleMeta,
	responseText string,
	meta *SessionMeta,
	candidates []projects.Project,
	showOutput bool,
) {
	capturedAt := userBubble.CreatedAt
	if capturedAt == "" {
		capturedAt = time.Now().UTC().Format(time.RFC3339)
	}

	turnStart := parseBubbleTime(capturedAt)

	// Turn end = last assistant bubble's createdAt + turnDurationMs
	var turnEnd time.Time
	if lastAssistant.CreatedAt != "" && lastAssistant.TurnDurationMs != nil {
		assistantStart := parseBubbleTime(lastAssistant.CreatedAt)
		if !assistantStart.IsZero() {
			turnEnd = assistantStart.Add(time.Duration(*lastAssistant.TurnDurationMs) * time.Millisecond)
		}
	}
	if turnEnd.IsZero() {
		turnEnd = time.Now()
	}

	// ── Mode and model from session state timeline ─────────────────────────
	mode := ""
	modelName := meta.ModelName
	if !turnStart.IsZero() {
		if stateEvent, _ := store.GetSessionStateAt(sessionID, turnStart); stateEvent != nil {
			mode = stateEvent.UnifiedMode
			if stateEvent.ModelName != "" {
				modelName = stateEvent.ModelName
			}
		}
	}
	// Fallback to session-level values.
	if mode == "" {
		mode = meta.UnifiedMode
	}

	// ── Changed files (agent mode only) ───────────────────────────────────
	var changedFiles []string
	var proj *projects.Project
	var touchedIDs []string

	// Both "agent" and "debug" modes involve autonomous tool use and can
	// produce file changes. "plan" and "chat" are read-only.
	isAgentTurn := mode == "agent" || mode == "debug"

	var consumedEventIDs []int64
	if isAgentTurn && !turnStart.IsZero() {
		// Use StartedAt as the floor so only events from THIS pcr start run
		// are considered. Events from previous runs are unreliable.
		floor := turnStart
		if s.diffTracker != nil {
			if st := s.diffTracker.startedAt_(); !st.IsZero() && st.After(floor) {
				floor = st
			}
		}
		windowEvents, _ := store.GetDiffEventsInWindow(floor, turnEnd)
		for _, e := range windowEvents {
			consumedEventIDs = append(consumedEventIDs, e.ID)
		}
		if len(candidates) == 1 {
			proj = &candidates[0]
			if proj.ProjectID != "" {
				touchedIDs = []string{proj.ProjectID}
			}
			changedFiles = extractChangedFiles(windowEvents, proj.ProjectID, touchedIDs, candidates)
		} else {
			proj, touchedIDs = resolveFromEvents(windowEvents, candidates)
			changedFiles = extractChangedFiles(windowEvents, proj.ProjectID, touchedIDs, candidates)
		}
	}

	if proj == nil {
		// Non-agent turn or no events: use best-candidate attribution.
		if len(candidates) == 1 {
			proj = &candidates[0]
		} else if len(candidates) > 1 {
			// Multi-repo workspace: pick the first registered candidate as the
			// primary project so the draft has a non-empty project_name and is
			// visible in `pcr bundle`. Tag all candidate IDs in touched_project_ids
			// so the draft also surfaces when browsing from any sub-repo.
			proj = &candidates[0]
			for _, c := range candidates {
				if c.ProjectID != "" {
					touchedIDs = append(touchedIDs, c.ProjectID)
				}
			}
		} else {
			proj = &projects.Project{}
		}
	}

	// Agent-mode prompts require verified file changes — without them there is
	// no causal attribution and no value. Non-agent turns (ask, plan, debug)
	// are always saved since they don't produce file changes by design.
	if isAgentTurn && len(changedFiles) == 0 {
		return
	}

	// ── Build file context ─────────────────────────────────────────────────
	fileContext := map[string]any{
		"capture_schema": versions.CaptureSchemaVersion,
		"cursor_mode":    mode,
		"is_agentic":     isAgentTurn,
	}
	if len(userBubble.RelevantFiles) > 0 {
		fileContext["relevant_files"] = userBubble.RelevantFiles
	}
	if len(touchedIDs) > 1 {
		fileContext["touched_project_ids"] = touchedIDs
	}
	if len(changedFiles) > 0 {
		fileContext["changed_files"] = changedFiles
	}
	if lastAssistant.TurnDurationMs != nil {
		fileContext["turn_duration_ms"] = *lastAssistant.TurnDurationMs
	}

	// ── Git metadata ───────────────────────────────────────────────────────
	var commitShas []string
	var gitDiff string
	if proj.Path != "" {
		fullSession := GetFullSessionData(sessionID)
		if fullSession != nil {
			commitShas = shared.GetCommitRange(proj.Path, fullSession.SessionCreatedAt, fullSession.SessionUpdatedAt)

			if s.userID != "" {
				var startSha, endSha string
				if len(commitShas) > 0 {
					endSha = commitShas[0]
					startSha = commitShas[len(commitShas)-1]
				}
				_ = supabase.UpsertCursorSession("", supabase.CursorSessionData{
					SessionID:         sessionID,
					Branch:            fullSession.Branch,
					ModelName:         fullSession.ModelName,
					IsAgentic:         boolPtr(fullSession.IsAgentic),
					UnifiedMode:       boolPtrFromStr(fullSession.UnifiedMode),
					PlanModeUsed:      fullSession.PlanModeUsed,
					DebugModeUsed:     fullSession.DebugModeUsed,
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
					CommitShas:        commitShas,
					Meta:              fullSession.Meta,
				}, proj.ProjectID, s.userID)
			}
		}
		gitDiff = shared.GetGitDiff(proj.Path)
	}

	// ── Save ───────────────────────────────────────────────────────────────
	hash := supabase.PromptContentHashV2(sessionID, userBubble.Text, capturedAt)
	record := supabase.PromptRecord{
		ID:            supabase.PromptIDV2(sessionID, userBubble.Text, capturedAt),
		ContentHash:   hash,
		SessionID:     sessionID,
		ProjectID:     proj.ProjectID,
		ProjectName:   proj.Name,
		PromptText:    userBubble.Text,
		ResponseText:  responseText,
		Model:         modelName,
		Source:        "cursor",
		CaptureMethod: "prompt-scanner",
		CapturedAt:    capturedAt,
		UserID:        s.userID,
		FileContext:   fileContext,
	}

	if err := store.SaveDraft(record, commitShas, gitDiff); err != nil {
		display.PrintError("cursor", "Failed to save draft: "+err.Error())
		return
	}

	// Attribution is now baked into the draft record — delete the consumed
	// diff_events so they don't accumulate indefinitely.
	_ = store.DeleteDiffEventsByID(consumedEventIDs)

	// Mark the bubble ID in the persistent store so restarts don't re-save.
	_ = store.MarkBubbleSaved(sessionID, userBubble.BubbleID, hash)

	branch := ""
	if fs := GetFullSessionData(sessionID); fs != nil {
		branch = fs.Branch
	}
	displayName := proj.Name
	if displayName == "" {
		displayName = "?"
	}
	if !showOutput {
		return
	}
	if s.userID == "" {
		display.PrintDrafted(display.DraftDisplayOptions{
			ProjectName:   displayName,
			Branch:        branch,
			PromptText:    userBubble.Text,
			ExchangeCount: 1,
		})
	} else {
		display.PrintCaptured(display.CaptureDisplayOptions{
			ProjectName:   displayName,
			Branch:        branch,
			PromptText:    userBubble.Text,
			ExchangeCount: 1,
		})
	}
}

// ─── fsnotify fast path ───────────────────────────────────────────────────────

func (s *PromptScanner) watchFSNotify() {
	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		display.PrintError("cursor fsnotify", err.Error())
		return
	}
	defer watcher.Close()

	// Watch existing directories.
	_ = filepath.WalkDir(s.dir, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			_ = watcher.Add(path)
		}
		return nil
	})

	debounce := time.NewTimer(0)
	<-debounce.C // drain initial tick

	for {
		select {
		case event, ok := <-watcher.Events:
			if !ok {
				return
			}
			if event.Has(fsnotify.Create) {
				if info, err := os.Stat(event.Name); err == nil && info.IsDir() {
					_ = watcher.Add(event.Name)
				}
			}
			if (event.Has(fsnotify.Write) || event.Has(fsnotify.Create)) && isAgentTranscript(event.Name) {
				// Debounce: wait 500ms after the last write before scanning.
				debounce.Reset(500 * time.Millisecond)
			}
		case <-debounce.C:
			s.scan()
		case err, ok := <-watcher.Errors:
			if !ok {
				return
			}
			display.PrintError("cursor watcher", err.Error())
		}
	}
}

// ─── Path helpers ─────────────────────────────────────────────────────────────

func isAgentTranscript(path string) bool {
	return strings.HasSuffix(path, ".jsonl") &&
		strings.Contains(path, "/agent-transcripts/") &&
		!strings.Contains(path, "/subagents/")
}

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

func parseBubbleTime(s string) time.Time {
	t, err := time.Parse(time.RFC3339, s)
	if err == nil {
		return t
	}
	t, _ = time.Parse("2006-01-02T15:04:05.999Z", s)
	return t
}

// ─── Attribution helpers ──────────────────────────────────────────────────────

func resolveFromEvents(events []store.DiffEvent, candidates []projects.Project) (*projects.Project, []string) {
	hits := map[string]int{}
	byID := map[string]*projects.Project{}
	for i := range candidates {
		if candidates[i].ProjectID != "" {
			byID[candidates[i].ProjectID] = &candidates[i]
		}
	}
	for _, e := range events {
		if _, ok := byID[e.ProjectID]; ok {
			hits[e.ProjectID] += len(e.Files)
		}
	}
	var allIDs []string
	for id := range hits {
		allIDs = append(allIDs, id)
	}
	var primary *projects.Project
	for id, p := range byID {
		if hits[id] == 0 {
			continue
		}
		if primary == nil || hits[id] > hits[primary.ProjectID] ||
			(hits[id] == hits[primary.ProjectID] && len(p.Path) > len(primary.Path)) {
			primary = p
		}
	}
	if primary == nil {
		return &projects.Project{}, nil
	}
	return primary, allIDs
}

func extractChangedFiles(events []store.DiffEvent, primaryProjectID string, touchedIDs []string, candidates []projects.Project) []string {
	if len(events) == 0 {
		return nil
	}
	filter := map[string]bool{}
	if primaryProjectID != "" {
		filter[primaryProjectID] = true
	}
	for _, id := range touchedIDs {
		if id != "" {
			filter[id] = true
		}
	}
	if len(filter) == 0 {
		for _, c := range candidates {
			if c.ProjectID != "" {
				filter[c.ProjectID] = true
			}
		}
	}

	seen := map[string]bool{}
	var files []string
	for _, e := range events {
		if len(filter) > 0 && !filter[e.ProjectID] {
			continue
		}
		for _, f := range e.Files {
			if !seen[f] {
				seen[f] = true
				files = append(files, f)
			}
		}
	}
	return files
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

func boolPtr(b bool) *bool { return &b }

func boolPtrFromStr(s string) *bool {
	if s == "" {
		return nil
	}
	b := s != ""
	return &b
}

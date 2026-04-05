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

// Poller is the interface the Cursor watcher uses to force an immediate
// DiffTracker poll before querying diff events. Defined here to avoid an
// import cycle between sources/cursor and the parent sources package.
type Poller interface {
	Poll()
}

const sourceID = "cursor"

type Watcher struct {
	dir         string
	userID      string
	state       *shared.FileState
	dedup       *shared.Deduplicator
	diffTracker Poller // forced poll before event queries

	timerMu sync.Mutex
	timers  map[string]*time.Timer

	lastFiredMu sync.Mutex
	startedAt   time.Time
	lastFiredAt time.Time
}

func NewWatcher(dir, userID string, dt Poller) *Watcher {
	return &Watcher{
		dir:         dir,
		userID:      userID,
		state:       shared.NewFileState(sourceID),
		dedup:       shared.NewDeduplicator(),
		timers:      map[string]*time.Timer{},
		diffTracker: dt,
	}
}

func (w *Watcher) Start() {
	w.lastFiredMu.Lock()
	w.startedAt = time.Now()
	w.lastFiredMu.Unlock()

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

	// Initial walk: register directories and record current line counts as
	// baselines — do NOT process existing content. Only new prompts written
	// after pcr start is running will be captured.
	_ = filepath.WalkDir(w.dir, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			_ = watcher.Add(path)
		} else if isAgentTranscript(path) {
			content, err := os.ReadFile(path)
			if err != nil {
				return nil
			}
			lines := filterNonEmpty(strings.Split(strings.TrimSpace(string(content)), "\n"))
			w.state.Set(path, len(lines))
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
			t.Reset(300 * time.Millisecond)
		} else {
			w.timers[path] = time.AfterFunc(300*time.Millisecond, func() {
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

	// Always re-read session metadata on file change so we pick up
	// bubble data that Cursor may have just written to the DB.
	InvalidateSessionCache(sessionUUID)

	// Resolve project candidates for this workspace slug.
	// When the Cursor workspace contains multiple git repos (e.g. opening
	// pcr-developers/ which holds cli/, pcr-dev/, etc.), we get several
	// candidates and must resolve per-prompt using relevant_files.
	candidates := projects.GetAllProjectsForCursorSlug(projectSlug)
	if len(candidates) == 0 {
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

	// Forced sync poll: capture any file changes that happened since the last
	// background DiffTracker poll. This ensures changes made during THIS exchange
	// (between JSONL write and watcher fire) are in the diff_events table before
	// we query it for per-bubble attribution.
	if w.diffTracker != nil {
		w.diffTracker.Poll()
	}

	w.lastFiredMu.Lock()
	firingTime := time.Now()
	w.lastFiredAt = firingTime
	w.lastFiredMu.Unlock()

	sessionMeta := GetSessionMeta(sessionUUID)

	schemaV := versions.CaptureSchemaVersion
	var newPrompts []supabase.PromptRecord

	// v14+: use DB bubble data directly — clean text, accurate timestamps, rich metadata.
	// v13 and below: fall back to JSONL parser.
	if sessionMeta != nil && len(sessionMeta.Bubbles) > 0 && sessionMeta.Bubbles[0].CreatedAt != "" {
		model := sessionMeta.ModelName
		bubbles := sessionMeta.Bubbles

		for i, b := range bubbles {
			if b.Type != 1 || strings.TrimSpace(b.Text) == "" {
				continue
			}

			// Find the next assistant bubble for response text
			var responseText string
			for j := i + 1; j < len(bubbles); j++ {
				if bubbles[j].Type == 2 && strings.TrimSpace(bubbles[j].Text) != "" {
					responseText = bubbles[j].Text
					break
				}
				if bubbles[j].Type == 1 {
					break // hit the next user turn
				}
			}

			hash := supabase.PromptContentHash(sessionUUID, b.Text, "")
			if w.dedup.IsDuplicate(sessionUUID, hash) {
				continue
			}

			if store.IsDraftSaved(sessionUUID, b.Text) {
				w.dedup.Mark(sessionUUID, hash)
				continue
			}
			w.dedup.Mark(sessionUUID, hash)

			capturedAt := b.CreatedAt
			if capturedAt == "" {
				capturedAt = time.Now().UTC().Format(time.RFC3339)
			}

			// Per-prompt time-window attribution.
			//
			// Each prompt N's attribution window is [T_N, T_{N+1}]:
			//   - T_N   = when this prompt was sent (b.CreatedAt)
			//   - T_{N+1} = when the NEXT user bubble was sent
			//             (or the current firing time for the last prompt)
			//
			// Files changed in that window = what the AI did in response to N.
			// No events in window → unattributed (honest).
			// Single-candidate workspace → always tag (no ambiguity).
			bubbleTime := parseBubbleTime(capturedAt)
			nextBubbleTime := firingTime
			for j := i + 1; j < len(bubbles); j++ {
				if bubbles[j].Type == 1 && bubbles[j].CreatedAt != "" {
					if t := parseBubbleTime(bubbles[j].CreatedAt); !t.IsZero() {
						nextBubbleTime = t
						break
					}
				}
			}

			var proj *projects.Project
			var touchedIDs []string
			if len(candidates) == 1 {
				proj = &candidates[0]
				if proj.ProjectID != "" {
					touchedIDs = []string{proj.ProjectID}
				}
			} else if !bubbleTime.IsZero() {
				events, _ := store.GetDiffEventsInWindow(bubbleTime, nextBubbleTime)
				proj, touchedIDs = resolveFromEvents(events, candidates)
			} else {
				proj = &projects.Project{}
			}

			fileContext := map[string]any{
				"capture_schema": schemaV,
			}
			if b.IsAgentic != nil {
				fileContext["is_agentic"] = *b.IsAgentic
			} else {
				fileContext["is_agentic"] = sessionMeta.IsAgentic
			}
			if b.UnifiedMode != "" {
				fileContext["cursor_mode"] = b.UnifiedMode
			} else if sessionMeta.UnifiedMode != "" {
				fileContext["cursor_mode"] = sessionMeta.UnifiedMode
			}
			if len(b.RelevantFiles) > 0 {
				fileContext["relevant_files"] = b.RelevantFiles
			}
			if len(touchedIDs) > 1 {
				// More than one repo touched — dashboard uses this for cross-repo display.
				fileContext["touched_project_ids"] = touchedIDs
			}

			newPrompts = append(newPrompts, supabase.PromptRecord{
				SessionID:     sessionUUID,
				ProjectName:   proj.Name,
				PromptText:    b.Text,
				ResponseText:  responseText,
				Model:         model,
				Source:        "cursor",
				CaptureMethod: "file-watcher",
				CapturedAt:    capturedAt,
				UserID:        w.userID,
				ProjectID:     proj.ProjectID,
				FileContext:   fileContext,
			})
		}
	} else {
		// v13 and below: parse from JSONL, enrich with DB metadata where available.
		session := ParseCursorTranscript(string(content), sessionUUID, projectSlug)
		if len(session.Prompts) == 0 {
			return
		}

		var assistantBubbles []BubbleMeta
		if sessionMeta != nil {
			for _, b := range sessionMeta.Bubbles {
				if b.Type == 2 {
					assistantBubbles = append(assistantBubbles, b)
				}
			}
		}

		for promptIdx, p := range session.Prompts {
			hash := supabase.PromptContentHash(p.SessionID, p.PromptText, "")
			if w.dedup.IsDuplicate(p.SessionID, hash) {
				continue
			}
			if store.IsDraftSaved(p.SessionID, p.PromptText) {
				w.dedup.Mark(p.SessionID, hash)
				continue
			}
			w.dedup.Mark(p.SessionID, hash)

			fileContext := map[string]any{"capture_schema": schemaV}
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

			// Per-prompt time-window attribution using SubmittedAt timestamps.
			promptTime := time.Time{}
			if bubble != nil && bubble.SubmittedAt != nil {
				promptTime = time.UnixMilli(*bubble.SubmittedAt).UTC()
			}
			nextPromptTime := firingTime
			if promptIdx+1 < len(session.Prompts) {
				next := session.Prompts[promptIdx+1]
				if nextBubble := findAssistantBubble(assistantBubbles, promptIdx+1); nextBubble != nil && nextBubble.SubmittedAt != nil {
					nextPromptTime = time.UnixMilli(*nextBubble.SubmittedAt).UTC()
				} else {
					_ = next // keep promptIdx+1 in scope
				}
			}

			var proj *projects.Project
			var touchedIDs []string
			if len(candidates) == 1 {
				proj = &candidates[0]
				if proj.ProjectID != "" {
					touchedIDs = []string{proj.ProjectID}
				}
			} else if !promptTime.IsZero() {
				events, _ := store.GetDiffEventsInWindow(promptTime, nextPromptTime)
				proj, touchedIDs = resolveFromEvents(events, candidates)
			} else {
				proj = &projects.Project{}
			}

			if len(touchedIDs) > 1 {
				fileContext["touched_project_ids"] = touchedIDs
			}

			model := p.Model
			if sessionMeta != nil && sessionMeta.ModelName != "" {
				model = sessionMeta.ModelName
			}

			p.CapturedAt = capturedAt
			p.Model = model
			p.UserID = w.userID
			p.ProjectID = proj.ProjectID
			p.ProjectName = proj.Name
			p.FileContext = fileContext
			newPrompts = append(newPrompts, p)
		}
	}

	if len(newPrompts) == 0 {
		return
	}

	// Get commit range and git diff per sub-project (cached by path).
	fullSession := GetFullSessionData(sessionUUID)
	type projectGitData struct {
		commitShas []string
		gitDiff    string
		headSha    string
	}
	gitCache := map[string]*projectGitData{}
	getGitData := func(proj *projects.Project) *projectGitData {
		if d, ok := gitCache[proj.Path]; ok {
			return d
		}
		d := &projectGitData{}
		if fullSession != nil && proj.Path != "" {
			d.commitShas = getCommitRange(proj.Path, fullSession.SessionCreatedAt, fullSession.SessionUpdatedAt)
		}
		d.gitDiff = getGitDiff(proj.Path)
		d.headSha = getHeadSha(proj.Path)
		gitCache[proj.Path] = d

		// Upsert cursor session metadata once per sub-project (non-fatal).
		if w.userID != "" && fullSession != nil && proj.Path != "" {
			var startSha, endSha string
			if len(d.commitShas) > 0 {
				endSha = d.commitShas[0]
				startSha = d.commitShas[len(d.commitShas)-1]
			}
			_ = supabase.UpsertCursorSession("", supabase.CursorSessionData{
				SessionID:         sessionUUID,
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
				CommitShas:        d.commitShas,
				Meta:              fullSession.Meta,
			}, proj.ProjectID, w.userID)
		}
		return d
	}

	// Re-resolve project per saved prompt to get the right git data.
	// We stored ProjectID on each PromptRecord; find the matching candidate.
	findCandidate := func(projectID string) *projects.Project {
		if projectID != "" {
			for i := range candidates {
				if candidates[i].ProjectID == projectID {
					return &candidates[i]
				}
			}
		}
		// No match or unattributed — return empty project so display shows "?"
		return &projects.Project{}
	}

	for _, p := range newPrompts {
		proj := findCandidate(p.ProjectID)
		gd := getGitData(proj)
		if err := store.SaveDraft(p, gd.commitShas, gd.gitDiff, gd.headSha); err != nil {
			display.PrintError("cursor", "Failed to save draft: "+err.Error())
		}
	}

	last := newPrompts[len(newPrompts)-1]
	branch := ""
	if fullSession != nil {
		branch = fullSession.Branch
	}
	lastProj := findCandidate(last.ProjectID)
	displayName := lastProj.Name
	if displayName == "" {
		displayName = "?"
	}
	if w.userID == "" {
		display.PrintDrafted(display.DraftDisplayOptions{
			ProjectName:   displayName,
			Branch:        branch,
			PromptText:    last.PromptText,
			ExchangeCount: len(newPrompts),
		})
	} else {
		display.PrintCaptured(display.CaptureDisplayOptions{
			ProjectName:   displayName,
			Branch:        branch,
			PromptText:    last.PromptText,
			ExchangeCount: len(newPrompts),
		})
	}
}

func getHeadSha(projectPath string) string {
	if projectPath == "" {
		return ""
	}
	out, err := exec.Command("git", "-C", projectPath, "rev-parse", "HEAD").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

func getGitDiff(projectPath string) string {
	if projectPath == "" {
		return ""
	}
	cmd := exec.Command("git", "diff", "HEAD")
	cmd.Dir = projectPath
	out, err := cmd.Output()
	if err != nil || len(out) == 0 {
		return ""
	}
	const maxBytes = 50_000
	if len(out) > maxBytes {
		return string(out[:maxBytes]) + "\n[truncated]"
	}
	return string(out)
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


// parseBubbleTime parses Cursor's bubble createdAt field which may include
// milliseconds ("2026-04-04T18:16:30.123Z") that time.RFC3339 can't handle.
func findAssistantBubble(bubbles []BubbleMeta, idx int) *BubbleMeta {
	if idx < len(bubbles) {
		return &bubbles[idx]
	}
	return nil
}

func parseBubbleTime(s string) time.Time {
	t, err := time.Parse(time.RFC3339, s)
	if err == nil {
		return t
	}
	t, _ = time.Parse("2006-01-02T15:04:05.999Z", s)
	return t
}

// resolveFromEvents picks the primary project and all touched project IDs
// from a set of DiffTracker events, filtered to the given workspace candidates.
// Returns an empty project (unattributed) when no events match any candidate.
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

func boolPtr(b bool) *bool { return &b }

func boolPtrFromStr(s string) *bool {
	if s == "" {
		return nil
	}
	b := s != ""
	return &b
}

package claudecode

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
	"github.com/pcr-developers/cli/internal/versions"
)

const sourceID = "claude-code"

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
		display.PrintError("claude-code", "Directory not found: "+w.dir+". Will activate when it appears.")
	}
	display.PrintWatcherReady("Claude Code", w.dir)

	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		display.PrintError("claude-code", "Failed to create watcher: "+err.Error())
		return
	}
	defer watcher.Close()

	// Initial walk: register directories for watching and record current line
	// counts as baselines — do NOT process existing content. Only prompts
	// written after pcr start is running will be captured.
	_ = filepath.WalkDir(w.dir, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			_ = watcher.Add(path)
		} else if strings.HasSuffix(path, ".jsonl") {
			content, err := os.ReadFile(path)
			if err != nil {
				return nil
			}
			lines := shared.FilterNonEmpty(strings.Split(strings.TrimSpace(string(content)), "\n"))
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
			if (event.Has(fsnotify.Write) || event.Has(fsnotify.Create)) &&
				strings.HasSuffix(event.Name, ".jsonl") {
				w.scheduleProcess(event.Name)
			}
		case err, ok := <-watcher.Errors:
			if !ok {
				return
			}
			display.PrintError("claude-code watcher", err.Error())
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
			w.processFile(path, false)
		})
	}
}

func (w *Watcher) processFile(filePath string, forceFullScan bool) {
	// Extract slug from ~/.claude/projects/<slug>/<session>.jsonl
	normalized := filepath.ToSlash(filePath)
	parts := strings.Split(normalized, "/")
	projectsIdx := -1
	for i, p := range parts {
		if p == "projects" {
			projectsIdx = i
		}
	}
	if projectsIdx < 0 || projectsIdx+1 >= len(parts) {
		return
	}
	projectSlug := parts[projectsIdx+1]

	project := projects.GetProjectForClaudeSlug(projectSlug)
	if project == nil {
		return // not a registered project
	}
	projectName := project.Name

	content, err := os.ReadFile(filePath)
	if err != nil {
		return
	}

	lines := shared.FilterNonEmpty(strings.Split(strings.TrimSpace(string(content)), "\n"))
	prevCount := w.state.Get(filePath)

	if !forceFullScan && len(lines) <= prevCount {
		return
	}
	w.state.Set(filePath, len(lines))

	session := ParseClaudeCodeSession(string(content), projectName, filePath)
	if len(session.Prompts) == 0 {
		return
	}

	schemaV := versions.CaptureSchemaVersion
	baseFileContext := map[string]any{
		"capture_schema": schemaV,
	}

	// Build per-project git data cache upfront — needed in dedup paths too.
	type projectGitData struct {
		commitShas []string
		gitDiff    string
		headSha    string
	}
	gitCache := map[string]*projectGitData{}
	projByID := map[string]*projects.Project{}
	for _, p := range projects.Load() {
		p := p // capture loop var
		if p.ProjectID != "" {
			projByID[p.ProjectID] = &p
		}
	}
	getGitData := func(projectID string) *projectGitData {
		proj, ok := projByID[projectID]
		if !ok || proj.Path == "" {
			proj = project // fall back to the file's project
		}
		if d, ok := gitCache[proj.Path]; ok {
			return d
		}
		d := &projectGitData{
			gitDiff: shared.GetGitDiff(proj.Path),
			headSha: shared.GetHeadSha(proj.Path),
		}
		if proj.Path != "" && session.SessionCreatedAt != "" {
			d.commitShas = shared.GetCommitsSince(proj.Path, session.SessionCreatedAt)
		}
		gitCache[proj.Path] = d
		return d
	}

	// touchedProjectIDs returns all registered project IDs whose files appear in
	// tool call paths. Used to set touched_project_ids in file_context so that
	// `pcr bundle` shows the draft in every repo the prompt actually edited.
	touchedProjectIDs := func(toolCalls []map[string]any, projByID map[string]*projects.Project) []string {
		seen := map[string]bool{}
		for _, tc := range toolCalls {
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
			if path == "" {
				continue
			}
			for id, proj := range projByID {
				if proj.Path != "" && strings.HasPrefix(path, proj.Path+"/") {
					seen[id] = true
				}
			}
		}
		ids := make([]string, 0, len(seen))
		for id := range seen {
			ids = append(ids, id)
		}
		return ids
	}

	// repoSnapshots returns git snapshots for repos OTHER than primaryProjectID
	// that are referenced by tool call file paths. Stored in file_context so
	// push can compute incremental diffs for each touched repo.
	repoSnapshots := func(toolCalls []map[string]any, primaryProjectID string) map[string]any {
		result := map[string]any{}
		for _, tc := range toolCalls {
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
			if path == "" {
				continue
			}
			for id, proj := range projByID {
				if id == primaryProjectID || proj.Path == "" {
					continue
				}
				if strings.HasPrefix(path, proj.Path+"/") {
					if _, ok := result[id]; !ok {
						gd := getGitData(id)
						result[id] = map[string]any{
							"head_sha": gd.headSha,
							"git_diff": gd.gitDiff,
						}
					}
				}
			}
		}
		if len(result) == 0 {
			return nil
		}
		return result
	}

	var newPrompts []supabase.PromptRecord
	for _, p := range session.Prompts {
		hash := supabase.PromptContentHash(p.SessionID, p.PromptText, "")
		if w.dedup.IsDuplicate(p.SessionID, hash) {
			_ = store.UpdateDraftResponse(p.SessionID, p.PromptText, p.ResponseText)
			_ = store.UpdateDraftToolCalls(p.SessionID, p.PromptText, p.ToolCalls)
			if snaps := repoSnapshots(p.ToolCalls, p.ProjectID); len(snaps) > 0 {
				_ = store.MergeDraftFileContext(p.SessionID, p.PromptText, map[string]any{"repo_snapshots": snaps})
			}
			// Backfill git_diff if it was empty when the draft was first saved
			// (captured before Claude finished editing files).
			if gd := getGitData(p.ProjectID); gd.gitDiff != "" {
				_ = store.UpdateDraftGitDiff(p.SessionID, p.PromptText, gd.gitDiff, gd.headSha)
			}
			continue
		}
		if store.IsDraftSaved(p.SessionID, p.PromptText) {
			w.dedup.Mark(p.SessionID, hash)
			_ = store.UpdateDraftResponse(p.SessionID, p.PromptText, p.ResponseText)
			_ = store.UpdateDraftToolCalls(p.SessionID, p.PromptText, p.ToolCalls)
			fc := map[string]any{}
			if snaps := repoSnapshots(p.ToolCalls, p.ProjectID); len(snaps) > 0 {
				fc["repo_snapshots"] = snaps
			}
			if ids := touchedProjectIDs(p.ToolCalls, projByID); len(ids) > 0 {
				fc["touched_project_ids"] = ids
			}
			if len(fc) > 0 {
				_ = store.MergeDraftFileContext(p.SessionID, p.PromptText, fc)
			}
			// Backfill git_diff if it was empty when the draft was first saved.
			if gd := getGitData(p.ProjectID); gd.gitDiff != "" {
				_ = store.UpdateDraftGitDiff(p.SessionID, p.PromptText, gd.gitDiff, gd.headSha)
			}
			continue
		}

		merged := map[string]any{}
		for k, v := range baseFileContext {
			merged[k] = v
		}
		for k, v := range p.FileContext {
			merged[k] = v
		}
		p.UserID = w.userID
		p.ProjectID = project.ProjectID
		p.ProjectName = project.Name

		// Populate touched_project_ids from tool call file paths so the draft
		// surfaces in every repo's `pcr bundle`, not just the session's repo.
		if ids := touchedProjectIDs(p.ToolCalls, projByID); len(ids) > 0 {
			merged["touched_project_ids"] = ids
		}
		p.FileContext = merged
		newPrompts = append(newPrompts, p)
	}

	if len(newPrompts) == 0 {
		return
	}

	for i := range newPrompts {
		p := &newPrompts[i]
		gd := getGitData(p.ProjectID)
		if snaps := repoSnapshots(p.ToolCalls, p.ProjectID); len(snaps) > 0 {
			p.FileContext["repo_snapshots"] = snaps
		}
		if err := store.SaveDraft(*p, gd.commitShas, gd.gitDiff, gd.headSha); err != nil {
			display.PrintError("claude-code", "Failed to save draft: "+err.Error())
			continue
		}
		hash := supabase.PromptContentHashV2(p.SessionID, p.PromptText, p.CapturedAt)
		w.dedup.Mark(p.SessionID, hash)
	}

	last := newPrompts[len(newPrompts)-1]
	if w.userID == "" {
		display.PrintDrafted(display.DraftDisplayOptions{
			ProjectName:   projectName,
			Branch:        session.Branch,
			PromptText:    last.PromptText,
			ExchangeCount: len(newPrompts),
		})
	} else {
		display.PrintCaptured(display.CaptureDisplayOptions{
			ProjectName:   projectName,
			Branch:        session.Branch,
			PromptText:    last.PromptText,
			ToolCalls:     last.ToolCalls,
			ExchangeCount: len(newPrompts),
		})
	}

}

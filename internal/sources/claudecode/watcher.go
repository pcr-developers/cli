package claudecode

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
	parts := strings.Split(filePath, "/")
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

	lines := filterNonEmpty(strings.Split(strings.TrimSpace(string(content)), "\n"))
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

	var newPrompts []supabase.PromptRecord
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

		merged := map[string]any{}
		for k, v := range baseFileContext {
			merged[k] = v
		}
		for k, v := range p.FileContext {
			merged[k] = v
		}
		p.UserID = w.userID
		p.ProjectID = project.ProjectID
		p.FileContext = merged
		newPrompts = append(newPrompts, p)
	}

	if len(newPrompts) == 0 {
		return
	}

	// Get git commits since session start
	var commitShas []string
	if project.Path != "" && session.SessionCreatedAt != "" {
		commitShas = getCommitsSince(project.Path, session.SessionCreatedAt)
	}

	// Capture git diff at the moment of capture (capped at 50KB)
	gitDiff := getGitDiff(project.Path)

	for _, p := range newPrompts {
		if err := store.SaveDraft(p, commitShas, gitDiff); err != nil {
			display.PrintError("claude-code", "Failed to save draft: "+err.Error())
		}
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

func getCommitsSince(projectPath, sinceISO string) []string {
	cmd := exec.Command("git", "log", "--format=%H", `--after=`+sinceISO)
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

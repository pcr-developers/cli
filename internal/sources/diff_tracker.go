package sources

import (
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/pcr-developers/cli/internal/config"
	"github.com/pcr-developers/cli/internal/display"
	"github.com/pcr-developers/cli/internal/projects"
	"github.com/pcr-developers/cli/internal/store"
)

// DiffTracker polls Cursor-registered projects every pollInterval and records a
// DiffEvent whenever file content changes (tracked modifications AND new
// untracked files). It uses git status --porcelain to get the full list of
// dirty/new files, then hashes each file's diff to detect changes to
// already-dirty files between polls.
//
// Only projects registered via RegisterProject are polled — the Cursor watcher
// calls this when it discovers an active session. Claude Code does not use
// diff_events (it snapshots git diff at parse time), so Claude-only projects
// are never polled.
//
// State is persisted to disk across restarts so that files already dirty when
// pcr start was last stopped are never re-attributed as newly changed. Only
// files whose diff hash has genuinely changed since the last known state will
// produce a DiffEvent. This means a file that has been sitting dirty with the
// same uncommitted content for days will never contaminate prompt attribution.
type DiffTracker struct {
	pollInterval time.Duration
	startedAt    time.Time // when this pcr start instance began — used as attribution floor

	mu                sync.Mutex
	prevState         map[string]map[string]string // projectPath → relFile → diffHash
	watchedProjectIDs map[string]bool              // only poll these; empty = nothing yet
	freshStart        bool                         // true on every startup → silent first poll to avoid restart bursts
}

func NewDiffTracker(interval time.Duration) *DiffTracker {
	t := &DiffTracker{
		pollInterval:      interval,
		startedAt:         time.Now(),
		prevState:         map[string]map[string]string{},
		watchedProjectIDs: map[string]bool{}, // nothing polled until Cursor registers
		freshStart:        true,
	}
	t.loadState()
	return t
}

// RegisterProject adds a project to the poll list. Called by the Cursor
// PromptScanner when it discovers an active session for a project.
func (t *DiffTracker) RegisterProject(id string) {
	if id == "" {
		return
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	t.watchedProjectIDs[id] = true
}

// StartedAt returns the time this DiffTracker instance started.
// Only diff_events recorded AFTER this time are trustworthy for attribution.
func (t *DiffTracker) StartedAt() time.Time {
	return t.startedAt
}

// Poll runs one immediate diff check across all registered projects.
// Called synchronously by the Cursor watcher before querying diff events
// to ensure last-second file changes are captured before attribution.
func (t *DiffTracker) Poll() {
	t.poll()
}

// Start launches the polling goroutine. Call as go tracker.Start().
func (t *DiffTracker) Start() {
	// Purge diff_events from before this instance started. Those came from a
	// previous run and may include restart bursts or stale dirty files.
	// Do NOT clear changed_files — those were set by the PromptScanner at
	// save time with correct attribution and must survive restarts.
	_ = store.PruneDiffEvents(t.startedAt)

	ticker := time.NewTicker(t.pollInterval)
	for range ticker.C {
		t.poll()
	}
}

func (t *DiffTracker) poll() {
	// Snapshot watched project IDs under lock so we don't hold it during I/O.
	t.mu.Lock()
	watchedIDs := make(map[string]bool, len(t.watchedProjectIDs))
	for id := range t.watchedProjectIDs {
		watchedIDs[id] = true
	}
	t.mu.Unlock()

	if len(watchedIDs) == 0 {
		// No Cursor sessions discovered yet — nothing to poll.
		t.mu.Lock()
		t.freshStart = false
		t.mu.Unlock()
		return
	}

	projs := projects.Load()
	now := time.Now()

	for _, p := range projs {
		if p.Path == "" || p.ProjectID == "" || !watchedIDs[p.ProjectID] {
			continue
		}

		current := t.getDirtyHashes(p.Path)

		t.mu.Lock()
		prev, knownProject := t.prevState[p.Path]

		// A file is "newly changed" only if its hash differs from the last known
		// state — which may come from a previous pcr start run (via persisted
		// state) or the current run. Pre-existing dirty files with unchanged
		// content will never appear here because their hash was already saved.
		var changed []string
		for rel, hash := range current {
			if prev[rel] != hash {
				changed = append(changed, filepath.Join(p.Path, rel))
			}
		}
		t.prevState[p.Path] = current
		t.mu.Unlock()

		// Skip recording if this is the global freshStart poll or if this is the
		// first time we've seen this project (no prevState baseline yet). In both
		// cases we silently establish the baseline so the next poll only records
		// genuine new changes.
		if t.freshStart || !knownProject {
			continue
		}

		if len(changed) > 0 {
			_ = store.RecordDiffEvent(p.ProjectID, p.Name, changed, now)
			for _, f := range changed {
				display.PrintVerboseEvent("diff", fmt.Sprintf("[%s]  %s", p.Name, filepath.Base(f)))
			}
		}
	}

	// After the first silent poll, clear freshStart so subsequent polls record normally.
	t.mu.Lock()
	t.freshStart = false
	t.mu.Unlock()

	// Prune diff_events older than 1 hour as a backstop for any events not
	// consumed by a Cursor turn (e.g. if a session was abandoned mid-turn).
	_ = store.PruneDiffEvents(now.Add(-1 * time.Hour))

	t.saveState()
}

// ─── State persistence ────────────────────────────────────────────────────────

func statePath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, config.PCRDir, "diff-tracker-state.json")
}

func (t *DiffTracker) loadState() {
	data, err := os.ReadFile(statePath())
	if err != nil {
		return // no saved state — prevState stays empty, first poll will establish baseline
	}
	var saved map[string]map[string]string
	if json.Unmarshal(data, &saved) == nil {
		t.mu.Lock()
		t.prevState = saved
		t.mu.Unlock()
	}
	// freshStart remains true — the first poll is always silent to avoid a
	// restart burst where accumulated changes since last save flood diff_events
	// and contaminate the first prompt's attribution window.
}

func (t *DiffTracker) saveState() {
	t.mu.Lock()
	data, err := json.Marshal(t.prevState)
	t.mu.Unlock()
	if err != nil {
		return
	}
	home, _ := os.UserHomeDir()
	dir := filepath.Join(home, config.PCRDir)
	_ = os.MkdirAll(dir, 0755)
	_ = os.WriteFile(statePath(), data, 0644)
}

// ─── Git helpers ──────────────────────────────────────────────────────────────

// getDirtyHashes returns a map of relFile → sha256(fileContent) for every
// dirty file in the project. Uses git status --porcelain to enumerate dirty
// files, then hashes the actual FILE CONTENT (not git diff output) for stable
// change detection. Git diff output is unstable — the index line changes when
// the git index is refreshed by other commands (go build, git log, etc.),
// causing false positives on every poll for unchanged files.
func (t *DiffTracker) getDirtyHashes(projectPath string) map[string]string {
	out, err := exec.Command("git", "-C", projectPath, "status", "--porcelain").Output()
	if err != nil || len(out) == 0 {
		return map[string]string{}
	}

	result := map[string]string{}
	for _, line := range filterLines(strings.Split(string(out), "\n")) {
		if len(line) < 4 {
			continue
		}
		rel := strings.TrimSpace(line[3:])

		if len(rel) >= 2 && rel[0] == '"' && rel[len(rel)-1] == '"' {
			rel = rel[1 : len(rel)-1]
		}

		if rel == "" || strings.HasSuffix(rel, "/") {
			continue
		}

		absPath := filepath.Join(projectPath, rel)
		content, err := os.ReadFile(absPath)
		if err != nil {
			continue
		}
		h := sha256.Sum256(content)
		result[rel] = fmt.Sprintf("%x", h[:16])
	}
	return result
}

func filterLines(lines []string) []string {
	var out []string
	for _, l := range lines {
		if strings.TrimSpace(l) != "" {
			out = append(out, l)
		}
	}
	return out
}

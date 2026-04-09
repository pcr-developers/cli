package cursor

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

// diffTracker polls Cursor-registered projects every pollInterval and records a
// DiffEvent whenever file content changes (tracked modifications AND new
// untracked files). It uses git status --porcelain to get the full list of
// dirty/new files, then hashes each file's content to detect changes between polls.
//
// Only projects registered via registerProject are polled — the PromptScanner
// calls this when it discovers an active session. Claude Code does not use
// diff_events (it snapshots git diff at parse time), so Claude-only projects
// are never polled.
//
// State is persisted to disk across restarts so that files already dirty when
// pcr start was last stopped are never re-attributed as newly changed.
type diffTracker struct {
	pollInterval time.Duration
	startedAt    time.Time

	mu                sync.Mutex
	prevState         map[string]map[string]string // projectPath → relFile → contentHash
	watchedProjectIDs map[string]bool              // only poll these; empty = nothing yet
	freshStart        bool
}

func newDiffTracker(interval time.Duration) *diffTracker {
	t := &diffTracker{
		pollInterval:      interval,
		startedAt:         time.Now(),
		prevState:         map[string]map[string]string{},
		watchedProjectIDs: map[string]bool{},
		freshStart:        true,
	}
	t.loadState()
	return t
}

func (t *diffTracker) registerProject(id string) {
	if id == "" {
		return
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	t.watchedProjectIDs[id] = true
}

func (t *diffTracker) startedAt_() time.Time { return t.startedAt }

func (t *diffTracker) poll() {
	t.mu.Lock()
	watchedIDs := make(map[string]bool, len(t.watchedProjectIDs))
	for id := range t.watchedProjectIDs {
		watchedIDs[id] = true
	}
	t.mu.Unlock()

	if len(watchedIDs) == 0 {
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
		var changed []string
		for rel, hash := range current {
			if prev[rel] != hash {
				changed = append(changed, filepath.Join(p.Path, rel))
			}
		}
		t.prevState[p.Path] = current
		t.mu.Unlock()

		// Skip recording on global freshStart or first time seeing this project —
		// silently establish baseline so next poll only records genuine new changes.
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

	t.mu.Lock()
	t.freshStart = false
	t.mu.Unlock()

	// Prune diff_events older than 1 hour — backstop for sessions abandoned mid-turn.
	_ = store.PruneDiffEvents(now.Add(-1 * time.Hour))

	t.saveState()
}

func (t *diffTracker) start() {
	_ = store.PruneDiffEvents(t.startedAt)
	ticker := time.NewTicker(t.pollInterval)
	for range ticker.C {
		t.poll()
	}
}

// ─── State persistence ────────────────────────────────────────────────────────

func diffTrackerStatePath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, config.PCRDir, "diff-tracker-state.json")
}

func (t *diffTracker) loadState() {
	data, err := os.ReadFile(diffTrackerStatePath())
	if err != nil {
		return
	}
	var saved map[string]map[string]string
	if json.Unmarshal(data, &saved) == nil {
		t.mu.Lock()
		t.prevState = saved
		t.mu.Unlock()
	}
}

func (t *diffTracker) saveState() {
	t.mu.Lock()
	data, err := json.Marshal(t.prevState)
	t.mu.Unlock()
	if err != nil {
		return
	}
	home, _ := os.UserHomeDir()
	_ = os.MkdirAll(filepath.Join(home, config.PCRDir), 0755)
	_ = os.WriteFile(diffTrackerStatePath(), data, 0644)
}

// ─── Git helpers ──────────────────────────────────────────────────────────────

func (t *diffTracker) getDirtyHashes(projectPath string) map[string]string {
	out, err := exec.Command("git", "-C", projectPath, "status", "--porcelain").Output()
	if err != nil || len(out) == 0 {
		return map[string]string{}
	}

	result := map[string]string{}
	for _, line := range filterNonEmpty(strings.Split(string(out), "\n")) {
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
		content, err := os.ReadFile(filepath.Join(projectPath, rel))
		if err != nil {
			continue
		}
		h := sha256.Sum256(content)
		result[rel] = fmt.Sprintf("%x", h[:16])
	}
	return result
}

package sources

import (
	"crypto/sha256"
	"fmt"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/pcr-developers/cli/internal/projects"
	"github.com/pcr-developers/cli/internal/store"
)

// DiffTracker polls each registered project every pollInterval and records a
// DiffEvent whenever file content changes (tracked modifications AND new
// untracked files). It uses git status --porcelain to get the full list of
// dirty/new files, then hashes each file's diff to detect changes to
// already-dirty files between polls.
//
// Events are stored in the diff_events SQLite table and queried by the Cursor
// watcher to attribute each prompt to the repos whose files changed in that
// prompt's time window.
type DiffTracker struct {
	pollInterval time.Duration

	mu          sync.Mutex
	prevState   map[string]map[string]string // projectPath → relFile → diffHash
	initialized bool                          // true after the first poll (baseline established)
}

func NewDiffTracker(interval time.Duration) *DiffTracker {
	return &DiffTracker{
		pollInterval: interval,
		prevState:    map[string]map[string]string{},
	}
}

// Poll runs one immediate diff check across all registered projects.
// Called synchronously by the Cursor watcher before querying diff events
// to ensure last-second file changes are captured before attribution.
func (t *DiffTracker) Poll() {
	t.poll()
}

// Start launches the polling goroutine. Call as go tracker.Start().
func (t *DiffTracker) Start() {
	// Prune events older than 7 days on startup.
	_ = store.PruneDiffEvents(time.Now().Add(-7 * 24 * time.Hour))

	ticker := time.NewTicker(t.pollInterval)
	for range ticker.C {
		t.poll()
	}
}

func (t *DiffTracker) poll() {
	projs := projects.Load()
	now := time.Now()

	t.mu.Lock()
	firstPoll := !t.initialized
	t.initialized = true
	t.mu.Unlock()

	for _, p := range projs {
		if p.Path == "" || p.ProjectID == "" {
			continue
		}

		current := t.getDirtyHashes(p.Path)

		t.mu.Lock()
		prev := t.prevState[p.Path]

		var changed []string
		for rel, hash := range current {
			if prev[rel] != hash { // new file OR content changed
				changed = append(changed, filepath.Join(p.Path, rel))
			}
		}
		t.prevState[p.Path] = current
		t.mu.Unlock()

		// First poll: just establish baseline — don't record events.
		// All files dirty at startup are pre-existing; only changes
		// that happen while the tracker is running should be recorded.
		if firstPoll {
			continue
		}

		if len(changed) > 0 {
			_ = store.RecordDiffEvent(p.ProjectID, p.Name, changed, now)
		}
	}
}

// getDirtyHashes returns a map of relFile → sha256(diff) for every dirty file
// in the project at projectPath. Uses git status --porcelain to enumerate all
// modified/untracked files, then hashes each file's diff for change detection.
func (t *DiffTracker) getDirtyHashes(projectPath string) map[string]string {
	// git status --porcelain gives us both tracked-modified (" M", "M ", "MM")
	// and untracked new files ("??") in one fast command.
	out, err := exec.Command("git", "-C", projectPath, "status", "--porcelain").Output()
	if err != nil || len(out) == 0 {
		return map[string]string{}
	}

	result := map[string]string{}
	for _, line := range filterLines(strings.Split(string(out), "\n")) {
		if len(line) < 4 {
			continue
		}
		status := line[:2]
		rel := strings.TrimSpace(line[3:])

		// Skip binary-only entries and directory markers
		if rel == "" || strings.HasSuffix(rel, "/") {
			continue
		}
		// Untracked file — hash its content (no diff available)
		if status == "??" {
			h, _ := hashFileContent(projectPath, rel)
			result[rel] = "new:" + h
			continue
		}
		// Tracked modified — hash the diff for change detection
		diffOut, _ := exec.Command("git", "-C", projectPath, "diff", "HEAD", "--", rel).Output()
		if len(diffOut) == 0 {
			// Staged-only change: diff vs index
			diffOut, _ = exec.Command("git", "-C", projectPath, "diff", "--cached", "--", rel).Output()
		}
		h := sha256.Sum256(diffOut)
		result[rel] = fmt.Sprintf("%x", h[:8])
	}
	return result
}

func hashFileContent(dir, rel string) (string, error) {
	out, err := exec.Command("sha256sum", filepath.Join(dir, rel)).Output()
	if err != nil {
		// Fallback: use file path as hash (still detects new files)
		return rel, nil
	}
	parts := strings.Fields(string(out))
	if len(parts) > 0 {
		return parts[0][:16], nil
	}
	return rel, nil
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

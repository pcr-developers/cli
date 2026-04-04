package cursor

import (
	"os"
	"path/filepath"
	"sort"
	"time"
)

type Source struct {
	DiffTracker Poller
}

func (s *Source) Name() string { return "Cursor" }

func (s *Source) Start(userID string) {
	home, _ := os.UserHomeDir()
	dir := filepath.Join(home, ".cursor", "projects")
	w := NewWatcher(dir, userID, s.DiffTracker)
	w.Start()
}

// ForceSync creates a one-shot watcher and force-processes the N most recently
// modified transcript files. Called by `pcr bundle` to capture any prompts that
// haven't been picked up by the background watcher yet.
func ForceSync(userID string, dt Poller, maxFiles int) {
	// Forced DiffTracker poll first so events are current.
	if dt != nil {
		dt.Poll()
	}

	home, _ := os.UserHomeDir()
	dir := filepath.Join(home, ".cursor", "projects")

	// Find recently modified transcript JSONL files.
	type entry struct {
		path    string
		modTime time.Time
	}
	var files []entry
	_ = filepath.WalkDir(dir, func(path string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return nil
		}
		if !isAgentTranscript(path) {
			return nil
		}
		info, err := d.Info()
		if err != nil {
			return nil
		}
		files = append(files, entry{path, info.ModTime()})
		return nil
	})

	// Sort newest first, take up to maxFiles.
	sort.Slice(files, func(i, j int) bool {
		return files[i].modTime.After(files[j].modTime)
	})
	if len(files) > maxFiles {
		files = files[:maxFiles]
	}

	if len(files) == 0 {
		return
	}

	// Process with a fresh watcher (forceFullScan=false so dedup + IsDraftSaved
	// prevent duplicates; only genuinely new bubbles get saved).
	w := NewWatcher(dir, userID, dt)
	w.startedAt = time.Now().Add(-24 * time.Hour) // allow attributing recent history
	for _, f := range files {
		w.processFile(f.path, false)
	}
}

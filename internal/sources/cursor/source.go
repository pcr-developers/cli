package cursor

import (
	"os"
	"path/filepath"
	"sort"
	"time"
)

type Source struct{}

func (s *Source) Name() string { return "Cursor" }

func (s *Source) Start(userID string) {
	home, _ := os.UserHomeDir()
	dir := filepath.Join(home, ".cursor", "projects")

	// DiffTracker: polls registered projects every 3s to record timestamped
	// file-change events used for per-prompt attribution in agent turns.
	dt := newDiffTracker(3 * time.Second)
	go dt.start()

	// PromptScanner: discovers completed turns (turnDurationMs present) and
	// saves fully-attributed drafts. Polls every 20s + fsnotify fast path.
	scanner := NewPromptScanner(dir, userID, dt)
	go scanner.Start()

	// SessionStateWatcher: tracks mode/model/context changes every 2s so the
	// PromptScanner can do a point-in-time lookup for each prompt's exact mode.
	stateWatcher := NewSessionStateWatcher()
	go stateWatcher.Start()
}

// ForceSync runs a one-shot scan of the N most recently modified transcript
// files. Called by `pcr bundle` to capture any turns that haven't been picked
// up by the background scanner yet.
func ForceSync(userID string, maxFiles int) {
	home, _ := os.UserHomeDir()
	dir := filepath.Join(home, ".cursor", "projects")

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

	sort.Slice(files, func(i, j int) bool {
		return files[i].modTime.After(files[j].modTime)
	})
	if len(files) > maxFiles {
		files = files[:maxFiles]
	}
	if len(files) == 0 {
		return
	}

	scanner := NewPromptScanner(dir, userID, nil)
	for _, f := range files {
		projectSlug, sessionID, ok := parseTranscriptPath(f.path)
		if !ok {
			continue
		}
		scanner.processSession(projectSlug, sessionID)
	}
}

package claudecode

import (
	"os"
	"path/filepath"
)

// Source implements sources.CaptureSource for Claude Code.
type Source struct{}

func (s *Source) Name() string { return "Claude Code" }

func (s *Source) Start(userID string) {
	home, _ := os.UserHomeDir()
	dir := filepath.Join(home, ".claude", "projects")
	w := NewWatcher(dir, userID)
	w.Start()
}

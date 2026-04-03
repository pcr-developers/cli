package cursor

import (
	"os"
	"path/filepath"
)

type Source struct{}

func (s *Source) Name() string { return "Cursor" }

func (s *Source) Start(userID string) {
	home, _ := os.UserHomeDir()
	dir := filepath.Join(home, ".cursor", "projects")
	w := NewWatcher(dir, userID)
	w.Start()
}

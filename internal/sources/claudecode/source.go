package claudecode

import (
	"os"
	"path/filepath"
	"runtime"
)

// Source implements sources.CaptureSource for Claude Code.
type Source struct{}

func (s *Source) Name() string { return "Claude Code" }

func (s *Source) Start(userID string) {
	home, _ := os.UserHomeDir()
	var dir string
	if runtime.GOOS == "windows" {
		appData := os.Getenv("APPDATA")
		if appData == "" {
			appData = filepath.Join(home, "AppData", "Roaming")
		}
		dir = filepath.Join(appData, "Claude", "projects")
	} else {
		dir = filepath.Join(home, ".claude", "projects")
	}
	w := NewWatcher(dir, userID)
	w.Start()
}

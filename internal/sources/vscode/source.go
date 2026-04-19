package vscode

import (
	"github.com/pcr-developers/cli/internal/display"
	"github.com/pcr-developers/cli/internal/sources/shared"
)

// Source implements sources.CaptureSource for VS Code Copilot Chat.
type Source struct{}

func (s *Source) Name() string { return "VS Code" }

func (s *Source) Start(userID string) {
	workspaces := ScanWorkspaces()
	if len(workspaces) == 0 {
		display.PrintError("vscode", "No VS Code workspaces match registered projects. Will activate when new workspaces appear.")
	}

	w := NewWatcher(userID, workspaces)

	// Process emptyWindowChatSessions in the background (persistent sessions
	// without a workspace — e.g. when a user opens a single file).
	go func() {
		ProcessEmptyWindowSessions(userID, shared.NewFileState("vscode-empty"), shared.NewDeduplicator())
	}()

	w.Start()
}

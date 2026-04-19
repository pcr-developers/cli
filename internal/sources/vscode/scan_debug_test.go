package vscode

import (
	"fmt"
	"testing"
)

func TestScanDebug(t *testing.T) {
	bases := workspaceStorageBases()
	fmt.Printf("Storage bases: %v\n", bases)
	
	matches := ScanWorkspaces()
	fmt.Printf("Found %d workspace matches\n", len(matches))
	for _, m := range matches {
		fmt.Printf("  Hash: %s\n", m.Hash)
		fmt.Printf("  Folder: %s\n", m.FolderPath)
		fmt.Printf("  TranscriptDir: %s\n", m.TranscriptDir)
		fmt.Printf("  Projects: %d\n", len(m.Projects))
		for _, p := range m.Projects {
			fmt.Printf("    - %s (%s)\n", p.Name, p.Path)
		}
	}
}

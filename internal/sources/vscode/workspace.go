package vscode

import (
	"encoding/json"
	"net/url"
	"os"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/pcr-developers/cli/internal/projects"
)

// WorkspaceMatch maps a VS Code workspace hash directory to the projects it covers.
type WorkspaceMatch struct {
	Hash           string
	TranscriptDir  string // full path to transcripts/ dir
	FolderPath     string // workspace folder on disk
	Projects       []projects.Project
}

// workspaceJSON is the structure of workspace.json inside each hash dir.
type workspaceJSON struct {
	Folder    string `json:"folder"`
	Workspace string `json:"workspace"`
}

// ScanWorkspaces discovers VS Code workspace hash directories and matches them
// to registered PCR projects. Returns only workspaces that match at least one project.
func ScanWorkspaces() []WorkspaceMatch {
	bases := workspaceStorageBases()
	allProjects := projects.Load()

	var matches []WorkspaceMatch
	for _, base := range bases {
		entries, err := os.ReadDir(base)
		if err != nil {
			continue
		}
		for _, e := range entries {
			if !e.IsDir() {
				continue
			}
			hashDir := filepath.Join(base, e.Name())
			wsFile := filepath.Join(hashDir, "workspace.json")
			data, err := os.ReadFile(wsFile)
			if err != nil {
				continue
			}
			var ws workspaceJSON
			if err := json.Unmarshal(data, &ws); err != nil {
				continue
			}

			folderPath := resolveWorkspaceFolder(ws)
			if folderPath == "" {
				continue
			}

			matched := matchProjects(folderPath, allProjects)
			if len(matched) == 0 {
				continue
			}

			transcriptDir := filepath.Join(hashDir, "GitHub.copilot-chat", "transcripts")
			matches = append(matches, WorkspaceMatch{
				Hash:          e.Name(),
				TranscriptDir: transcriptDir,
				FolderPath:    folderPath,
				Projects:      matched,
			})
		}
	}
	return matches
}

// resolveWorkspaceFolder extracts the folder path from workspace.json.
func resolveWorkspaceFolder(ws workspaceJSON) string {
	if ws.Folder != "" {
		return uriToPath(ws.Folder)
	}
	// Multi-root workspace: we'd need to parse the .code-workspace file.
	// For now, return empty — multi-root is a future enhancement.
	return ""
}

// uriToPath converts a file:// URI to a local filesystem path.
func uriToPath(uri string) string {
	if !strings.HasPrefix(uri, "file://") {
		return uri // already a path
	}
	u, err := url.Parse(uri)
	if err != nil {
		return strings.TrimPrefix(uri, "file://")
	}
	p := u.Path
	// On Windows url.Parse returns /C:/... — strip the leading slash.
	if runtime.GOOS == "windows" && len(p) > 2 && p[0] == '/' && p[2] == ':' {
		p = p[1:]
	}
	return p
}

// matchProjects returns all registered projects whose path is equal to or is a
// subdirectory of the workspace folder path. Skips projects whose path no
// longer exists on disk.
func matchProjects(workspaceFolder string, allProjects []projects.Project) []projects.Project {
	workspaceFolder = filepath.Clean(workspaceFolder)
	// On Windows, drive letters may differ in case (c: vs C:).
	if runtime.GOOS == "windows" {
		workspaceFolder = strings.ToLower(workspaceFolder)
	}
	var matched []projects.Project
	for _, p := range allProjects {
		if p.Path == "" {
			continue
		}
		projPath := filepath.Clean(p.Path)
		cmpPath := projPath
		if runtime.GOOS == "windows" {
			cmpPath = strings.ToLower(projPath)
		}
		// Match if workspace IS the project, or workspace is an ancestor of the project
		if cmpPath == workspaceFolder || strings.HasPrefix(cmpPath, workspaceFolder+string(filepath.Separator)) {
			// Skip if the project path no longer exists on disk
			if _, err := os.Stat(projPath); os.IsNotExist(err) {
				continue
			}
			matched = append(matched, p)
		}
	}
	return matched
}

// workspaceStorageBases returns all platform-appropriate VS Code workspace
// storage directories, including Insiders and VSCodium variants.
func workspaceStorageBases() []string {
	home, _ := os.UserHomeDir()
	if home == "" {
		return nil
	}

	var configBase string
	switch runtime.GOOS {
	case "darwin":
		configBase = filepath.Join(home, "Library", "Application Support")
	case "linux":
		configBase = filepath.Join(home, ".config")
	case "windows":
		configBase = os.Getenv("APPDATA")
		if configBase == "" {
			configBase = filepath.Join(home, "AppData", "Roaming")
		}
	default:
		return nil
	}

	variants := []string{"Code", "Code - Insiders", "VSCodium"}
	var bases []string
	for _, v := range variants {
		base := filepath.Join(configBase, v, "User", "workspaceStorage")
		if info, err := os.Stat(base); err == nil && info.IsDir() {
			bases = append(bases, base)
		}
	}
	return bases
}

// GlobalStorageBase returns the path to VS Code's globalStorage directory for
// emptyWindowChatSessions.
func GlobalStorageBase() string {
	home, _ := os.UserHomeDir()
	if home == "" {
		return ""
	}
	var configBase string
	switch runtime.GOOS {
	case "darwin":
		configBase = filepath.Join(home, "Library", "Application Support")
	case "linux":
		configBase = filepath.Join(home, ".config")
	case "windows":
		configBase = os.Getenv("APPDATA")
		if configBase == "" {
			configBase = filepath.Join(home, "AppData", "Roaming")
		}
	default:
		return ""
	}
	return filepath.Join(configBase, "Code", "User", "globalStorage")
}

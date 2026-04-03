package projects

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/pcr-developers/cli/internal/config"
)

type Project struct {
	Path         string `json:"path"`
	CursorSlug   string `json:"cursorSlug"`
	ClaudeSlug   string `json:"claudeSlug"`
	Name         string `json:"name"`
	RegisteredAt string `json:"registeredAt"`
	ProjectID    string `json:"projectId,omitempty"`
}

type registry struct {
	Projects []Project `json:"projects"`
}

func filePath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, config.PCRDir, "projects.json")
}

func Load() []Project {
	data, err := os.ReadFile(filePath())
	if err != nil {
		return nil
	}
	var r registry
	if err := json.Unmarshal(data, &r); err != nil {
		return nil
	}
	return r.Projects
}

func save(projects []Project) error {
	path := filePath()
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(registry{Projects: projects}, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, data, 0644)
}

// PathToCursorSlug converts an absolute path to a Cursor project slug.
// e.g. /Users/foo/Desktop/PCR.dev -> Users-foo-Desktop-PCR-dev
func PathToCursorSlug(path string) string {
	s := strings.TrimPrefix(path, "/")
	s = strings.ReplaceAll(s, "/", "-")
	s = strings.ReplaceAll(s, ".", "-")
	return s
}

// PathToClaudeSlug converts an absolute path to a Claude Code project slug.
// e.g. /Users/foo/Desktop/PCR.dev -> -Users-foo-Desktop-PCR.dev
func PathToClaudeSlug(path string) string {
	return strings.ReplaceAll(path, "/", "-")
}

func Register(projectPath string) Project {
	projects := Load()
	cursorSlug := PathToCursorSlug(projectPath)
	claudeSlug := PathToClaudeSlug(projectPath)
	name := filepath.Base(projectPath)

	existing := -1
	for i, p := range projects {
		if p.Path == projectPath {
			existing = i
			break
		}
	}

	entry := Project{
		Path:       projectPath,
		CursorSlug: cursorSlug,
		ClaudeSlug: claudeSlug,
		Name:       name,
	}
	if existing >= 0 {
		entry.ProjectID = projects[existing].ProjectID
		entry.RegisteredAt = projects[existing].RegisteredAt
		projects[existing] = entry
	} else {
		entry.RegisteredAt = time.Now().UTC().Format(time.RFC3339)
		projects = append(projects, entry)
	}

	_ = save(projects)
	return entry
}

// Unregister removes a project from the registry. Returns false if not found.
func Unregister(projectPath string) bool {
	projs := Load()
	for i, p := range projs {
		if p.Path == projectPath {
			projs = append(projs[:i], projs[i+1:]...)
			_ = save(projs)
			return true
		}
	}
	return false
}

func UpdateProjectID(projectPath, projectID string) {
	projects := Load()
	for i, p := range projects {
		if p.Path == projectPath {
			projects[i].ProjectID = projectID
			_ = save(projects)
			return
		}
	}
}

func GetBestProjectForCursorSlug(slug string) *Project {
	projects := Load()
	for i, p := range projects {
		if p.CursorSlug == slug {
			return &projects[i]
		}
	}
	// prefix fallback for monorepos
	var best *Project
	for i, p := range projects {
		if strings.HasPrefix(p.CursorSlug, slug+"-") {
			if best == nil || len(p.CursorSlug) > len(best.CursorSlug) {
				best = &projects[i]
			}
		}
	}
	return best
}

func GetProjectForClaudeSlug(slug string) *Project {
	projects := Load()
	for i, p := range projects {
		if p.ClaudeSlug == slug {
			return &projects[i]
		}
	}
	return nil
}

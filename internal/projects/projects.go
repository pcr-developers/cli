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

// GetBestProjectForCursorSlug returns the single registered project for a
// Cursor workspace slug, or nil when the slug maps to multiple sub-projects
// (use GetAllProjectsForCursorSlug + GetProjectForFile in that case).
func GetBestProjectForCursorSlug(slug string) *Project {
	projects := Load()
	// Exact match first — the workspace IS a registered project.
	for i, p := range projects {
		if p.CursorSlug == slug {
			return &projects[i]
		}
	}
	// Single prefix match: only one sub-project under this workspace path.
	var matches []*Project
	for i, p := range projects {
		if strings.HasPrefix(p.CursorSlug, slug+"-") {
			matches = append(matches, &projects[i])
		}
	}
	if len(matches) == 1 {
		return matches[0]
	}
	// Multiple sub-projects: caller should use GetAllProjectsForCursorSlug.
	return nil
}

// GetAllProjectsForCursorSlug returns every project that lives under the
// given Cursor workspace slug (exact match + all sub-projects).
func GetAllProjectsForCursorSlug(slug string) []Project {
	projs := Load()
	var result []Project
	for _, p := range projs {
		if p.CursorSlug == slug || strings.HasPrefix(p.CursorSlug, slug+"-") {
			result = append(result, p)
		}
	}
	return result
}

// GetProjectForFile returns the registered project whose path is the deepest
// (longest) prefix of filePath. candidates should come from
// GetAllProjectsForCursorSlug so we only search relevant projects.
func GetProjectForFile(filePath string, candidates []Project) *Project {
	var best *Project
	var bestLen int
	for i, p := range candidates {
		if (strings.HasPrefix(filePath, p.Path+"/") || filePath == p.Path) && len(p.Path) > bestLen {
			best = &candidates[i]
			bestLen = len(p.Path)
		}
	}
	return best
}

func GetProjectForClaudeSlug(slug string) *Project {
	projects := Load()
	// Exact match first
	for i, p := range projects {
		if p.ClaudeSlug == slug {
			return &projects[i]
		}
	}
	// Ancestor match: slug may represent a parent directory of a registered project
	// e.g. Claude Code open at /Users/foo/pcr-developers captures sessions for cli sub-project
	for i, p := range projects {
		parent := p.Path
		for {
			parent = filepath.Dir(parent)
			if parent == "." || parent == "/" {
				break
			}
			if PathToClaudeSlug(parent) == slug {
				return &projects[i]
			}
		}
	}
	return nil
}

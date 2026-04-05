package cmd

import (
	"os"
	"strings"

	"github.com/pcr-developers/cli/internal/projects"
)

// projectContext holds the resolved project filter for the current directory.
type projectContext struct {
	// display name for the header (innermost matched project)
	name string
	// for filtering store queries
	ids   []string
	names []string
}

// resolveProjectContext finds all registered projects that are at or above the
// current git repo root. This includes the exact match AND any ancestor
// directories that are also registered (e.g. the parent monorepo workspace).
func resolveProjectContext() projectContext {
	cwd, _ := os.Getwd()
	if gitRoot := gitOutput("git", "rev-parse", "--show-toplevel"); gitRoot != "" {
		cwd = gitRoot
	}

	projs := projects.Load()

	seen := map[string]bool{}
	var ctx projectContext

	for _, p := range projs {
		// match: exact path, cwd is inside this project, OR this project is inside cwd
		// (the last case handles running from a parent workspace folder like pcr-developers/)
		if p.Path != cwd && !strings.HasPrefix(cwd, p.Path+"/") && !strings.HasPrefix(p.Path, cwd+"/") {
			continue
		}
		if seen[p.Path] {
			continue
		}
		seen[p.Path] = true

		// innermost (longest path match) becomes the display name
		if ctx.name == "" || len(p.Path) > len(getProjectByName(projs, ctx.name).Path) {
			ctx.name = p.Name
		}

		if p.ProjectID != "" {
			ctx.ids = append(ctx.ids, p.ProjectID)
		}
		// include both human-readable name and legacy slug so old captures match
		if p.Name != "" {
			ctx.names = append(ctx.names, p.Name)
		}
		if p.ClaudeSlug != "" {
			ctx.names = append(ctx.names, p.ClaudeSlug)
		}
	}

	return ctx
}

func getProjectByName(projs []projects.Project, name string) projects.Project {
	for _, p := range projs {
		if p.Name == name {
			return p
		}
	}
	return projects.Project{}
}

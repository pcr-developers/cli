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
	// singleRepo is true when cwd is exactly a registered project (not a parent workspace).
	// In this mode, bundling is scoped strictly to this repo — cross-repo drafts remain
	// available for bundling from their other repos.
	singleRepo bool
}

// resolveProjectContext finds the registered project(s) relevant to the current directory.
//
// Exact match (cwd == registered project path): returns only that single project.
// This prevents parent workspaces from being pulled in when you're inside a specific repo.
//
// No exact match (e.g. running from pcr-developers/ parent folder): returns all
// registered projects nested under cwd, enabling workspace-wide bundling.
func resolveProjectContext() projectContext {
	cwd, _ := os.Getwd()
	if gitRoot := gitOutput("git", "rev-parse", "--show-toplevel"); gitRoot != "" {
		cwd = gitRoot
	}

	projs := projects.Load()

	// Exact match: running from within a specific registered project.
	// Scope strictly to that project — don't pull in parent workspaces.
	for _, p := range projs {
		if p.Path == cwd {
			ctx := projectContext{name: p.Name, singleRepo: true}
			if p.ProjectID != "" {
				ctx.ids = []string{p.ProjectID}
			}
			if p.Name != "" {
				ctx.names = []string{p.Name}
			}
			if p.ClaudeSlug != "" {
				ctx.names = append(ctx.names, p.ClaudeSlug)
			}
			return ctx
		}
	}

	// No exact match — include all registered projects nested under cwd
	// (handles parent workspace directories like pcr-developers/).
	seen := map[string]bool{}
	var ctx projectContext
	for _, p := range projs {
		if !strings.HasPrefix(p.Path, cwd+"/") {
			continue
		}
		if seen[p.Path] {
			continue
		}
		seen[p.Path] = true
		if ctx.name == "" || len(p.Path) > len(getProjectByName(projs, ctx.name).Path) {
			ctx.name = p.Name
		}
		if p.ProjectID != "" {
			ctx.ids = append(ctx.ids, p.ProjectID)
		}
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

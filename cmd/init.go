package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
	"github.com/pcr-developers/cli/internal/projects"
	"github.com/pcr-developers/cli/internal/supabase"
)

var initCmd = &cobra.Command{
	Use:   "init",
	Short: "Register the current directory (or all sub-repos) as tracked projects",
	Long: `Register the current directory as a tracked project.

If the current directory is not a git repo but contains sub-directories that
are (e.g. an org workspace with multiple repos), all of them are registered
in one shot.`,
	RunE: func(cmd *cobra.Command, args []string) error {
		unregister, _ := cmd.Flags().GetBool("unregister")

		projectPath, err := os.Getwd()
		if err != nil {
			return err
		}

		if unregister {
			if projects.Unregister(projectPath) {
				fmt.Fprintf(os.Stderr, "PCR: Unregistered %s\n", projectPath)
			} else {
				fmt.Fprintf(os.Stderr, "PCR: %s was not registered.\n", projectPath)
			}
			return nil
		}

		// If this directory itself is a git repo, register it directly.
		if isGitRepo(projectPath) {
			registerOne(projectPath)
			fmt.Fprintln(os.Stderr, "\nPCR: Run `pcr start` to begin capturing prompts.")
			return nil
		}

		// Not a git repo — scan immediate subdirectories for git repos.
		entries, err := os.ReadDir(projectPath)
		if err != nil {
			return err
		}
		var found []string
		for _, e := range entries {
			if !e.IsDir() {
				continue
			}
			sub := filepath.Join(projectPath, e.Name())
			if isGitRepo(sub) {
				found = append(found, sub)
			}
		}

		if len(found) == 0 {
			fmt.Fprintln(os.Stderr, "PCR: No git repositories found in the current directory.")
			fmt.Fprintln(os.Stderr, "     Run `pcr init` inside a git repo directory.")
			return nil
		}

		fmt.Fprintf(os.Stderr, "PCR: Found %d git repo%s — registering all.\n\n", len(found), plural(len(found)))
		for _, sub := range found {
			registerOne(sub)
			fmt.Fprintln(os.Stderr)
		}
		fmt.Fprintln(os.Stderr, "PCR: Run `pcr start` to begin capturing prompts.")
		return nil
	},
}

// registerOne registers a single git repo directory locally and remotely.
func registerOne(projectPath string) {
	gitRemote := gitOutputIn(projectPath, "git", "remote", "get-url", "origin")

	project := projects.Register(projectPath)
	fmt.Fprintf(os.Stderr, "  ✓ %s\n", project.Name)
	fmt.Fprintf(os.Stderr, "    Path:        %s\n", projectPath)
	fmt.Fprintf(os.Stderr, "    Cursor slug: %s\n", project.CursorSlug)

	a := auth.Load()
	if a != nil && gitRemote != "" {
		projectID, err := supabase.RegisterProject("", project.Name, gitRemote, projectPath, a.UserID)
		if err != nil {
			fmt.Fprintf(os.Stderr, "    Remote:      failed (%v)\n", err)
		} else if projectID != "" {
			projects.UpdateProjectID(projectPath, projectID)
			fmt.Fprintf(os.Stderr, "    Remote ID:   %s\n", projectID)
		}
	} else if gitRemote == "" {
		fmt.Fprintf(os.Stderr, "    Remote:      skipped (no git remote)\n")
	} else {
		fmt.Fprintf(os.Stderr, "    Remote:      skipped (not logged in — run `pcr login`)\n")
	}
}

// isGitRepo returns true if the directory contains a .git entry.
func isGitRepo(dir string) bool {
	_, err := os.Stat(filepath.Join(dir, ".git"))
	return err == nil
}

// gitOutputIn runs a git command in a specific directory.
func gitOutputIn(dir string, name string, args ...string) string {
	cmd := exec.Command(name, args...)
	cmd.Dir = dir
	out, err := cmd.Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

func gitOutput(name string, args ...string) string {
	out, err := exec.Command(name, args...).Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

func init() {
	initCmd.Flags().Bool("unregister", false, "Unregister the current project")
}

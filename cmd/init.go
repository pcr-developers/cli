package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"strings"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
	"github.com/pcr-developers/cli/internal/projects"
	"github.com/pcr-developers/cli/internal/supabase"
)

var initCmd = &cobra.Command{
	Use:   "init",
	Short: "Register the current directory as a tracked project",
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

		// Detect git remote
		gitRemote := gitOutput("git", "remote", "get-url", "origin")

		project := projects.Register(projectPath)
		fmt.Fprintf(os.Stderr, "PCR: Registered %s\n", project.Name)
		fmt.Fprintf(os.Stderr, "  Path:         %s\n", projectPath)
		fmt.Fprintf(os.Stderr, "  Claude slug:  %s\n", project.ClaudeSlug)
		fmt.Fprintf(os.Stderr, "  Cursor slug:  %s\n", project.CursorSlug)

		// Register remotely if logged in
		a := auth.Load()
		if a != nil && gitRemote != "" {
			projectID, err := supabase.RegisterProject(a.Token, project.Name, gitRemote, projectPath)
			if err == nil && projectID != "" {
				projects.UpdateProjectID(projectPath, projectID)
				fmt.Fprintf(os.Stderr, "  Remote ID:    %s\n", projectID)
			}
		}

		fmt.Fprintln(os.Stderr, "\nPCR: Run `pcr start` to begin capturing prompts.")
		return nil
	},
}

func gitOutput(name string, args ...string) string {
	out, err := exec.Command(name, args...).Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}


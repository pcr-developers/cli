package cmd

import (
	"fmt"
	"os"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
	"github.com/pcr-developers/cli/internal/projects"
)

var statusCmd = &cobra.Command{
	Use:   "status",
	Short: "Show auth and registered project info",
	Run: func(cmd *cobra.Command, args []string) {
		a := auth.Load()
		if a != nil {
			fmt.Fprintf(os.Stderr, "PCR: Logged in (user: %s)\n", a.UserID)
		} else {
			fmt.Fprintln(os.Stderr, "PCR: Not logged in. Run `pcr login`.")
		}

		projs := projects.Load()
		if len(projs) == 0 {
			fmt.Fprintln(os.Stderr, "PCR: No projects registered. Run `pcr init` in a project directory.")
			return
		}

		fmt.Fprintf(os.Stderr, "\nRegistered projects (%d):\n", len(projs))
		for _, p := range projs {
			remoteInfo := ""
			if p.ProjectID != "" {
				remoteInfo = fmt.Sprintf(" [remote: %s]", p.ProjectID)
			}
			fmt.Fprintf(os.Stderr, "  %s%s\n    %s\n", p.Name, remoteInfo, p.Path)
		}
	},
}

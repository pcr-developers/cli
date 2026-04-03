package cmd

import (
	"fmt"
	"os"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
)

var logoutCmd = &cobra.Command{
	Use:   "logout",
	Short: "Remove saved credentials",
	Run: func(cmd *cobra.Command, args []string) {
		auth.Clear()
		fmt.Fprintln(os.Stderr, "PCR: Logged out.")
	},
}

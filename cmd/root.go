package cmd

import (
	"fmt"
	"os"

	"github.com/spf13/cobra"
)

var Version = "dev"

var rootCmd = &cobra.Command{
	Use:          "pcr",
	Short:        "PCR.dev — prompt & code review",
	SilenceUsage: true,
}

func Execute(version string) {
	Version = version
	rootCmd.Version = version
	rootCmd.SetVersionTemplate("{{.Version}}\n")
	rootCmd.CompletionOptions.DisableDefaultCmd = true
	if err := rootCmd.Execute(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func init() {
	rootCmd.AddCommand(
		loginCmd,
		logoutCmd,
		initCmd,
		startCmd,
		mcpCmd,
		statusCmd,
		bundleCmd,
		pushCmd,
		logCmd,
		showCmd,
		pullCmd,
		gcCmd,
		hookCmd,
	)
}

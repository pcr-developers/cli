package cmd

import (
	"fmt"
	"os"

	"github.com/spf13/cobra"
)

var Version = "dev"

var rootCmd = &cobra.Command{
	Use:   "pcr",
	Short: "PCR.dev — prompt & code review",
	Long: `PCR.dev v` + Version + ` — prompt & code review

Usage: pcr <command>

Commands:
  init      Register the current directory as a tracked project
  login     Authenticate with PCR.dev
  logout    Remove saved credentials
  start     Start the file watcher
  mcp       Start the MCP server on stdio
  status    Show auth and registered project info
  add       Stage draft prompts for bundling
  commit    Bundle staged prompts into a named bundle
  push      Upload committed bundles to PCR.dev
  log       Show local prompt state
  pull      Restore a pushed bundle to local drafts
  gc        Garbage collect old pushed records
  github    Set up GitHub PR integration`,
	SilenceUsage: true,
}

func Execute(version string) {
	Version = version
	rootCmd.Version = version
	rootCmd.SetVersionTemplate("{{.Version}}\n")
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
		addCmd,
		commitCmd,
		pushCmd,
		logCmd,
		pullCmd,
		gcCmd,
		githubCmd,
	)
}

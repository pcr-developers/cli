package cmd

import (
	"fmt"
	"os"
	"strconv"
	"strings"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/store"
)

var gcCmd = &cobra.Command{
	Use:   "gc",
	Short: "Clean up old pushed records or orphaned bundles",
	RunE: func(cmd *cobra.Command, args []string) error {
		allPushed, _ := cmd.Flags().GetBool("all-pushed")
		olderThan, _ := cmd.Flags().GetString("older-than")
		orphaned, _ := cmd.Flags().GetBool("orphaned")

		if orphaned {
			projectPath, _ := os.Getwd()
			if gitRoot := gitOutput("git", "rev-parse", "--show-toplevel"); gitRoot != "" {
				projectPath = gitRoot
			}
			deleted, err := store.GCOrphaned(projectPath)
			if err != nil {
				return err
			}
			if deleted == 0 {
				fmt.Fprintln(os.Stderr, "PCR: No orphaned bundles found.")
			} else {
				fmt.Fprintf(os.Stderr, "PCR: Deleted %d orphaned bundle%s (drafts restored).\n",
					deleted, plural(deleted))
			}
			return nil
		}

		unpushed, _ := cmd.Flags().GetBool("unpushed")
		if unpushed {
			deleted, err := store.GCUnpushed()
			if err != nil {
				return err
			}
			if deleted == 0 {
				fmt.Fprintln(os.Stderr, "PCR: No unpushed bundles to discard.")
			} else {
				fmt.Fprintf(os.Stderr, "PCR: Discarded %d unpushed bundle%s.\n", deleted, plural(deleted))
			}
			return nil
		}

		if allPushed {
			deleted, err := store.GCAllPushed()
			if err != nil {
				return err
			}
			if deleted == 0 {
				fmt.Fprintln(os.Stderr, "PCR: No pushed records to clean up.")
			} else {
				fmt.Fprintf(os.Stderr, "PCR: Deleted %d pushed prompt%s from local store.\n",
					deleted, plural(deleted))
			}
			return nil
		}

		days := 30
		if olderThan != "" {
			raw := strings.TrimSuffix(olderThan, "d")
			n, err := strconv.Atoi(raw)
			if err != nil || n <= 0 {
				return fmt.Errorf("invalid --older-than value: %q. Expected e.g. \"30d\" or \"7\"", olderThan)
			}
			days = n
		}

		deleted, err := store.GCPushed(days)
		if err != nil {
			return err
		}
		if deleted == 0 {
			fmt.Fprintf(os.Stderr, "PCR: No pushed records older than %d days.\n", days)
		} else {
			fmt.Fprintf(os.Stderr, "PCR: Deleted %d pushed prompt%s older than %d days.\n",
				deleted, plural(deleted), days)
		}
		return nil
	},
}

func init() {
	gcCmd.Flags().Bool("all-pushed", false, "Delete all pushed records regardless of age")
	gcCmd.Flags().String("older-than", "", "Delete pushed records older than N days (e.g. 30d or 7)")
	gcCmd.Flags().Bool("orphaned", false, "Delete unpushed bundles whose git SHA no longer exists")
	gcCmd.Flags().Bool("unpushed", false, "Discard all unpushed committed bundles")
}
